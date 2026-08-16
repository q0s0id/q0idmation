# q0lang overhaul + project graph + bundled q0s

## цель

сделать q0lang полноценной частью q0editor/q0player, а не отдельным мини-скриптом внутри `q0rg.script`.

главный runtime-путь после этой работы:

`root .q1s -> project graph -> q0l/q0lang + linked .q0s -> export external/bundled .q0s -> q0player -> deterministic runtime`

будущий путь для standalone-приложений должен получиться без переделки формата:

`bundled .q0s + tiny q0player stub -> .exe`

## что уже реально есть сейчас

- parser и runtime q0lang уже лежат в `q0s-format/src/q0lang`, то есть editor/player/shell могут использовать одно ядро.
- `q0shell` уже принимает и `.q0lang`, и `.q0l`. это должны остаться два полностью равноправных расширения одного plain utf-8 формата, как `.jpg`/`.jpeg`.
- q0editor сейчас ассоциирует только `.q1s` через `q0editor/src/assoc.rs`.
- верхние вкладки q0editor сейчас жёстко являются `ProjectTab`, внутри хранится только `ProjectWorkspace`.
- standalone q0l-документа как сущности в q0editor нет.
- нынешний q0lang editor является отдельным floating window и редактирует только `Q0rg.script`.
- текущий editor основан на egui `TextEdit`: есть syntax highlight, gutter, font picker, basic auto-indent/tab/outdent и diagnostics, но нет нормального document model, search/replace, autocomplete, spans, symbols, inline diagnostics и нормального code navigation.
- `Q0rg` сейчас хранит один общий `script: String`.
- q0player при загрузке v2 `.q0s` берёт только script entry-q0rg и исполняет его один раз.
- frame scripts на таймлайне отсутствуют.
- runtime сейчас в основном выдаёт host actions: timeline, q0shell, q0rig. это хорошая архитектурная точка расширения, её надо сохранить.
- q0player уже применяет q0rig runtime override без изменения authored project. тот же принцип нужен для transform/drawing/spawn/audio.
- `ProjectV2` пока не содержит project graph/dependencies.
- текущий `.q0s` parser запрещает trailing bytes, поэтому нельзя просто приписать bundle в хвост старого q0s. bundling обязан быть нормальной новой ревизией формата.
- текущие версии: q1s current = 20, q0s current = 19. новые поля должны идти через новые версии + backward parse defaults + roundtrip/migration tests.

## архитектурные правила

1. `.q0l` и `.q0lang` не имеют разных magic/version. это один текстовый формат, различается только extension.
2. project tree не должен жить только в ui/session. связи должны сериализоваться и доходить до exported q0s.
3. внешний runtime не должен мутировать authored `ProjectV2` во время playback. все изменения сцены живут в runtime scene state/overrides.
4. linked q0s не надо flatten-ить в один общий id-space во время обычной работы. каждый linked movie получает namespace/alias, иначе столкнутся asset/q0rg/instance ids.
5. frame code должен исполняться shared runtime в q0editor preview и q0player одинаково.
6. bundle export должен быть опциональным. external graph остаётся первым классом, а не урезанной legacy-веткой.
7. save `.q1s` остаётся защищённым: validate -> serialize -> parse-back -> temp рядом -> flush/sync -> reread/compare -> atomic replace.
8. `q0shell` capabilities не должны автоматически становиться filesystem-доступом у скачанного `.q0s`/будущего `.exe`. player host по умолчанию не выполняет shell actions.

## целевая модель документов q0editor

перед добавлением q0l-вкладок заменить project-only tab abstraction на document abstraction.

примерное направление:

```text
DocumentTab
  id
  kind:
    Project(ProjectWorkspace)
    Q0lang(Q0langDocument)

Q0langDocument
  path: Option<PathBuf>
  text: String
  dirty: bool
  diagnostics
  editor_state
  project_context: Option<ProjectNodeId>
```

`project_context` нужен, чтобы q0l, открытый из project tree, видел aliases, q0rg, assets, named instances и project libraries. q0l, открытый просто двойным кликом из explorer, работает standalone.

нужно обобщить:

- верхнюю tab strip;
- ctrl+tab/cycle;
- dirty close confirmation;
- save/save as;
- open from command line;
- recent files;
- drag/drop open;
- window title/taskbar title;
- shutdown dirty scan.

не делать вторую параллельную систему вкладок сбоку от `ProjectTab`, иначе dirty/save/switch логика раздвоится.

## 1. ассоциации `.q0l` и `.q0lang`

### поведение

