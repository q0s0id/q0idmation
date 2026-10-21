# q0lang overhaul + project graph + bundled q0s

> Design history. This is the plan written before the overhaul; much of it has since been built and some of it changed on the way. The [reference](README.md) describes the current language.

## goal

Make q0lang a full part of q0editor and q0player, not a separate mini-script inside `q0rg.script`.

The main runtime path after this work:

`root .q1s -> project graph -> q0l/q0lang + linked .q0s -> export external/bundled .q0s -> q0player -> deterministic runtime`

The future path for standalone applications should follow without reworking the format:

`bundled .q0s + tiny q0player stub -> .exe`

## what actually exists right now

- The q0lang parser and runtime already live in `q0s-format/src/q0lang`, so the editor, player and shell can share one core.
- `q0shell` already accepts both `.q0lang` and `.q0l`. These must stay two fully equal extensions of one plain UTF-8 format, like `.jpg`/`.jpeg`.
- q0editor currently associates only `.q1s`, through `q0editor/src/assoc.rs`.
- q0editor's top tabs are hard-wired as `ProjectTab`, holding only a `ProjectWorkspace`.
- There is no standalone q0l document in q0editor.
- The current q0lang editor is a separate floating window and edits only `Q0rg.script`.
- The current editor is built on egui `TextEdit`: it has syntax highlighting, a gutter, a font picker, basic auto-indent/tab/outdent and diagnostics, but no proper document model, search/replace, autocomplete, spans, symbols, inline diagnostics or code navigation.
- `Q0rg` stores a single shared `script: String`.
- When q0player loads a v2 `.q0s`, it takes only the entry q0rg's script and runs it once.
- There are no frame scripts on the timeline.
- The runtime mostly emits host actions: timeline, q0shell, q0rig. That is a good architectural extension point and must be kept.
- q0player already applies q0rig runtime overrides without changing the authored project. The same principle is needed for transform/drawing/spawn/audio.
- `ProjectV2` has no project graph or dependencies yet.
- The current `.q0s` parser rejects trailing bytes, so a bundle cannot simply be appended to an old q0s. Bundling must be a proper new format revision.
- Current versions: q1s current = 20, q0s current = 19. New fields must come with new versions, backward parse defaults and roundtrip/migration tests.

## architectural rules

1. `.q0l` and `.q0lang` do not get different magic/versions. It is one text format; only the extension differs.
2. The project tree must not live only in UI/session state. Links must be serialised and reach the exported q0s.
3. The runtime must never mutate the authored `ProjectV2` during playback. All scene changes live in runtime scene state/overrides.
4. Linked q0s must not be flattened into one shared id space during normal work. Every linked movie gets a namespace/alias, otherwise asset/q0rg/instance ids collide.
5. Frame code must run in the shared runtime, the same way in the q0editor preview and in q0player.
6. Bundle export must be optional. The external graph stays first-class, not a cut-down legacy branch.
7. Saving `.q1s` stays protected: validate -> serialise -> parse back -> temp file alongside -> flush/sync -> reread/compare -> atomic replace.
8. `q0shell` capabilities must not automatically become file-system access for a downloaded `.q0s` or a future `.exe`. The player host does not run shell actions by default.

## target document model for q0editor

Before adding q0l tabs, replace the project-only tab abstraction with a document abstraction.

Rough direction:

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

`project_context` lets a q0l opened from the project tree see aliases, q0rgs, assets, named instances and project libraries. A q0l opened with a plain double-click in Explorer works standalone.

To generalise:

- the top tab strip;
- ctrl+tab / cycling;
- the dirty close confirmation;
- save / save as;
- opening from the command line;
- recent files;
- drag-and-drop opening;
- the window and taskbar title;
- the dirty scan on shutdown.

Do not build a second, parallel tab system beside `ProjectTab`; the dirty/save/switch logic would split in two.

## 1. `.q0l` and `.q0lang` associations

### behaviour

- The Windows association registers `.q1s`, `.q0l` and `.q0lang` separately, all pointing at the current `q0editor.exe "%1"`.
- `.q0l` and `.q0lang` share one prog id, such as `q0editor.Q0langSource`.
- Friendly type: `q0lang source`.
- Registration still keeps a foreign default prog id and restores it only while q0editor still owns the association.
- The settings UI shows the state of each extension family.
- The installer/publication should later follow the same association rules, but runtime registration inside the editor stays a working fallback.

