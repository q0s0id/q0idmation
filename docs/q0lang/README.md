# q0lang: гайд

q0lang - маленький скриптовый язык для q0idmation, q0rg-символов и будущих `.q0l`
файлов. Базовая идея остается брутально-простой: чистые вычисления пишутся
как обычный код, а команда, которая прямо что-то делает во внешнем мире,
заканчивается на `!`.

Текущее состояние:

- parser и runtime лежат в `q0s-format`, поэтому `q0editor` и `q0player`
  используют одно и то же ядро языка;
- `q0editor` валидирует скрипты и показывает Script summary в q0lang-редакторе;
- `q0player` выполняет script у entry-q0rg при загрузке v2 `.q0s`;
- `gorun!` и `gostop!` уже влияют на playback при загрузке файла.
- `q0.math` уже дает базовую арифметику и функции вроде `sin`, `cos`, `sqrt`,
  `clamp`, `lerp`.
- `q0shell` запускает q0lang-файлы как низкоуровневый терминальный host и
  выполняет файловые команды `q0shell.*!`.

## Быстрый старт

```q0lang
import q0.timeline
import q0.mouse
import q0.math
import q0shell

start = 2
title = "intro"
wobble = sin(pi / 2) * 12

q0shell.print! "loaded", title
gostop! start + 1
listen mouse.click onClick

pb func onClick target
do! {
  gorun! 3
}
```

## Главные правила

Переменные объявляются стандартно:

```q0lang
frame = 12
name = "Intro"
target = frame + 2
```

Строки пишутся как строки, без декоративной пыли:

```q0lang
text = "Hello"
```

Старый вид `..."Hello"` считается legacy-синтаксисом и получает diagnostic.

Сигналы выполнения заканчиваются на `!`:

```q0lang
gorun! 4
gostop! 0
do! {
  gorun! 2
}
```

Объявления и подписки без прямого выполнения пишутся без `!`:

```q0lang
listen mouse.click onClick
xlisten mouse.click onClick
pb func openDoor actor speed
pr func cacheFrame frame
```

Математика уже работает в assignments и timeline targets:

```q0lang
import q0.math

t = 0.25
x = lerp(10, 90, t)
y = sin(t * tau) * 40
target = clamp(round(x / 10), 1, 12)
gorun! target
```

Timeline - это host-модуль `q0.timeline`, а не весь язык. q0editor только
одна из оболочек: ядро parser/runtime лежит в `q0s-format` и может жить в
player, CLI, тестах и будущих `.q0l`-инструментах.

## q0shell

`q0shell` - третья программа рядом с q0player и q0editor. Она запускает
q0lang-файл без графической среды:

```powershell
cargo run -p q0shell -- tools/build.q0lang --root .
cargo run -p q0shell -- -e "q0shell.print! 'loaded'" --root .
```

Команды начинаются с `q0shell.` и заканчиваются на `!`, потому что они реально
трогают внешний мир:

```q0lang
import q0shell
import q0.math

name = "artefact-" + round(sin(pi / 2) * 10)
q0shell.mkdir! "out"
q0shell.write! "out/report.txt", name
q0shell.append! "out/report.txt", "\nDONE"
q0shell.read! "out/report.txt"
```

По умолчанию host работает внутри текущей папки. `--root DIR` задает другой
корень. Абсолютные пути и `..` запрещены, чтобы команда была грубой, но не
слепой.

## Файлы в этой папке

- [syntax.md](syntax.md) - справочник синтаксиса.
- [runtime.md](runtime.md) - что сейчас реально исполняет runtime.
- [builtins.md](builtins.md) - встроенные библиотеки и символы.
- [q0shell.md](q0shell.md) - терминальный host и файловые команды.
- [examples/basic.q0lang](examples/basic.q0lang) - маленький пример скрипта.