- windows association регистрирует `.q1s`, `.q0l`, `.q0lang` отдельно, но все ведут на текущий `q0editor.exe "%1"`.
- `.q0l` и `.q0lang` используют один prog id вроде `q0editor.Q0langSource`.
- friendly type: `q0lang source`.
- registration по-прежнему сохраняет чужой default prog id и восстанавливает только если q0editor всё ещё владелец association.
- settings ui показывает состояние каждого семейства расширений.
- installer/publication позже должен иметь те же association rules, но runtime registration внутри editor остаётся рабочей запасной веткой.

### tests

- foreign defaults для обоих расширений сохраняются независимо;
- повторная registration не перетирает backup;
- unregister не забирает extension после выбора другого приложения;
- command line path с пробелами/кириллицей корректно передаётся;
- `.q0l` и `.q0lang` маршрутизируются в один open-code path.

## 2. standalone q0l/q0lang tabs

### open/save

- новый action уровня приложения: `OpenDocumentFromPath`, который по extension dispatch-ит project/code.
- `.q1s` -> существующий protected project loader.
- `.q0l|.q0lang` -> bounded utf-8 text loader.
- неизвестное расширение через open dialog не угадывать по содержимому без отдельного явного решения.
- code save делать также через staging + flush/sync + reread + atomic replace, хоть формат и простой текстовый.
- сохранять исходный extension пользователя. `foo.q0lang` не переименовывать в `.q0l` самовольно.

### ui

при активной q0l-вкладке центральная область становится полноценным code editor, а stage/timeline скрываются. project tree можно оставить видимым, если документ открыт в контексте graph.

встроенный script q0rg и frame-code не должны иметь отдельный другой редактор. они используют тот же `q0lang editor core`, но с другим document backend.

## 3. project tree

### модель

сделать сериализуемый rooted project graph.

минимальные сущности:

```text
ProjectNode
  node_id
  alias
  kind: Movie | Q0lang
  parent: Option<node_id>
  source: ExternalRelativePath | Embedded
  source_path: optional normalized relative utf-8 path
  embedded_bytes: optional bytes
```

root всегда текущий exported movie. для authoring `.q1s` root может иметь source project path только в session, но child relations должны сериализоваться.

для начала graph держать tree-семантику: один parent на node, любой уровень вложенности, cycles запрещены. если позже понадобится shared dependency, можно добавить explicit shared reference, не ломая tree ui.

### aliases/namespaces

каждый child обязан иметь уникальный alias среди siblings. alias используется runtime и не зависит от имени файла.

пример:

```text
main.q0s
  scripts/player.q0l     alias player
  ui.q0s                 alias ui
    ui_logic.q0lang      alias logic
  enemies.q0s            alias enemies
```

код должен обращаться к linked movie через alias, а не через абсолютный path.

### ui

новое toggleable окно/panel `project tree`:

- root сверху;
- q0s/q0l icons;
- текущий node выделен;
- breadcrumb/строка `parent: ...`;
- add existing q0s;
- add existing q0l/q0lang;
- create q0l;
- open node;
- rename alias;
- reparent drag/drop с cycle check;
- remove link без удаления файла;
- relink missing file;
- badge `external` / `bundled` / `missing` / `dirty`;
- tooltip с resolved path;
- контекстное `set bundle policy` можно добавить позже, но глобального export toggle достаточно для первой версии.

### paths

- хранить portable relative paths относительно файла parent/root, если это возможно;
- запрещать `..` escape при packaged runtime resolution;
- absolute paths допустимы только как authoring fallback и должны давать portability warning;
- при move/save as root делать rebase paths, не молча ломать graph.

### format/version

project graph требует новую q1s/q0s revision. старые версии parse-ятся с пустым graph.

обязательные tests:

- old q1s/q0s -> empty graph;
- current graph roundtrip;
- nested parent/child roundtrip;
- duplicate sibling alias rejected;
- cycle rejected;
- missing external file не делает сам q1s unparsable, но runtime/export выдаёт понятную dependency error;
- protected save с graph не теряет существующие assets/rig/audio/script данные.

## 4. сильно улучшенный q0lang editor

### сначала editor core, потом украшения

выделить reusable `Q0langEditorState`/`Q0langEditorView`, не привязанный к `q0rg_id`.

он должен обслуживать:

- standalone `.q0l/.q0lang` tab;
- q0rg script;
- frame script;
- позже debug source view.

### обязательный первый пакет