### tests

- foreign defaults for both extensions are kept independently;
- registering again does not overwrite the backup;
- unregistering does not take the extension back after the user chose another application;
- a command-line path with spaces or non-Latin characters arrives intact;
- `.q0l` and `.q0lang` route into one open-code path.

## 2. standalone q0l/q0lang tabs

### open/save

- A new application-level action, `OpenDocumentFromPath`, dispatches to project or code by extension.
- `.q1s` -> the existing protected project loader.
- `.q0l|.q0lang` -> a bounded UTF-8 text loader.
- Do not guess an unknown extension from the open dialog by its contents without a separate explicit decision.
- Save code through staging + flush/sync + reread + atomic replace too, even though the format is plain text.
- Keep the user's extension. Never rename `foo.q0lang` to `.q0l` on your own.

### ui

With a q0l tab active, the central area becomes a full code editor and the stage/timeline hide. The project tree may stay visible when the document was opened in the context of the graph.

The q0rg's own script and frame code must not get a different editor. They use the same `q0lang editor core` with a different document backend.

## 3. project tree

### model

Make a serialisable rooted project graph.

Minimal entities:

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

The root is always the current exported movie. For an authoring `.q1s`, the root may know its source project path only in the session, but child relations must be serialised.

To start, keep tree semantics: one parent per node, any depth, no cycles. If a shared dependency is needed later, an explicit shared reference can be added without breaking the tree UI.

### aliases/namespaces

Every child must have an alias unique among its siblings. The runtime uses the alias, independent of the file name.

Example:

```text
main.q0s
  scripts/player.q0l     alias player
  ui.q0s                 alias ui
    ui_logic.q0lang      alias logic
  enemies.q0s            alias enemies
```

Code must reach a linked movie through its alias, not through an absolute path.

### ui

A new toggleable `project tree` window/panel:

- the root on top;
- q0s/q0l icons;
- the current node highlighted;
- a breadcrumb / `parent: ...` line;
- add existing q0s;
- add existing q0l/q0lang;
- create q0l;
- open node;
- rename alias;
- reparent by drag and drop, with a cycle check;
- remove a link without deleting the file;
- relink a missing file;
- `external` / `bundled` / `missing` / `dirty` badges;
- a tooltip with the resolved path;
- a context `set bundle policy` can come later; a global export toggle is enough for the first version.

### paths

- Store portable paths relative to the parent/root file where possible.
- Refuse `..` escapes in packaged runtime resolution.
- Absolute paths are allowed only as an authoring fallback and must give a portability warning.
- On move / save-as of the root, rebase the paths instead of silently breaking the graph.

### format/version

The project graph needs a new q1s/q0s revision. Old versions parse with an empty graph.

Required tests:

- old q1s/q0s -> empty graph;
- current graph roundtrip;
- nested parent/child roundtrip;
- duplicate sibling alias rejected;
- cycle rejected;
- a missing external file does not make the q1s itself unparsable, but runtime/export gives a clear dependency error;
- a protected save with a graph loses none of the existing asset/rig/audio/script data.

## 4. a much better q0lang editor

### editor core first, decoration later

Extract a reusable `Q0langEditorState`/`Q0langEditorView` that is not tied to a `q0rg_id`.

It must serve:

- the standalone `.q0l/.q0lang` tab;
- the q0rg script;
- frame scripts;
- later, a debug source view.

### required first package

- a proper gutter that scrolls in sync;
- current-line highlight;
- selection/cursor that does not jump after auto-indent;
- editor-local undo/redo;
- ctrl+f search, f3/shift+f3;
- ctrl+h replace;
- ctrl+g go to line;
- bracket/paren matching;
- auto-closing `()[]{}` and quotes, with correct skip-over;
- a smart newline after `{` and before `}`;
- multi-line indent/outdent;
- comment/uncomment selection;
- duplicate line;
- move line up/down;
- a diagnostics panel where clicking moves the cursor;
- inline diagnostic underlines by span;
- status: line/column, encoding, dirty;
- no minimap in the first version.

### language-aware package

