# q0lang: синтаксис

Это описание текущего реализованного синтаксиса. Будущие возможности можно
расширять поверх него, но именно эту форму parser понимает сейчас.

## Комментарии

Комментарий начинается с `#` и идет до конца строки.

```q0lang
# комментарий
frame = 1 # комментарий после кода
```

## Импорты

```q0lang
import q0.mouse
import q0.timeline
import q0.core
import q0.math
import q0shell
```

Parser принимает import как statement и выдает diagnostic, если имя библиотеки
неизвестно.

## Переменные

```q0lang
name = value
```

Левая часть должна быть identifier:

```q0lang
frame = 2
intro = "Intro"
target = frame + 2
```

Поддерживаемые значения:

- число: `12`, `12.5`;
- identifier: `frame`;
- строка: `"text"`;
- выражение: `sin(pi / 2) + 3 * -2`;
- raw fallback: сохраняется, если значение пока необычное для parser.

## Строки

Строки q0lang пишутся нормально:

```q0lang
message = "Press Start"
quick = 'PowerShell-friendly'
```

Legacy-форма `..."text"` больше не канон. Parser пока принимает ее с diagnostic,
чтобы старые скрипты было проще мигрировать.

## Выражения и математика

Assignments и timeline targets принимают выражения:

```q0lang
import q0.math

x = sin(pi / 2)
y = clamp(lerp(0, 10, 0.75), 2, 6)
frame = round(y) + 1
gorun! frame
```

Поддерживаются `+`, `-`, `*`, `/`, `%`, `^`, unary `-`, скобки и вызовы функций.
`^` правоассоциативный: `2 ^ 3 ^ 2` читается как `2 ^ (3 ^ 2)`.

## Timeline-сигналы

Timeline-сигналы выполняют действие, поэтому обязаны заканчиваться на `!`.

```q0lang
gorun! 1
gostop! 0
```

`gorun! X` - аналог `gotoAndPlay(X)`.

`gostop! X` - аналог `gotoAndStop(X)`.

Target может быть числом или переменной:

```q0lang
start = 3
gorun! start + 2
```

Labels уже представлены в runtime как label targets, но q0player пока умеет
разрешать только числовые targets и numeric label strings вроде `"3"`.

## q0shell-команды

`q0shell.*!` - host-команды для терминального исполнения q0lang. Они не
исполняются самим parser/runtime; runtime только выдает action, а программа
`q0shell` применяет его к системе.

```q0lang
import q0shell

q0shell.print! "boot"
q0shell.mkdir! "out"
q0shell.write! "out/a.txt", "Q0"
q0shell.append! "out/a.txt", "S"
q0shell.read! "out/a.txt"
q0shell.move! "out/a.txt", "out/done.txt"
q0shell.list! "out"
q0shell.delete! "out/done.txt"
```

Аргументы команд - обычные выражения, поэтому можно собирать пути и текст из
математики, строк и переменных.

## Слушатели событий

```q0lang
listen mouse.click onClick
xlisten mouse.click onClick
```

`listen` регистрирует handler для event.

`xlisten` удаляет handler для event.

Повторная регистрация одного и того же handler не дублируется.

## Функции

```q0lang
pb func openDoor actor speed
pr func cacheFrame frame
```

`pb func` объявляет public function signature.

`pr func` объявляет private function signature.

Текущее состояние runtime: signatures регистрируются, но тела функций и вызовы
функций еще не реализованы.

## do! блоки

```q0lang
do! {
  frame = 4
  gorun! frame
}
```

`do!` выполняет statements внутри блока. Открывающая `{` должна быть на строке
`do!`. Закрывающая `}` должна быть одна на своей строке.

Вложенные `do!` поддерживаются с runtime-лимитом глубины.

## Мини-грамматика

```text
program         = statement*
statement       = import
                | assignment
                | timeline_signal
                | shell_command
                | listener
                | function_decl
                | do_block

import          = "import" library_name
assignment      = identifier "=" value
timeline_signal = ("gorun!" | "gostop!") value
shell_command   = "q0shell." identifier "!" value_list?
listener        = ("listen" | "xlisten") event_name handler_name
function_decl   = ("pb" | "pr") "func" identifier identifier*
do_block        = "do!" "{" statement* "}"
value_list      = value ("," value)*
value           = expression | raw
expression      = call | binary | unary | number | identifier | q0_string
q0_string       = "\"" text "\"" | "'" text "'"
```