- нормальный gutter, синхронный со scroll;
- current-line highlight;
- selection/cursor без прыжков после auto-indent;
- editor-local undo/redo;
- ctrl+f search, f3/shift+f3;
- ctrl+h replace;
- ctrl+g go to line;
- bracket/paren matching;
- auto-close `()[]{}` и quotes с корректным skip-over;
- smart newline после `{` и перед `}`;
- multi-line indent/outdent;
- comment/uncomment selection;
- duplicate line;
- move line up/down;
- diagnostics panel с click -> cursor;
- inline diagnostic underline по span;
- status: line/column, encoding, dirty;
- minimap не нужен в первой версии.

### language-aware пакет

- lexer tokens со span `(byte_start, byte_end, line, column)`;
- parser diagnostics со span, не только line;
- autocomplete builtin libraries/symbols;
- autocomplete project aliases;
- autocomplete q0rg/assets/named instances из active project context;
- hover docs для builtin;
- signature help;
- go-to-definition для local function/module;
- document symbols outline;
- rename local symbol после появления нормального symbol table;
- snippets для `func`, `if`, listener/frame handlers;

не прибивать editor к стороннему crate заранее. сначала сделать spike: может ли текущий egui `TextEdit` дать нужное cursor/selection поведение. если нет, заменить именно text surface, сохранив parser/editor state API.

## 5. нормальный q0lang core

текущий parser line-oriented и слишком слаб для полного scene runtime. перед большим host api поднять сам язык.

### parser/runtime v1

- real block AST вместо raw body у `do!`;
- function body + call + return;
- booleans;
- comparisons `== != < <= > >=`;
- logic `and/or/not` или эквивалентный финальный синтаксис;
- `if/else`;
- bounded loops или iterator form;
- proper string escapes + utf-8 lexer;
- member/property syntax либо чёткий command-api, выбранный до scene bindings;
- deterministic runtime value model;
- per-execution instruction budget;
- recursion depth budget;
- diagnostic stack/source location.

### imports/modules

оставить builtin `import q0.math` и добавить project module imports через alias, без path literals в игровом коде.

предпочтительное направление:

```q0lang
import project.player
import project.ui
```

standalone q0l child становится module. public functions/symbols экспортируются, private остаются внутри module.

### libraries

расширить набор как минимум до:

- `q0.core` - types/helpers/functions/events;
- `q0.math` - trig, pow/log, map/remap, random helpers отдельно если nondeterministic;
- `q0.string`;
- `q0.color`;
- `q0.time` - frame, seconds, delta;
- `q0.input` - mouse + keyboard;
- `q0.timeline`;
- `q0.scene` - find/spawn/remove/visibility/transform;
- `q0.q0rg` - symbol playback/control;
- `q0.draw` - runtime vectors;
- `q0.audio` - play/stop/pause/seek/gain/pan;
- `q0.rig` - существующее api;
- `q0.project` - graph/root/current/library lookup;
- `q0.debug` - print/warn/assert/runtime inspect;
- `q0shell` - только host capability, не автоматическая player capability.

## 6. runtime control всего проекта

### стабильная адресация объектов

сейчас placement имеет stable numeric `instance_id`, но нормального user-facing runtime name нет. добавить sparse instance-name metadata, например key `(q0rg_id, instance_id) -> name`.

properties должен позволять задать runtime instance name. имена уникальны в пределах q0rg.

код получает объект по стабильному имени/id, не по placement index.

### runtime scene state

создать общий host-side слой поверх `ProjectV2`:

```text
RuntimeSceneState
  transform_overrides
  visibility_overrides
  appearance/fx overrides
  symbol_playback_state
  spawned_instances
  dynamic_vector_layers
  audio_voices
  rig_overrides
```

это состояние:

- создаётся при play/load;
- сбрасывается на stop/reset/reload по определённой политике;
- не ставит editor dirty;
- используется и editor preview, и player;
- рендерер читает authored project + runtime state.

### scene transform api

минимум:

- set/add x/y;
- rotation;
- scale x/y;
- skew;
- opacity;
- visible;
- reset override;
- remove spawned runtime object;
- spawn q0rg from current/linked library.

### q0rg playback api

каждый q0rg instance получает собственное runtime playback state:

- play/stop;
- goto frame;
- goto label;
- loop on/off;
- playback speed позже;
- current frame/readback.

не использовать один глобальный frame_index для всех nested q0rg.

### linked library spawn

нужен namespaced resolver:

```text
(root namespace)::local symbol
ui::button
characters::hero
```

runtime resolver возвращает `(project node, q0rg id)` и создаёт runtime instance без копирования child assets в authored root.

