# built-in libraries

[home](README.md) · [syntax](syntax.md) · [host api](hosts.md)

`import name` takes exactly one known library name, without quotes. The import is recorded in the runtime. It loads no file, creates no namespace and grants no access to the system. The math functions work without an import. A few imports change how q0player runs the script: `import q0.input` switches on a game clock that ticks while the timeline is paused, and `q0.object`, `q0.input` or `q0.mouse` in an [object script](hosts.md) make its frame code run every tick.

## all libraries

| library | symbols |
| --- | --- |
| `q0.core` | `do!`, `listen`, `xlisten`, `pb`, `pr`, `func`, `if`, `else`, `while`, `for`, `in`, `range`, `break`, `continue`, `return`, `true`, `false`, `null`, `and`, `or`, `not` |
| `q0.math` | `pi`, `tau`, `sin`, `cos`, `tan`, `sqrt`, `abs`, `min`, `max`, `clamp`, `lerp`, `floor`, `ceil`, `round` |
| `q0.timeline` | `gorun!`, `gostop!`, `frame`, `label` |
| `q0.object` | `self.x`, `self.y`, `self.frame`, `self.playing`, `self.name`, `self.id`, `self.width`, `self.height`, `self.left`, `self.right`, `self.top`, `self.bottom`, `self.hovered`, `self.mouse_x`, `self.mouse_y` |
| `q0.mouse` | `mouse.down`, `mouse.up`, `mouse.move`, `mouse.click`, `mouse.double`, `mouse.wheel`, `mouse.over`, `mouse.out` |
| `q0.input` | `input.left`, `input.right`, `input.jump`, `input.jump_pressed`, `input.up`, `input.down`, `input.action`, `input.action_pressed` |
| `q0.scene` | `stage.width`, `stage.height`, `player.x`, `player.y`, `player.left`, `player.right`, `player.top`, `player.bottom`, `player.width`, `player.height`, `q0scene.switch!` |
| `q0.time` | `time.dt`, `time.tick` |
| `q0.rig` | `q0rig.position!`, `q0rig.value!`, `q0rig.reset!`, `q0rig.pose!`, `q0rig.pose_reset!` |
| `q0.physics` | `q0physics.start!`, `q0physics.pause!`, `q0physics.reset!`, `q0physics.push!`, `q0physics.velocity!`, `q0physics.spin!`, `q0physics.teleport!`, `q0physics.gravity!`, `q0physics.cut!` |
| `q0.particles` | `q0particles.start!`, `q0particles.stop!`, `q0particles.burst!`, `q0particles.move!`, `q0particles.clear!` |
| `q0.light` | `q0light.on!`, `q0light.off!`, `q0light.move!`, `q0light.color!`, `q0light.brightness!`, `q0light.size!`, `q0light.darkness!` |
| `q0.layer` | `q0layer.hide!`, `q0layer.show!`, `q0layer.move!`, `q0layer.alpha!`, `q0layer.reset!`, `q0layer.solid!`, `q0layer.unsolid!`, `q0layer.touches`, `q0layer.which`, `q0layer.hit`, `q0layer.circle`, `q0layer.box`, `q0layer.visible`, `q0layer.count` |
| `q0.window` | `q0window.fullscreen!`, `q0window.ui!`, `q0window.hotkeys!`, `q0window.move!`, `q0window.size!`, `q0window.center!`, `q0window.title!`, `q0window.top!`, `q0window.minimize!`, `q0window.perf!`, `q0window.quit!`, `window.x`, `window.y`, `window.width`, `window.height`, `window.fullscreen`, `window.ui`, `screen.width`, `screen.height` |
| `q0shell` | `q0shell.print!`, `q0shell.read!`, `q0shell.write!`, `q0shell.append!`, `q0shell.mkdir!`, `q0shell.list!`, `q0shell.move!`, `q0shell.delete!` |

This is the list of libraries the parser recognises, not a promise that every host implements every symbol. `frame` holds the current frame only inside object scripts; `label` has no value yet. `player` stands for an instance name you give in the project. The [availability table](hosts.md) explains which program supports what.

## q0.math

Every function takes exactly the listed number of number arguments and returns a number. Numeric strings and booleans are not converted. A wrong argument count or type gives a diagnostic. Arguments are evaluated left to right.

| written as | does |
| --- | --- |
| `pi` | π, about 3.141592653589793 |
| `tau` | 2π, about 6.283185307179586 |
| `sin(x)` | sine; x in radians |
| `cos(x)` | cosine; x in radians |
| `tan(x)` | tangent; x in radians |
| `sqrt(x)` | square root; negative x gives nan |
| `abs(x)` | absolute value |
| `floor(x)` | rounds down, floor(-1.2) = -2 |
| `ceil(x)` | rounds up, ceil(-1.2) = -1 |
| `round(x)` | nearest whole number; halves round away from zero |
| `min(a, b)` | the smaller of two numbers |
| `max(a, b)` | the larger of two numbers |
| `clamp(x, lo, hi)` | keeps x within the range; lo ≤ hi, neither bound nan |
| `lerp(a, b, t)` | a + (b - a) * t; t is not clamped |

`pi` and `tau` resolve as constants unless a shared variable has the same name; an assignment can shadow them. The built-in function names cannot be shadowed by your own functions.

