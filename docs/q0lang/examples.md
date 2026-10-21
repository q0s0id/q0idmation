# recipes

[home](README.md) · [host api](hosts.md) · [events](events.md)

Short scripts for common jobs. Each one names where it goes: a q0rg's general script, frame code, or a symbol's object script. The instance, emitter and body names are examples; use the names in your project.

## shared helpers in the general script

Functions defined in the main timeline's general script stay available to all later frame code.

```q0lang
import q0.math

pb func ease_in_out t {
  return t * t * (3 - 2 * t)
}

pb func between a b t {
  return lerp(a, b, ease_in_out(clamp(t, 0, 1)))
}
```

Frame code can then call `between(0, 300, 0.5)`.

## a wobble that runs every tick

In a one-frame movie, `import q0.input` keeps the frame's code running at the movie's fps. Start the counter in the general script:

```q0lang
wobble = 0
```

and move a `logo` instance in the frame code:

```q0lang
import q0.input
import q0.time
import q0.math
import q0.scene

wobble = wobble + time.dt
logo.y = stage.height / 2 + sin(wobble * 4) * 20
```

## moving a player with the keyboard

Frame code of a held frame. The step is limited by the bounds measured before this run, so the instance stays on the stage.

```q0lang
import q0.input
import q0.time
import q0.math
import q0.scene

speed = 240
dx = 0
if input.left {
  dx = dx - speed * time.dt
}
if input.right {
  dx = dx + speed * time.dt
}
dx = clamp(dx, 0 - player.left, stage.width - player.right)
player.x = player.x + dx
```

## a button that lights up

The object script of a button symbol whose frame 0 is the normal look and frame 1 the highlight. Every copy of the button reacts on its own.

```q0lang
import q0.object
import q0.mouse
import q0.timeline

pb func on_over target {
  gostop! 1
}

pb func on_out target {
  gostop! 0
}

listen mouse.over on_over
listen mouse.out on_out
```

## kicking a ball with live physics

Turn on "Live in q0player" in the physics window and give the ball the script name `ball`. Then, in the main timeline's general script:

```q0lang
import q0.mouse
import q0.physics

pb func kick target {
  q0physics.start!
  q0physics.push! "ball", 0, -500
}

listen mouse.click kick
```

## sparks where you click

Make an emitter called `sparks` in the particle window and turn on "Live". Then, in the general script:

```q0lang
import q0.mouse
import q0.particles

pb func sparkle target {
  q0particles.move! "sparks", mouse.x, mouse.y
  q0particles.burst! "sparks", 25
}

listen mouse.down sparkle
```

## a torch that follows the player

In the particle editor, start an emitter from the Fire preset, press "Selected object" next to Follows with the player selected, and turn on "Live". In the light editor, add a Torch light that follows the player too. The fire and its glow then ride along on their own; a script only needs to light them:

```q0lang
import q0.particles
import q0.light

q0particles.start! "fire"
q0light.on! "torch"
```

## a light switch

Frame code of a held frame. Space toggles the room's lamp, and the room goes dark without it.

```q0lang
import q0.input
import q0.light

if input.jump_pressed {
  lamp_on = not lamp_on
  if lamp_on {
    q0light.on! "lamp"
    q0light.darkness! 0.4
  } else {
    q0light.off! "lamp"
    q0light.darkness! 0.95
  }
}
```

Set `lamp_on = true` in the general script first.

## stopping on a frame once

Frame code that jumps once, guarded by a flag so the jump does not repeat itself:

```q0lang
import q0.timeline

if not intro_done {
  intro_done = true
  gostop! 0
}
```

Set `intro_done = false` in the general script first; an unassigned name is a truthy `ident`, so `not intro_done` would be false.
