# animation and host api

[home](README.md) · [libraries](builtins.md) · [recipes](examples.md)

A host is the program that runs q0lang and applies what it asks for. The runtime itself never opens files or draws the stage.

## availability

| capability | editor: code analysis | editor: preview | q0player v2 | q0shell |
| --- | --- | --- | --- | --- |
| math, functions, conditions, loops | yes, with a small budget | yes | yes | yes |
| general script and frame code | analyses one text | yes | yes | one input text |
| frame jumps and stops | reported only | yes | yes | reports it is unavailable |
| named instances, keyboard, time.dt | no host values | no host values | yes | no host values |
| object scripts (`self.*`) | analyses one text | no | yes | no |
| rig commands | reported only | not applied | yes | reports it is unavailable |
| live physics, particles and lights | reported only | not applied | yes | reports it is unavailable |
| layer commands and solid layers | reported only | not applied | yes | reports it is unavailable |
| layer questions (`q0layer.touches(...)`) | diagnostic, `null` | diagnostic, `null` | yes | diagnostic, `null` |
| window commands and `window.*` | reported only | not applied | yes, desktop; phones keep their screen | reports it is unavailable |
| switching bundled scenes | reported only | not applied | yes | reports it is unavailable |
| file commands | reported only | not applied | ignored | yes |
| mouse subscription table | yes | yes | yes | yes |
| mouse events delivered to handlers | no | no | yes | no |

Analysing a single script cannot reproduce the shared variables that dependencies and earlier frames will have set. Test interactive projects in q0player. Everything here applies to vector v2 movies; old bitmap v1 movies and q0v videos do not run q0lang.

## start-up order

q0player first creates the shared runtime. The embedded q0lang dependencies of the root project graph run in ascending node id order, then the general script of the entry q0rg, then the code of the first frame. External q0lang dependencies do not run: the player reports that they need a bundled export. If the start-up script already jumped somewhere, the destination's code is not run a second time.