- lexer tokens with spans `(byte_start, byte_end, line, column)`;
- parser diagnostics with spans, not only lines;
- autocomplete for built-in libraries/symbols;
- autocomplete for project aliases;
- autocomplete for q0rgs/assets/named instances from the active project context;
- hover docs for built-ins;
- signature help;
- go-to-definition for a local function/module;
- a document symbols outline;
- rename a local symbol once a proper symbol table exists;
- snippets for `func`, `if`, listener/frame handlers.

Do not tie the editor to a third-party crate up front. First do a spike: can the current egui `TextEdit` give the cursor/selection behaviour we need? If not, replace exactly the text surface and keep the parser/editor state API.

## 5. a proper q0lang core

The current parser is line-oriented and too weak for a full scene runtime. Raise the language itself before the big host api.

### parser/runtime v1

- a real block AST instead of the raw body of `do!`;
- function body + call + return;
- booleans;
- comparisons `== != < <= > >=`;
- logic `and/or/not` or an equivalent final syntax;
- `if/else`;
- bounded loops or an iterator form;
- proper string escapes + a UTF-8 lexer;
- member/property syntax, or a clear command api, chosen before scene bindings;
- a deterministic runtime value model;
- a per-execution instruction budget;
- a recursion depth budget;
- diagnostic stack/source locations.

### imports/modules

Keep the built-in `import q0.math` and add project module imports through aliases, with no path literals in game code.

Preferred direction:

```q0lang
import project.player
import project.ui
```

A standalone q0l child becomes a module. Public functions/symbols are exported; private ones stay inside the module.

### libraries

Extend the set to at least:

- `q0.core` - types/helpers/functions/events;
- `q0.math` - trig, pow/log, map/remap, random helpers kept separate if nondeterministic;
- `q0.string`;
- `q0.color`;
- `q0.time` - frame, seconds, delta;
- `q0.input` - mouse + keyboard;
- `q0.timeline`;
- `q0.scene` - find/spawn/remove/visibility/transform;
- `q0.q0rg` - symbol playback/control;
- `q0.draw` - runtime vectors;
- `q0.audio` - play/stop/pause/seek/gain/pan;
- `q0.rig` - the existing api;
- `q0.project` - graph/root/current/library lookup;
- `q0.debug` - print/warn/assert/runtime inspect;
- `q0shell` - only a host capability, never automatically a player capability.

## 6. runtime control of the whole project

### stable object addressing

A placement already has a stable numeric `instance_id`, but no proper user-facing runtime name. Add sparse instance-name metadata, keyed `(q0rg_id, instance_id) -> name`.

Properties must let the user set a runtime instance name. Names are unique within a q0rg.

Code reaches an object by its stable name/id, never by placement index.

### runtime scene state

Create a shared host-side layer on top of `ProjectV2`:

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

This state:

- is created on play/load;
- is reset on stop/reset/reload by a defined policy;
- never marks the editor dirty;
- is used by both the editor preview and the player;
- the renderer reads the authored project + the runtime state.

### scene transform api

At least:

- set/add x/y;
- rotation;
- scale x/y;
- skew;
- opacity;
- visible;
- reset override;
- remove a spawned runtime object;
- spawn a q0rg from the current/linked library.

### q0rg playback api

Every q0rg instance gets its own runtime playback state:

- play/stop;
- goto frame;
- goto label;
- loop on/off;
- playback speed later;
- current frame / readback.

Do not use one global frame_index for all nested q0rgs.

### linked library spawn

A namespaced resolver is needed:

```text
(root namespace)::local symbol
ui::button
characters::hero
```

The runtime resolver returns `(project node, q0rg id)` and creates a runtime instance without copying the child's assets into the authored root.

### runtime vector drawing

`q0.draw` draws into a separate ephemeral vector layer / runtime asset pool.

Required operations for the first version:

- begin path;
- move/line/curve;
- close;
- fill rgba;
- stroke rgba, width;
- clear a named runtime canvas;
- transform the canvas/object.

Do not turn every runtime line into authored assets and do not touch history.

### audio

`q0.audio` must be able to take an audio asset from the local or a linked q0s library:

- play;
- stop;
- pause/resume;
- seek;
- gain;
- pan;
- optional loop.

