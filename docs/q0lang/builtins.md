# встроенные библиотеки

[главная](README.md) · [синтаксис](syntax.md) · [host api](hosts.md)

`import имя` принимает ровно одно известное имя библиотеки без кавычек. импорт регистрируется в runtime. он не загружает файл, не создаёт пространство имён и не выдаёт доступ к системным функциям. встроенная математика доступна и без импорта. у `import q0.input` есть дополнительный эффект в q0player: включение независимых игровых тактов.

## полный список библиотек

| библиотека | символы |
| --- | --- |
| `q0.core` | `do!`, `listen`, `xlisten`, `pb`, `pr`, `func`, `if`, `else`, `while`, `for`, `in`, `range`, `break`, `continue`, `return`, `true`, `false`, `null`, `and`, `or`, `not` |
| `q0.math` | `pi`, `tau`, `sin`, `cos`, `tan`, `sqrt`, `abs`, `min`, `max`, `clamp`, `lerp`, `floor`, `ceil`, `round` |
| `q0.timeline` | `gorun!`, `gostop!`, `frame`, `label` |
| `q0.mouse` | `mouse.down`, `mouse.up`, `mouse.move`, `mouse.click`, `mouse.double`, `mouse.wheel`, `mouse.over`, `mouse.out` |
| `q0.input` | `input.left`, `input.right`, `input.jump`, `input.jump_pressed` |
| `q0.scene` | `stage.width`, `stage.height`, `player.x`, `player.y`, `player.left`, `player.right`, `player.top`, `player.bottom`, `player.width`, `player.height`, `q0scene.switch!` |
| `q0.time` | `time.dt` |
| `q0.rig` | `q0rig.position!`, `q0rig.value!`, `q0rig.reset!`, `q0rig.pose!`, `q0rig.pose_reset!` |
| `q0shell` | `q0shell.print!`, `q0shell.read!`, `q0shell.write!`, `q0shell.append!`, `q0shell.mkdir!`, `q0shell.list!`, `q0shell.move!`, `q0shell.delete!` |

это список распознаваемых библиотек, а не обещание реализации каждого события или свойства в каждом host. `frame` и `label` пока являются именами из каталога, без автоматического значения. `player` обозначает пример имени экземпляра, которое нужно назначить в проекте. [таблица доступности](hosts.md) объясняет различия.

## q0.math

все функции принимают ровно указанное число аргументов типа number и возвращают number. числовые строки и логические значения не приводятся к числам. ошибочное число или тип аргументов даёт диагностику. аргументы вычисляются слева направо.

| запись | действие |
| --- | --- |
| `pi` | число π, примерно 3.141592653589793 |
| `tau` | число 2π, примерно 6.283185307179586 |
| `sin(x)` | синус; x в радианах |
| `cos(x)` | косинус; x в радианах |
| `tan(x)` | тангенс; x в радианах |
| `sqrt(x)` | квадратный корень; для отрицательного x результат nan |
| `abs(x)` | абсолютное значение |
| `floor(x)` | округление вниз, например floor(-1.2) = -2 |
| `ceil(x)` | округление вверх, например ceil(-1.2) = -1 |
| `round(x)` | ближайшее целое; половины округляются от нуля |
| `min(a, b)` | меньшее из двух чисел |
| `max(a, b)` | большее из двух чисел |
| `clamp(x, lo, hi)` | ограничение x интервалом; lo ≤ hi, границы не nan |
| `lerp(a, b, t)` | a + (b - a) * t; t автоматически не ограничивается |

`pi` и `tau` разрешаются как константы, если нет общей переменной с таким именем. присваивание может их затенить. встроенные имена функций затенить пользовательскими вызовами нельзя.

```q0lang
import q0.math
angle = 90 * pi / 180
wave = sin(angle)
position = lerp(10, 90, 0.25)
limited = clamp(position, 0, 20)
```

`wave` приблизительно `1`, `position` равен `30`, `limited` равен `20`. специальных генераторов случайных чисел, преобразований `str()` / `number()` и строковых функций пока нет. строку можно собрать через `+`.

## команды

команды являются инструкциями и не возвращают значение в выражение. после имени с `!` ставится пробел, аргументы разделяются запятыми. не пиши `q0rig.pose!(...)` и не присваивай результат команды переменной.

| команда | аргументы | host |
| --- | --- | --- |
| `gorun! target` | номер кадра или строка-цель | editor preview, player |
| `gostop! target` | номер кадра или строка-цель | editor preview, player |
| `q0scene.switch! alias` | строковое имя bundled-сцены | player |
| `q0rig.position! control, x, y` | имя, два числа | player |
| `q0rig.value! control, value` | имя, число | player |
| `q0rig.reset! control` | имя | player |
| `q0rig.pose! pose, weight` | имя, конечное число; ограничивается 0…1 | player |
| `q0rig.pose_reset! pose` | имя | player |
| `q0shell.print! value, ...` | от нуля до 255 значений | shell |
| `q0shell.read! path` | путь | shell |
| `q0shell.write! path, text` | путь, значение для записи | shell |
| `q0shell.append! path, text` | путь, значение для добавления | shell |
| `q0shell.mkdir! path` | путь | shell |
| `q0shell.list!` или `q0shell.list! path` | ноль или один путь | shell |
| `q0shell.move! from, to` | два пути | shell |
| `q0shell.delete! path` | путь | shell |

подробная семантика кадров, имён и ригов находится в [host api](hosts.md); файловые операции, преобразование значений и ограничения путей в [q0shell](q0shell.md). ключевые слова описаны в [синтаксисе](syntax.md), события в [справочнике событий](events.md).

источники: [каталог parser](../../q0s-format/src/q0lang/parser.rs), [вычисления qvm](../../q0vm/src/lib.rs), [преобразование команд](../../q0s-format/src/q0lang/runtime.rs).