### runtime vector drawing

`q0.draw` рисует в отдельный ephemeral vector layer/runtime asset pool.

обязательные операции первой версии:

- begin path;
- move/line/curve;
- close;
- fill rgba;
- stroke rgba,width;
- clear named runtime canvas;
- transform canvas/object.

не конвертировать каждую runtime линию в authored assets и не трогать history.

### audio

`q0.audio` должен уметь брать audio asset из local или linked q0s library:

- play;
- stop;
- pause/resume;
- seek;
- gain;
- pan;
- optional loop.

editor playback и q0player должны использовать один semantic host api, даже если backend audio device различается.

## 7. frame code на таймлайне

### storage

не запихивать frame code в visual placement и не создавать фиктивный display object.

добавить отдельную sparse коллекцию в project data:

```text
FrameScript
  q0rg_id
  layer_id
  frame
  source: Inline(String) | ModuleCall(... later)
```

это позволяет ставить script marker на любом layer/frame, не ломая visual keyframes.

### ui

- selected frame -> `add/edit frame code`;
- отдельный marker на cell, например `{}`/script glyph;
- double click marker открывает общий q0lang editor;
- delete script marker не удаляет visual keyframe;
- copy/cut/paste frame range должен копировать frame scripts вместе с соответствующим timeline content;
- frame insert/delete/shift/truncate обязан сдвигать scripts так же, как audio/placements по своей семантике;
- properties может показывать `frame script: n lines` + edit button.

### execution semantics

зафиксировать до реализации:

- entry q0rg script выполняется при runtime init;
- frame 0 script выполняется при первом входе на frame 0 после init;
- при обычном playback script выполняется ровно один раз при входе playhead на его frame;
- loop last -> 0 снова выполняет frame 0 script;
- goto/seek, инициированный runtime, выполняет target-frame script;
- editor scrub по умолчанию не должен запускать irreversible external side effects; для editor preview host capabilities ограничены;
- script, который прыгает на frame со script, может породить цепочку, поэтому нужен max frame-script transitions per tick и diagnostic вместо hang;
- skipped frames при прямом jump не исполняются, исполняется только destination frame;
- несколько frame scripts на одном frame выполняются в deterministic layer order, затем stable insertion order.

### tests

- frame script fires once on crossing;
- loop refires frame 0;
- backward goto fires destination;
- skipped intermediate frames do not fire;
- two scripts same frame deterministic order;
- self-jump/loop stops by budget, player не зависает;
- editor preview and player produce same runtime actions for fixture.

## 8. optional external vs bundled export

это главная часть и фундамент будущего `.exe`.

### два режима export

`external`:

- root q0s содержит graph metadata;
- linked q0s/q0l остаются отдельными файлами;
- paths portable relative;
- q0player resolver загружает dependencies рядом/по graph paths;

`bundle`:

- рекурсивно resolve весь graph;
- validate каждый child q0s;
- parse/validate каждый q0l;
- detect cycles/duplicate aliases/missing files;
- в root q0s embed bytes всех dependency nodes;
- runtime resolver использует embedded bytes и не требует внешних файлов.

### формат

не append trailer.

предпочтительный путь: расширить новую ревизию `ProjectV2` project graph так, чтобы node source мог быть external или embedded. тогда bundled q0s остаётся обычным валидным q0s новой версии, просто graph nodes несут embedded bytes.

это проще будущего exe:

```text
exe stub
  tiny q0player/runtime
  embedded root bundled.q0s
```

не потребуется отдельный второй package format.

### bundle safety/determinism

- canonical node order;
- canonical relative internal names;
- duplicate canonical path rejected;
- size/recursion limits;
- q0l utf-8 validation;
- q0s parse validation до embedding;
- deterministic output: одинаковый input graph -> одинаковые bytes;
- не включать absolute authoring path в bundle;
- optional source hashes в manifest желательно добавить сразу или отдельной маленькой revision;
- bundle не должен менять исходный q1s/project graph сам по себе.

### bundle tests

- root + q0l -> one q0s -> player executes module without external file;
- root + child q0s -> player resolves child q0rg library;
- nested root -> child q0s -> q0l works;
- after bundling удалить исходные dependency files из temp dir, bundled q0s всё равно полностью работает;
- external export наоборот fails cleanly when dependency missing at runtime;
- same graph bundled twice byte-identical;
- unicode/space paths;
- malicious `..`/absolute packaged path rejected;
- oversized/nesting-bomb graph rejected;
- old q0player gives unsupported-version error, current q0player loads it;
- old q0s revisions продолжают открываться current player.

