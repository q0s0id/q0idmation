# events

[home](README.md) · [host api](hosts.md) · [functions](syntax.md)

`listen event handler` adds a handler's name to an event's list; `xlisten event handler` removes it. These are registration statements, without `!` and without parentheses. Subscribing the same handler twice keeps one entry; removing a subscription that is not there is fine.

```q0lang
import q0.mouse

pb func on_click target {
  q0shell.print! "clicked", target
}

listen mouse.click on_click
xlisten mouse.click on_click
```

## event names

| name | when q0player sends it |
| --- | --- |
| `mouse.down` | a button is pressed |
| `mouse.up` | a button is released |
| `mouse.move` | the pointer moves |
| `mouse.click` | a press and release |
| `mouse.double` | a double click |
| `mouse.wheel` | the wheel or a two-finger scroll turns |
| `mouse.over` | the pointer enters an object (object scripts only) |
| `mouse.out` | the pointer leaves an object (object scripts only) |

Another event name gives a parser diagnostic. The runtime stores the names without checking that the handler exists or how many parameters it takes.

## delivery in q0player

q0player delivers events to vector v2 movies that import `q0.mouse` or have [object scripts](hosts.md). Each handler is called with one argument, `target`: `"stage"` in the movie's shared runtime, the object's name in an object script. On touch screens a tap counts as the mouse.

Before a handler runs, the shared runtime gets these values, in stage coordinates:

| name | value |
| --- | --- |
| `mouse.x`, `mouse.y` | where the pointer is |
| `mouse.inside` | the pointer is over the stage |
| `mouse.wheel_x`, `mouse.wheel_y` | how far the wheel turned, for `mouse.wheel` |

Object scripts receive events only when they concern them:

- `mouse.down`, `mouse.double` and `mouse.wheel` go to the object under the pointer.
- `mouse.up` goes to the object under the pointer, and to the one the press started on.
- `mouse.click` needs the press and the release on the same object.
- `mouse.over` and `mouse.out` are sent when the pointer crosses an object's bounds.

Inside an object script, `self.hovered`, `self.mouse_x` and `self.mouse_y` tell where the pointer is relative to that object. Commands a handler issues are applied right after it, like any script's.

The editor preview and q0shell keep the subscription table but deliver no events. Calling `on_click("button")` yourself is an ordinary function call, not a mouse event.

Sources: [parser catalogue and subscriptions](../../q0s-format/src/q0lang/parser.rs), [qvm listener table](../../q0vm/src/lib.rs), [player delivery](../../q0player/src/lib.rs), [object delivery](../../q0player/src/objects.rs), [player input](../../q0player/src/ui/app.rs).
