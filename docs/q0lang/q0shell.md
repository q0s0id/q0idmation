# q0shell: терминальный host q0lang

`q0shell` - третья программа q0idmation-линейки после q0player и q0editor. Это
чистый терминальный host: он запускает q0lang-файлы без редактора, выполняет
чистые вычисления через общий runtime и применяет `q0shell.*!` actions к
файловой системе.

## Запуск

```powershell
cargo run -p q0shell -- script.q0lang --root .
```

`--root` задает директорию, внутри которой разрешены файловые операции. Если
`--root` не указан, используется текущая директория.

Для быстрых команд без файла есть inline-режим:

```powershell
cargo run -p q0shell -- -e "q0shell.print! 'hi'" --root .
cargo run -p q0shell -- -e "q0shell.mkdir! 'out'" -e "q0shell.write! 'out/a.txt', 'Q0'" --root .
```

Каждый `-e` / `--eval` добавляет одну строку q0lang; несколько `-e`
склеиваются в один скрипт.

## Команды

```q0lang
q0shell.print! "text", 123
q0shell.read! "path.txt"
q0shell.write! "path.txt", "text"
q0shell.append! "path.txt", "more"
q0shell.mkdir! "dir"
q0shell.list! "dir"
q0shell.move! "from.txt", "to.txt"
q0shell.delete! "path.txt"
```

Все аргументы - q0lang expressions:

```q0lang
import q0.math
import q0shell

slot = round(sin(pi / 2) * 7)
path = "out/" + "slot-" + slot + ".txt"

q0shell.mkdir! "out"
q0shell.write! path, "armed"
q0shell.read! path
```

## Безопасность v0

q0shell намеренно низкоуровневый, но не слепой:

- абсолютные пути запрещены;
- `..` запрещен;
- удаление root-директории запрещено;
- все операции резолвятся относительно `--root`.

Этого достаточно для первого слоя системного менеджмента: создать, записать,
прочитать, переместить, удалить, вывести список. Позже этот же host можно
встроить в q0editor как debug-terminal: editor будет показывать тот же stdout,
stderr, diagnostics и список host actions.