## 9. порядок реализации

### phase a - baseline + regression scaffolding

1. зафиксировать текущие q0lang parser/runtime/player tests.
2. fixtures для `.q0l` и `.q0lang` как identical sources.
3. fixture старого q1s/q0s для future migration checks.
4. tests на current protected save перед format bump.

### phase b - document system + file association

1. refactor `ProjectTab` -> `DocumentTab`.
2. add `Q0langDocument`.
3. generic open dispatch.
4. q0l/q0lang tab save/dirty/close.
5. windows association для двух extensions.
6. open by explorer/command line/drop.

### phase c - editor core

1. вынести reusable editor state/view.
2. search/replace/navigation/indent/comment/brackets.
3. span lexer/diagnostics.
4. autocomplete builtins.
5. standalone tabs и q0rg script переключить на один core.

### phase d - language core

1. real blocks/functions.
2. comparisons/bool/if.
3. module/public symbol model.
4. budgets + source locations.
5. project alias imports.

### phase e - project graph format + ui

1. q1s/q0s version bump.
2. graph structures + validation.
3. writer/parser + compatibility tests.
4. project tree panel.
5. add/remove/reparent/relink.
6. relative path rebase on save as.

### phase f - timeline frame scripts

1. format storage/version bump если не включено в phase e revision.
2. timeline marker/edit/copy/paste/shift/delete semantics.
3. shared runtime frame-enter dispatcher.
4. editor playback integration.
5. q0player integration.

### phase g - full scene host api

1. stable instance names.
2. shared runtime scene state.
3. transform/visibility.
4. q0rg per-instance playback.
5. linked library resolver/spawn.
6. runtime vector draw.
7. runtime audio.
8. extra libraries/debug/input/time.

### phase h - bundle export

1. recursive dependency resolver.
2. external export mode stays default/available.
3. embedded node source format.
4. deterministic bundle writer.
5. player embedded resolver.
6. standalone bundled smoke with source files removed.

### phase i - polish + future exe seam

1. project tree missing/bundle badges.
2. editor project-aware autocomplete/hover/signature help.
3. q0lang debug console/runtime inspector.
4. define tiny player bootstrap api: `Player::from_embedded_bytes(...)`.
5. не делать exe exporter пока bundled q0s не прошёл отдельный полный smoke.

## 10. обязательный compatibility/check pipeline для format phases

после каждого изменения q0s-format:

```text
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -d warnings
cargo build --release --locked -p q0editor -p q0player
```

плюс targeted checks:

- old q1s parse;
- current q1s protected save roundtrip;
- current q0s roundtrip;
- old q0s player parse;
- project graph roundtrip;
- frame scripts roundtrip;
- bundle roundtrip;
- player runtime parity fixture.

publication build только после feature completion:

```text
powershell -executionpolicy bypass -file .\installer\build.ps1
```

и считать его полным только если появились `build-info.txt` и `sha256sums.txt`.

## 11. gui smoke после реализации

не запускать gui самостоятельно без прямой команды пользователя.

короткий ручной smoke для q0s:

1. открыть `.q0l` двойным кликом -> отдельная вкладка, syntax/dirty/save работают.
2. открыть `.q0lang` -> поведение идентичное.
3. root project -> project tree -> добавить q0l и child q0s.
4. открыть child q0l из tree -> editor знает project aliases.
5. поставить frame script на frame 5 -> playback -> действие происходит именно при входе на 5.
6. frame script двигает named object x/y без изменения authored transform после stop/reset.
7. script запускает nested q0rg frame и audio.
8. script runtime-draw рисует vector shape.
9. external q0s export работает рядом с dependencies.
10. bundled export -> перенести один `.q0s` в пустую папку -> q0player воспроизводит всё без исходных q0l/child q0s.

## 12. что нельзя сделать костылём

- не открывать q0l во floating `q0lang editor window` вместо настоящей document tab.
- не хранить project tree только в settings/json рядом с проектом.
- не исполнять frame code из q0editor-only таймеров, минуя shared runtime.
- не менять authored transforms напрямую во время playback.
- не использовать placement index как runtime object identity.
- не объединять linked q0s assets по numeric id без namespace.
- не bundle-ить простым concat/trailing blob.
- не удалять external mode после появления bundle.
- не давать bundled/downloaded q0s бесконтрольный `q0shell` filesystem access.
- не считать bundle готовым, пока один bundled q0s не играет после физического удаления/перемещения всех исходных dependencies из test dir.