```q0lang
import q0.math
angle = 90 * pi / 180
wave = sin(angle)
position = lerp(10, 90, 0.25)
limited = clamp(position, 0, 20)
```

`wave` is about `1`, `position` is `30` and `limited` is `20`. There is no random number generator, no `str()` / `number()` conversion and no string function yet. Strings join with `+`.

## functions the player answers

These look like math functions and go in expressions, with parentheses and commas, but q0player answers them while the script runs. Elsewhere they give a diagnostic and `null`. Each takes exactly the listed values.

| written as | gives |
| --- | --- |
| `q0layer.touches(layer, thing)` | whether anything `thing` draws overlaps anything drawn on the layer |
| `q0layer.which(layer, thing)` | the instance name of the first object on the layer that `thing` touches, `"drawing"` for unnamed artwork, `null` when nothing |
| `q0layer.hit(layer, x, y)` | whether the stage point lies on something drawn on the layer |
| `q0layer.circle(layer, x, y, radius)` | whether the circle overlaps the layer's drawing |
| `q0layer.box(layer, left, top, width, height)` | whether the rectangle overlaps it |
| `q0layer.visible(layer)` | whether the layer shows right now |
| `q0layer.count(layer)` | how many objects the layer shows on this frame |

What counts as drawn, and what `layer` and `thing` may be, is in the [host api](hosts.md#q0layer-whole-layers-and-everything-on-them).

## commands

Commands are statements; they never return a value into an expression. Put a space after the name ending in `!` and separate arguments with commas. Do not write `q0rig.pose!(...)` and do not assign a command's result.

| command | arguments | host |
| --- | --- | --- |
| `gorun! target` | frame index or target string | editor preview, player |
| `gostop! target` | frame index or target string | editor preview, player |
| `q0scene.switch! alias` | the alias of a bundled scene, as a string | player |
| `q0rig.position! control, x, y` | name, two numbers | player |
| `q0rig.value! control, value` | name, number | player |
| `q0rig.reset! control` | name | player |
| `q0rig.pose! pose, weight` | name, finite number; clamped to 0…1 | player |
| `q0rig.pose_reset! pose` | name | player |
| `q0physics.start!`, `q0physics.pause!`, `q0physics.reset!` | none | player |
| `q0physics.push! body, x, y` | instance name, two numbers in px/s | player |
| `q0physics.velocity! body, x, y` | instance name, two numbers in px/s | player |
| `q0physics.spin! body, degrees` | instance name, degrees per second, clockwise | player |
| `q0physics.teleport! body, x, y` | instance name, a stage point | player |
| `q0physics.gravity! x, y` | two numbers in px/s² | player |
| `q0physics.cut! link` | link name | player |
| `q0particles.start! emitter`, `q0particles.stop! emitter`, `q0particles.clear! emitter` | emitter name | player |
| `q0particles.burst! emitter, count` | emitter name, number of particles | player |
| `q0particles.move! emitter, x, y` | emitter name, a stage point | player |
| `q0light.on! light`, `q0light.off! light` | light name | player |
| `q0light.move! light, x, y` | light name, a stage point | player |
| `q0light.color! light, red, green, blue` | light name, three numbers 0…255 | player |
| `q0light.brightness! light, value` | light name, number; 1 shows colours as drawn | player |
| `q0light.size! light, radius` | light name, pixels | player |
| `q0light.darkness! value` | number, 0 (day) to 1 (pitch dark) | player |
| `q0layer.hide! layer`, `q0layer.show! layer`, `q0layer.reset! layer` | a layer's name or row | player |
| `q0layer.move! layer, x, y` | layer, two numbers in stage px | player |
| `q0layer.alpha! layer, value` | layer, number 0…1 | player |
| `q0layer.solid! layer, instance`, `q0layer.unsolid! layer, instance` | layer, instance name | player |
| `q0window.fullscreen! on`, `q0window.ui! on`, `q0window.hotkeys! on`, `q0window.top! on` | `true`/`false`, `1`/`0` or `"on"`/`"off"` | player |
| `q0window.move! x, y` | screen points | player |
| `q0window.size! width, height` | screen points, at least 1 | player |
| `q0window.title! text` | any value | player |
| `q0window.decorations! on`, `q0window.titlebar! on` | boolean, system window frame | desktop player |
| `q0window.icon! asset` | bundled bitmap name/id; null restores the default | desktop player |
| `q0window.center!`, `q0window.minimize!`, `q0window.perf!`, `q0window.quit!` | none | player |
| `q0shell.print! value, ...` | zero to 255 values | shell |
| `q0shell.read! path` | path | shell |
| `q0shell.write! path, text` | path, value to write | shell |
| `q0shell.append! path, text` | path, value to append | shell |
| `q0shell.mkdir! path` | path | shell |
| `q0shell.list!` or `q0shell.list! path` | no path or one path | shell |
| `q0shell.move! from, to` | two paths | shell |
| `q0shell.delete! path` | path | shell |

The detailed behaviour of frames, names, rigs, physics, particles and lights is in the [host api](hosts.md); file operations, value conversion and path rules are in [q0shell](q0shell.md). Keywords are covered in [syntax](syntax.md), events in [events](events.md).

Sources: [parser catalogue](../../q0s-format/src/q0lang/parser.rs), [qvm evaluation](../../q0vm/src/lib.rs), [command translation](../../q0s-format/src/q0lang/runtime.rs).