Editor playback and q0player must use one semantic host api, even if the audio device backend differs.

## 7. frame code on the timeline

### storage

Do not stuff frame code into a visual placement and do not create a fake display object.

Add a separate sparse collection to the project data:

```text
FrameScript
  q0rg_id
  layer_id
  frame
  source: Inline(String) | ModuleCall(... later)
```

That allows a script marker on any layer/frame without breaking visual keyframes.

### ui

- selected frame -> `add/edit frame code`;
- a separate marker on the cell, such as `{}` or a script glyph;
- double-clicking the marker opens the shared q0lang editor;
- deleting the script marker does not delete the visual keyframe;
- copy/cut/paste of a frame range must copy frame scripts together with the matching timeline content;
- frame insert/delete/shift/truncate must move scripts the same way audio/placements move under their rules;
- properties can show `frame script: n lines` + an edit button.

### execution semantics

Settle before implementing:

- the entry q0rg script runs at runtime init;
- the frame 0 script runs on the first entry into frame 0 after init;
- in normal playback a script runs exactly once when the playhead enters its frame;
- looping last -> 0 runs the frame 0 script again;
- a runtime-initiated goto/seek runs the target frame's script;
- editor scrubbing must not trigger irreversible external side effects by default; host capabilities are limited in the editor preview;
- a script that jumps to a frame with a script can start a chain, so a maximum number of frame-script transitions per tick is needed, with a diagnostic instead of a hang;
- frames skipped by a direct jump do not run; only the destination frame does;
- several frame scripts on one frame run in deterministic layer order, then stable insertion order.

### tests

- a frame script fires once on crossing;
- a loop refires frame 0;
- a backward goto fires the destination;
- skipped intermediate frames do not fire;
- two scripts on the same frame run in deterministic order;
- a self-jump/loop stops by budget and the player does not hang;
- the editor preview and the player produce the same runtime actions for a fixture.

## 8. optional external vs bundled export

This is the main part and the foundation of the future `.exe`.

### two export modes

`external`:

- the root q0s holds the graph metadata;
- linked q0s/q0l stay separate files;
- paths are portable relative;
- the q0player resolver loads dependencies alongside / by graph paths;

`bundle`:

- resolve the whole graph recursively;
- validate every child q0s;
- parse/validate every q0l;
- detect cycles / duplicate aliases / missing files;
- embed the bytes of every dependency node in the root q0s;
- the runtime resolver uses the embedded bytes and needs no outside files.

### format

Do not append a trailer.

Preferred path: extend a new revision of the `ProjectV2` project graph so that a node's source can be external or embedded. A bundled q0s then stays an ordinary valid q0s of the new version; its graph nodes just carry embedded bytes.

That is simpler for the future exe:

```text
exe stub
  tiny q0player/runtime
  embedded root bundled.q0s
```

No second package format will be needed.

### bundle safety/determinism

- canonical node order;
- canonical relative internal names;
- duplicate canonical paths rejected;
- size/recursion limits;
- q0l UTF-8 validation;
- q0s parse validation before embedding;
- deterministic output: the same input graph -> the same bytes;
- no absolute authoring paths in the bundle;
- optional source hashes in the manifest, ideally right away or in a small separate revision;
- bundling must not change the source q1s/project graph by itself.

### bundle tests

- root + q0l -> one q0s -> the player runs the module without the outside file;
- root + child q0s -> the player resolves the child q0rg library;
- nested root -> child q0s -> q0l works;
- after bundling, delete the source dependency files from the temp dir; the bundled q0s still works fully;
- external export, by contrast, fails cleanly when a dependency is missing at runtime;
- the same graph bundled twice is byte-identical;
- unicode/space paths;
- malicious `..`/absolute packaged paths rejected;
- oversized / nesting-bomb graphs rejected;
- an old q0player gives an unsupported-version error, the current q0player loads it;
- old q0s revisions keep opening in the current player.

## 9. implementation order

### phase a - baseline + regression scaffolding

1. Pin down the current q0lang parser/runtime/player tests.
2. Fixtures for `.q0l` and `.q0lang` as identical sources.
3. A fixture of an old q1s/q0s for future migration checks.
4. Tests for the current protected save before the format bump.

### phase b - document system + file association

