# q0lang: built-in libraries

q0lang уже имеет маленький список встроенных библиотек. Imports используются
для validation и будущей организации языка.

## q0shell

Terminal host symbols:

```text
q0shell.print!
q0shell.read!
q0shell.write!
q0shell.append!
q0shell.mkdir!
q0shell.list!
q0shell.move!
q0shell.delete!
```

Пример:

```q0lang
import q0shell

q0shell.mkdir! "out"
q0shell.write! "out/a.txt", "Q0"
q0shell.append! "out/a.txt", "S"
q0shell.read! "out/a.txt"
q0shell.move! "out/a.txt", "out/done.txt"
q0shell.list! "out"
q0shell.delete! "out/done.txt"
```

Текущее состояние: parser/runtime отдают `ShellCommand` actions, `q0shell`
применяет их к файловой системе внутри root-директории. q0player игнорирует
эти actions, а q0editor показывает их в Script summary.

## q0.math

Math symbols:

```text
pi
tau
sin
cos
tan
sqrt
abs
min
max
clamp
lerp
floor
ceil
round
```

Пример:

```q0lang
import q0.math

t = 0.25
x = lerp(10, 90, t)
y = sin(t * tau) * 40
frame = clamp(round(x / 10), 1, 12)
```

Это ядро не зависит от q0editor и не требует timeline. Его можно гонять в
script summary, player, тестах и будущих CLI-инструментах.

## q0.mouse

Mouse event symbols:

```text
mouse.down
mouse.up
mouse.move
mouse.click
mouse.double
mouse.wheel
mouse.over
mouse.out
```

Пример:

```q0lang
import q0.mouse

listen mouse.click onClick
xlisten mouse.click onClick
```

Текущее состояние runtime: listener tables работают. Event dispatch пока не
подключен к q0player.

## q0.timeline

Timeline symbols:

```text
gorun!
gostop!
frame
label
```

Пример:

```q0lang
import q0.timeline

start = 1
gorun! start + 2
gostop! 4
```

Текущее состояние q0player: `gorun!` и `gostop!` выполняются из entry q0rg
script при загрузке v2 `.q0s`.

## q0.core

Core symbols:

```text
do!
listen
xlisten
pb
pr
func
```

Пример:

```q0lang
import q0.core

pb func onStart target
do! {
  target = 2
  gostop! target
}
```