During normal playback, the code of every frame the playhead enters runs. Scripts of one frame run in ascending layer id order, then in stored order. Shared state and defined functions carry over. Frame code of nested q0rgs does not run in the root runtime; a placed symbol with code of its own runs in an [object script](#object-scripts).

The editor preview runs the general script of the open q0rg and its frame code. Moving the editor's playhead by hand is editing and runs no frame code; `seek` in the player does run the destination frame's code.

## q0.timeline

```q0lang
import q0.timeline
start = 3
gorun! start
```

`gorun! target` starts playback and jumps to target. `gostop! target` stops playback and jumps to target. A jump runs the destination's code and skips the frames in between. The command does not end the current script: the whole script runs first, then the host applies the collected actions in order.

Frame indices in code start at zero. A non-negative numeric target becomes a u16: the fraction is dropped and values that are too large saturate at 65535. The host clamps it to the last frame. Use finite whole numbers within the timeline.

A string target like `"12"` is taken as a numeric index. Named labels like `"intro"` do not resolve yet. Playback still starts or stops even when the target does not resolve. There is no automatic `frame` or `label` variable in the root runtime; inside an object script, `frame` holds that object's own frame.

An unconditional jump from a frame to itself runs the same code again. The editor and the player stop a chain of jumps after 64 transitions. To stop on a frame without looping, use a flag variable, or put the command in the general script.

## q0.input and q0.time

| name | type | value in q0player |
| --- | --- | --- |
| `input.left` | bool | A or the left arrow is held |
| `input.right` | bool | D or the right arrow is held |
| `input.jump` | bool | W, the up arrow or Space is held |
| `input.jump_pressed` | bool | jump went from released to pressed; cleared after the frame's scripts run |
| `input.up` | bool | W or the up arrow is held |
| `input.down` | bool | S or the down arrow is held |
| `input.action` | bool | Z or Enter is held |
| `input.action_pressed` | bool | action went from released to pressed; cleared like `input.jump_pressed` |
| `time.dt` | number | the fixed step in seconds, 1 / fps |

`import q0.input` switches on a game clock that runs independently of the animation's pause button. While a frame is held, its code keeps running at the movie's fps, even in a one-frame project, so controls and movement keep updating with the timeline stopped. Without the import, a paused animation runs no new ticks. On touch screens q0player shows on-screen left, right and jump buttons; when a script reads `input.up` or `input.down`, the arrows become a cross with up and down too. The big button is jump and action at once. Up also counts as jump on a keyboard, so platformers and four-way games share the keys.

The host refreshes these values before every script. Assigning `input.left` or `time.dt` changes neither the keyboard nor the clock, and the next refresh overwrites it. The api offers no other keys, no typed text, no timers and no system time.

## q0.scene: size and instances

| name | access | value |
| --- | --- | --- |
| `stage.width`, `stage.height` | read | the stage size in stage units |
| `name.x`, `name.y` | read and write | moves the named instance |
| `name.left`, `name.right`, `name.top`, `name.bottom` | read | the instance's bounds on the stage |
| `name.width`, `name.height` | read | the width and height of those bounds |
| `name.frame`, `name.playing` | read and write | the own timeline of a named instance that has an object script |

Replace `name` with an instance name given in the project, such as `player`. The library creates no character by itself. The instance must belong to the current entry q0rg; an unknown name stays an ordinary symbolic identifier.

```q0lang
import q0.scene
player.x = player.x + 10
```

The example needs an instance called `player` in q0player. Writes to x/y apply after the script ends, as a temporary runtime transform; the project itself never changes. Both values must be finite numbers. Writing width, height or the bounds does not scale the instance: those are measured again.

Within one script, writing `player.x` does not update `player.right`; the geometry is measured between runs. Compute limits from the starting bounds plus your own offset, as in the [movement recipe](examples.md).

## object scripts

Every placed copy of a symbol (q0rg) that has code of its own, a general script or frame code, gets its own runtime in q0player. Each copy keeps separate variables, its own timeline position and its own handlers, so ten coins can each count their own score. The object's name is its instance name, or `Symbol#id` when it has none.

- The symbol's general script runs once when the copy appears on stage.
- Its frame code runs whenever the copy's own frame changes. With `q0.object`, `q0.input` or `q0.mouse` imported, it runs every tick instead.
- `gorun!` and `gostop!` inside an object script drive that copy's own timeline.
- Mouse events reach the copy under the pointer, see [events](events.md).

| name | access | value |
| --- | --- | --- |
| `self.x`, `self.y` | read and write | position within the parent timeline; writing moves the copy |
| `self.frame` / `frame` | read | the copy's current frame |
| `self.playing` | read and write | whether the copy's timeline plays |
| `self.name`, `self.id` | read | instance name and instance id |
| `self.left`, `self.right`, `self.top`, `self.bottom`, `self.width`, `self.height` | read | the copy's bounds |
| `self.hovered` | read | the pointer is over the copy |
| `self.mouse_x`, `self.mouse_y` | read | the pointer in the copy's own coordinates |

Named instances inside the symbol are reachable as `name.x` and so on, relative to the copy. q0player runs at most 4096 active objects, and each object run has its own budget of 100,000 instructions.

```q0lang
import q0.object
import q0.mouse

pb func on_click target {
  self.playing = not self.playing
}

listen mouse.click on_click
```

## q0.scene: switching scenes

```q0lang
import q0.scene
q0scene.switch! "room_2"
```

The alias must match the alias of a bundled movie in the root project's graph. An unknown alias gives a diagnostic. This never loads an arbitrary path or opens an outside file.

The player replaces the current scene and starts at frame zero. It drops temporary moves, rig changes, live physics and particles, and refreshes the sound. The shared runtime keeps its variables, functions and imports. Then the new scene's general script and its first frame's code run. The q0lang dependencies of the child scene's graph do not run again, so keep shared logic in the root graph.

## q0.rig

| command | does |
| --- | --- |
| `q0rig.position! control, x, y` | a temporary position for a control |
| `q0rig.value! control, value` | a temporary numeric value for a control |
| `q0rig.reset! control` | removes that control's temporary position and value |
| `q0rig.pose! pose, weight` | blends in a saved pose with a weight clamped to 0…1 |
| `q0rig.pose_reset! pose` | removes that pose's temporary weight |

Names are strings; position and value take numbers. The pose weight must be finite. The commands act on the entry q0rg's rig. A control must be open to the runtime through its "visible in simple" setting; a hidden or missing control gives a diagnostic. Poses are looked up by name and need no such setting.

```q0lang
import q0.rig
q0rig.position! "look", 40, 25
q0rig.value! "arm blend", 0.75
q0rig.pose! "wave", 0.6
```

This example assumes a project with those controls and that pose. The commands store temporary values; what they do depends on the rig's bindings. `reset` does not delete the authored control, and `pose_reset` does not delete the saved pose. Temporary settings survive frame jumps; switching scenes drops them.

## q0.physics

The physics world of the main timeline, when "Live in q0player" is on in the physics editor. Without that switch, physics is only baked into keys and does not travel with the movie.

| command | does |
| --- | --- |
| `q0physics.start!` | runs the world from the current frame, or resumes it after a pause |
| `q0physics.pause!` | holds the world; bodies stay where they are |
| `q0physics.reset!` | removes the world and hands the bodies back to the timeline |
| `q0physics.push! body, x, y` | adds to a body's velocity, in px/s |
| `q0physics.velocity! body, x, y` | sets a body's velocity, in px/s |
| `q0physics.spin! body, degrees` | sets the spin, in degrees per second, clockwise |
| `q0physics.teleport! body, x, y` | puts a body down at a stage point |
| `q0physics.gravity! x, y` | new gravity for the world, in px/s²; y points down |
| `q0physics.cut! link` | cuts a rope or chain in its middle, or undoes a spring, hinge or weld |

A body is named by its instance name, set with the "Script name" field in the physics window; a link by the name it has in the window's Links list. The world steps once per movie frame, even with the timeline paused. Animated bodies keep following the timeline, bodies tied on with links swing and bounce along, and liquids, ropes, chains and springs are drawn over the stage. Any command other than `start!` also builds the world but does not start it. Switching scenes drops the world.

```q0lang
import q0.physics
q0physics.start!
q0physics.push! "ball", 300, -600
q0physics.cut! "rope1"
```

## q0.particles

The emitters of the particle editor that have "Live" on. An emitter releases nothing until a command tells it to.

| command | does |
| --- | --- |
| `q0particles.start! emitter` | starts emitting, beginning with the emitter's burst |
| `q0particles.stop! emitter` | stops emitting; particles in flight live out their lives |
| `q0particles.burst! emitter, count` | releases count particles at once |
| `q0particles.move! emitter, x, y` | moves the emitter to a stage point |
| `q0particles.clear! emitter` | stops it and removes all its particles |

Particles are drawn over the stage. Glowing (additive) emitters add their light to what is behind them. Particles that are library items are drawn at full opacity during playback; the built-in shapes fade with their colour. An emitter that follows an object in the particle editor follows it in the player too, wherever the timeline, live physics or a script has put it.

```q0lang
import q0.particles
q0particles.move! "sparks", player.x, player.y
q0particles.burst! "sparks", 40
```

## q0.light

The lights of the light editor. Lights need no switch to travel: every movie with lights is lit while it plays, and these commands change them on the fly. A light is named by the name given in the light editor.

| command | does |
| --- | --- |
| `q0light.on! light` | switches a light on |
| `q0light.off! light` | switches a light off |
| `q0light.move! light, x, y` | puts a light at a stage point; it stops following its object |
| `q0light.color! light, red, green, blue` | a new colour, each channel 0 to 255 |
| `q0light.brightness! light, value` | 1 shows colours as drawn, 2 doubles them, 0 is off |
| `q0light.size! light, radius` | how far it reaches, in pixels |
| `q0light.darkness! value` | how dark unlit places are, 0 (day) to 1 (pitch dark) |

Lights shine on the main timeline's layers up to its light layer; layers above it stay as drawn. A command that changes a light's position, colour, brightness or size takes it over from its keyframes for the rest of the scene. Switching scenes drops every change.

```q0lang
import q0.light
import q0.mouse

pb func lights_out target {
  q0light.off! "lamp"
  q0light.darkness! 0.95
}

listen mouse.click lights_out
```

## q0.layer: whole layers and everything on them

A layer of the main timeline is named by its name in the timeline, like `"bones"`, or by its row counted from the top of the timeline, like `1` for the top row. An exact name wins; otherwise case does not matter. A folder stands for everything inside it.

| command | does |
| --- | --- |
| `q0layer.hide! layer` | hides the layer and everything on it |
| `q0layer.show! layer` | shows it, even one hidden in the editor |
| `q0layer.move! layer, x, y` | draws the whole layer `x`, `y` stage pixels away from where it is drawn; `0, 0` puts it back |
| `q0layer.alpha! layer, value` | fades it: 1 as drawn, 0 invisible |
| `q0layer.reset! layer` | the layer as authored again |
| `q0layer.solid! layer, instance` | the instance can no longer move into anything drawn on the layer |
| `q0layer.unsolid! layer, instance` | lets it through again |

The questions are [functions](builtins.md#functions-the-player-answers): `q0layer.touches(layer, thing)`, `q0layer.which(layer, thing)`, `q0layer.hit(layer, x, y)`, `q0layer.circle(layer, x, y, radius)`, `q0layer.box(layer, left, top, width, height)`, `q0layer.visible(layer)` and `q0layer.count(layer)`. A `thing` is an instance name of the main timeline, or the name of another layer, which asks about everything on it.

What counts as drawn:

- fills where they are filled, and lines along their width; a box drawn as an outline is solid along its line and empty inside, a filled box is solid all over
- brush strokes, and the artwork inside symbols placed on the layer, at the frame they show
- shapes as a shape tween or a motion tween has them on this frame
- bitmaps and videos as their rectangles

Colour does not matter, so a fully transparent shape still counts: invisible walls are transparent shapes, or a layer faded to 0 with `q0layer.alpha!`. A hidden layer counts as nothing. Glow, shadow, blur and masks are left out; a moved layer counts where it is drawn.

A question sees where the script has put things, although moves land after the script:

```q0lang
import q0.layer
soul.x = soul.x + 4
hit = q0layer.touches("bones", "soul")
```

`hit` is about the soul's new place. An instance that sits on the layer it asks about never touches itself.

A solid layer stops an instance a script moves. After each script of the main timeline, an instance with a solid layer stops at the layer's edge and slides along it, and `name.x` / `name.y` say where it stopped. A fast move is walked in small steps, so it cannot jump over a thin line. Something already inside a solid layer, because the layer moved onto it, moves freely until it is out. Animation, `q0layer.move!` and live physics do not push anything.

```q0lang
import q0.input
import q0.layer
import q0.time

if not ready {
  q0layer.solid! "walls", "soul"
  hp = 20
  hurt = 0
  ready = true
}
step = 160 * time.dt
if input.left {
  soul.x = soul.x - step
}
if input.right {
  soul.x = soul.x + step
}
if input.up {
  soul.y = soul.y - step
}
if input.down {
  soul.y = soul.y + step
}
if hurt > 0 {
  hurt = hurt - 1
} else {
  if q0layer.touches("bones", "soul") {
    hp = hp - 1
    hurt = 30
  }
}
```

This is the frame code of a held frame; draw the box's walls as lines on a layer called `walls`, the attacks on a layer called `bones`, and animate the bones with tweens on the main timeline. Commands from an object script act on the main timeline's layers; questions are answered for the main timeline's own scripts only.

## q0.window

The player's own window, on desktop systems. Sizes and places are in screen points; `window.*` and `screen.*` are refreshed before every script.

| name | value |
| --- | --- |
| `window.x`, `window.y` | where the window's outer corner is on the desktop |
| `window.width`, `window.height` | the size of the window's inside |
| `window.fullscreen` | whether it is fullscreen |
| `window.ui` | whether the menu bar and status show around the movie |
| `screen.width`, `screen.height` | the size of the screen the window is on |

Writing `window.x`, `window.y`, `window.width` or `window.height` moves or sizes the window, like the commands:

| command | does |
| --- | --- |
| `q0window.fullscreen! on` | fullscreen with only the movie, as F9 does; `false` comes back |
| `q0window.ui! on` | shows or hides the menu bar and status line |
| `q0window.hotkeys! on` | whether F9 and F4 still work; Ctrl+O and Ctrl+R always do |
| `q0window.move! x, y` | puts the window there |
| `q0window.size! width, height` | sizes its inside |
| `q0window.center!` | centres it on its screen |
| `q0window.title! text` | the title in the title bar |
| `q0window.decorations! on` (`q0window.titlebar! on`) | desktop system frame and title-bar buttons; false hides them, true restores them |
| `q0window.icon! "app_icon"` | desktop window/taskbar icon from a library bitmap name or numeric asset id |
| `q0window.icon! null` | restores q0player's own window icon |
| `q0window.top! on` | keeps it above other windows |
| `q0window.minimize!` | minimizes it |
| `q0window.perf!` | copies the performance log, as F4 does |
| `q0window.quit!` | closes the player |

`on` takes `true` or `false`, `1` or `0`, or `"on"` or `"off"`. A fullscreen window neither moves nor sizes. Opening a movie again puts the menu bar and the keys back.

```q0lang
import q0.window
import q0.math

# shake the window when something hits
pb func shake amount {
  window.x = window.x + amount
}
q0window.title! "BONE ZONE"
q0window.size! 640, 480
q0window.center!
```

Sources: [player](../../q0player/src/lib.rs), [layers and window](../../q0player/src/layers.rs), [what layers draw](../../q0s-format/src/collide.rs), [object scripts](../../q0player/src/objects.rs), [live physics, particles and lights](../../q0player/src/live.rs), [lighting](../../q0s-format/src/lighting.rs), [player keys](../../q0player/src/ui/app.rs), [editor preview](../../q0editor/src/app.rs), [runtime commands](../../q0s-format/src/q0lang/runtime.rs), [simulation](../../q0physics/src/lib.rs).

Window icons are bundled bitmap assets, not filesystem paths. Import a PNG into
the movie library and pass its library name or asset id. Closing the movie or
opening another one restores the normal player icon and system frame. These
commands affect the desktop window; they do not change the executable's file
icon, and have no effect in the browser or Android host.