1. Refactor `ProjectTab` -> `DocumentTab`.
2. Add `Q0langDocument`.
3. Generic open dispatch.
4. q0l/q0lang tab save/dirty/close.
5. Windows association for both extensions.
6. Open from Explorer / command line / drop.

### phase c - editor core

1. Extract the reusable editor state/view.
2. Search/replace/navigation/indent/comment/brackets.
3. Span lexer/diagnostics.
4. Autocomplete for built-ins.
5. Switch standalone tabs and the q0rg script to one core.

### phase d - language core

1. Real blocks/functions.
2. Comparisons/bool/if.
3. Module/public symbol model.
4. Budgets + source locations.
5. Project alias imports.

### phase e - project graph format + ui

1. q1s/q0s version bump.
2. Graph structures + validation.
3. Writer/parser + compatibility tests.
4. Project tree panel.
5. Add/remove/reparent/relink.
6. Relative path rebase on save-as.

### phase f - timeline frame scripts

1. Format storage/version bump if not part of the phase e revision.
2. Timeline marker/edit/copy/paste/shift/delete semantics.
3. Shared runtime frame-enter dispatcher.
4. Editor playback integration.
5. q0player integration.

### phase g - full scene host api

1. Stable instance names.
2. Shared runtime scene state.
3. Transform/visibility.
4. Per-instance q0rg playback.
5. Linked library resolver/spawn.
6. Runtime vector drawing.
7. Runtime audio.
8. Extra libraries/debug/input/time.

### phase h - bundle export

1. Recursive dependency resolver.
2. External export mode stays the default and available.
3. Embedded node source format.
4. Deterministic bundle writer.
5. Player embedded resolver.
6. Standalone bundled smoke test with the source files removed.

### phase i - polish + future exe seam

1. Project tree missing/bundle badges.
2. Project-aware autocomplete/hover/signature help in the editor.
3. q0lang debug console / runtime inspector.
4. Define a tiny player bootstrap api: `Player::from_embedded_bytes(...)`.
5. Do not build an exe exporter until a bundled q0s has passed a separate full smoke test.

## 10. required compatibility/check pipeline for format phases

After every q0s-format change:

```text
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -d warnings
cargo build --release --locked -p q0editor -p q0player
```

Plus targeted checks:

- old q1s parse;
- current q1s protected save roundtrip;
- current q0s roundtrip;
- old q0s player parse;
- project graph roundtrip;
- frame scripts roundtrip;
- bundle roundtrip;
- player runtime parity fixture.

The publication build only after the feature is complete:

```text
powershell -executionpolicy bypass -file .\installer\build.ps1
```

and count it complete only once `build-info.txt` and `sha256sums.txt` appear.

## 11. gui smoke test after implementation

Do not start the GUI on your own without a direct instruction from the user.

A short manual smoke test for q0s:

1. Open a `.q0l` by double-clicking -> a separate tab; syntax/dirty/save work.
2. Open a `.q0lang` -> identical behaviour.
3. Root project -> project tree -> add a q0l and a child q0s.
4. Open the child q0l from the tree -> the editor knows the project aliases.
5. Put a frame script on frame 5 -> playback -> the action happens exactly on entering 5.
6. A frame script moves a named object's x/y without changing the authored transform after stop/reset.
7. A script starts a nested q0rg frame and audio.
8. A script draws a vector shape at runtime.
9. External q0s export works next to its dependencies.
10. Bundled export -> move the single `.q0s` to an empty folder -> q0player plays everything without the source q0l/child q0s.

## 12. what must not be done as a hack

- Do not open q0l in the floating `q0lang editor window` instead of a real document tab.
- Do not keep the project tree only in settings/JSON next to the project.
- Do not run frame code from q0editor-only timers, bypassing the shared runtime.
- Do not change authored transforms directly during playback.
- Do not use the placement index as the runtime object identity.
- Do not merge linked q0s assets by numeric id without a namespace.
- Do not bundle by plain concatenation / a trailing blob.
- Do not remove external mode once bundling exists.
- Do not give a bundled/downloaded q0s uncontrolled `q0shell` file-system access.
- Do not consider bundling done until a single bundled q0s plays after all its source dependencies have been physically deleted or moved out of the test dir.
