# q0ex

[api reference](api.md) · [examples](examples/)

q0ex is the language q0editor plugins are written in. A plugin can reach every part of the editor: the
project, the stage, the timeline, the selection, the settings, files on disk. It can add menu items,
windows, keys and stage tools of its own.

A plugin is one text file ending in `.qx` (or `.q0ex`; they are the same). That one file holds the
plugin's card, its page, its settings, its pictures and its code.

## using plugins

**Plugins > Manage plugins** lists what is installed. Each card has a switch; the card opens the plugin's
page, with what the plugin says about itself, its settings and the way to remove it.

To install one, use **Add plugin...**, drop a `.qx` file on the window, or open it with **File > Open**.
The editor shows the plugin's name, author, version and **fingerprint** first and asks before installing.
The fingerprint is the same for the same file everywhere: compare it with the one posted where the plugin
was shared.

> A plugin can change anything in q0editor and your files. Install only plugins you trust.

Installed plugins live in the editor's settings folder under `q0editor/plugins`. A plugin that runs into
an error stops, and its card says why; it never takes the editor down with it.

## a plugin file

```q0ex
# Spin: turns the selection a little.
plugin "Spin" {
  id = "me.spin"
  version = "1.0.0"
  author = "me"
  summary = "Turns the selected objects."
  color = "#ff6b9d"
  thumbnail = "picture"
}

page ~~~
# Spin
Turns things **a little**, or {big}a lot{/}.
~~~

settings {
  slider step "Degrees a turn" = 15 from 1 to 90
}

when menu "Spin the selection" {
  for obj in project.selection() {
    obj.rotation += config.step
  }
}

blob picture png ~~~
iVBORw0KGgoAAAANSUhEUgAA...
~~~
```

| part | what it is |
| --- | --- |
| `plugin "Name" { }` | the card: `id` (unique, like `yourname.plugin`), `version`, `author`, `summary`, `color`, `thumbnail` |
| `page ~~~ ... ~~~` | the plugin's page, in [page markup](#page-markup) |
| `settings { }` | what people can set on the plugin's page; code reads `config.name` |
| `blob name kind ~~~ ... ~~~` | a picture or other file, as base64; the Plugin Studio adds these for you |
| everything else | code: `when` blocks, functions, `keep` and plain statements |

Plain statements at the top run once, when the plugin turns on.

## the language

q0ex is q0lang's cousin. One statement takes a line, `#` starts a comment, `{` ends a header and `}`
closes on a line of its own:

```q0ex
score = 0
if score < 10 {
  score += 1
} else if score == 10 {
  say! "ten!"
} else {
  score = 0
}
```

A body that fits can stay on its line: `if done { return }`. Lists, maps and calls may run over several
lines.

### values

| kind | written as |
| --- | --- |
| number | `12`, `0.5`, `-3`, and with a unit: `500ms` (half a second), `2s`, `45deg`, `10px` |
| text | `"hello"`, `'hello'`, and `` `frame {n + 1}` `` with values in braces |
| true / false / null | `true`, `false`, `null` |
| list | `[1, 2, 3]`, items by `list[0]`, from the end by `list[-1]` |
| map | `{name: "Ada", "with space": 2}`, values by `map.name` or `map["with space"]` |
| range | `1..5` is `[1, 2, 3, 4, 5]`; `0..<5` leaves out the 5 |
| function | `fn(x) { return x * 2 }` |
| object | layers, objects, timelines and library items the editor hands out |

Units only change how a number is written: `500ms` is `0.5`, and `deg` and `px` change nothing; they are
there to be read.

`false`, `null`, `0`, `""` and empty lists and maps count as false. `and` and `or` give back one of their
sides, so `name = config.name or "anonymous"` works.

### operators

`+ - * / % ^`, comparisons `== != < <= > >=`, `and or not` (also `&& || !`). `+` joins text with anything,
and lists with lists; `"ab" * 3` is `"ababab"`. `^` binds tighter than a minus: `-2 ^ 2` is `-4`.
`x += 1`, `-=`, `*=` and `/=` change a value in place.

### variables

Setting a name at the top makes a variable every block can see. Inside a function or a `when`, setting a
name that does not exist yet makes a local one that is gone afterwards. Functions see the variables around
them where they were made, so a function can keep a counter of its own.

### functions

```q0ex
fn twice(x) {
  return x * 2
}

doubled = map([1, 2, 3], fn(x) { return twice(x) })
```

### loops and choices

```q0ex
repeat 3 { say! "hi" }
while lives > 0 { lives -= 1 }
for obj in project.selection() { obj.x += 10 }
for key, value in {a: 1, b: 2} { print! key, value }
for i in 1..10 { print! i }
```

`break` leaves a loop and `continue` starts its next round.

### the quirks

**`name!` does something.** A word ending in `!` changes something: the project, a list, the screen.
Words without one only look. A command takes its values without parentheses at the start of a line:

```q0ex
say! "hello", 2
push! list, 4
editor.goto! 12
project.deselect!
```

Inside a bigger expression, write it with parentheses: `last = pop!(list)`.

**`do!`** runs its block right away, in the same scope, as in q0lang.

**`later!` and `every!`** wait without stopping anything:

```q0ex
ticks = 0
later! 2s {
  say! "two seconds later"
}
every! 250ms {
  ticks += 1
  if ticks == 20 { break }
}
```

`break` inside `every!` stops it for good.

**`keep`** makes a variable that outlives the editor. Its value is saved when the plugin stops and comes
back next time:

```q0ex
keep launches = 0
when enabled { launches += 1 }
```

**`try` and `oops`** catch a mistake, and `oops!` makes one:

```q0ex
try {
  oops! "no selection"
} oops why {
  say! `that did not work: {why}`
}
```

**`a <-> b`** swaps two values.

**`|>`** hands a value to the next function as its first value:

```q0ex
total = project.selection() |> map(fn(obj) { return obj.x }) |> sum()
```

### limits

A run that takes too long, like an endless loop, is stopped with an error, and so is a function that
calls itself forever. A `try` cannot catch those. A plugin that stops on an error can be started again
from its card once it is fixed.

## events

`when` blocks are how a plugin hears about things:

```q0ex
when menu "Make it red" { ... }        # an item under Plugins > (plugin)
when window "Controls" { ... }         # a window, drawn every frame while open
when tool "Spray" { ... }              # a stage tool; press, drag, release and hover use it
when press x, y { ... }
when draw { ... }                      # drawing over the stage
when key "ctrl+shift+k" { ... }
when frame n { ... }
when setting "size" value { ... }      # a setting changed, or its button was pressed
```

The [api reference](api.md#events) lists them all with what they pass.

## the editor

The editor's words come in namespaces: `editor`, `project`, `ui`, `draw`, `file`, `settings`, `config`,
`keys`, `mouse`, `clip` and `plugin`. The [api reference](api.md) lists every one.

`project.selection()`, `project.objects()`, `project.layers()` and their kin hand out **objects**. Their
fields read and write the project:

```q0ex
for obj in project.selection() {
  obj.x += 10
  obj.rotation = 45
  obj.alpha = 0.5
  print! obj.kind, obj.name
}
layer = project.layer()
layer.locked = true
```

Objects have `x`, `y`, `rotation`, `scale_x`, `scale_y`, `skew_x`, `skew_y`, `alpha`, `name`, `kind`,
`frame`, `asset`, `symbol`, `layer` and `timeline`. Layers have `name`, `kind`, `visible`, `locked`,
`keyframes` and `objects()`. Timelines have `name`, `frames`, `script` and `layers()`. Library items have
`name`, `kind`, `fill`, `stroke` and their size.

An object is the one on the keyframe in effect on the frame it was found on. Changing it changes that
keyframe.

### raw data: anything else

Everything has a `raw` field: the thing itself as plain maps and lists, with every detail the file format
holds. Read it, change it and set it back to change what no word above covers:

```q0ex
obj = first(project.selection())
data = obj.raw
data.fx.blend_mode = "Multiply"
obj.raw = data
```

`project.raw()` is the whole project, and `project.runtime()` its extras: physics, particles, lights,
camera, labels and more. Big projects make big data, so keep raw reads out of `tick` and `draw`.

### undo

Everything one `when` block changes in the project is one undo step.

### drawing on the stage

`project.line!`, `project.rect!`, `project.circle!` and `project.path!` draw real artwork on the current
layer. `draw.line!`, `draw.circle!`, `draw.text!` and the rest only paint over the stage inside
`when draw`, for guides, previews and tools.

### windows

Inside `when window`, `ui` fills the window, every frame:

```q0ex
size = 10
when window "Brush" {
  ui.heading! "Brush"
  size = ui.slider("Size", size, 1, 100)
  if ui.button("Reset") { size = 10 }
  ui.row! fn() {
    ui.text! "in a row"
    ui.text! "side by side"
  }
}
```

## settings

```q0ex
settings {
  heading "Look" ? "How it looks"
  slider size "Size" = 10 from 1 to 100 step 1 ? "Shown when hovering"
  number count "Count" = 3
  toggle glow "Glow" = true
  text label "Label" = "hello"
  color tint "Tint" = "#ff4488"
  choice mode "Mode" = "fill" of "fill", "line", "both"
  button reset "Reset everything"
  note "Changes apply right away."
}
```

The plugin's page shows them the way q0editor's own settings look. Code reads `config.size`, may set
`config.size = 20`, and hears changes and button presses with `when setting "size" value`.

## page markup

```text
# Heading            ## Smaller           ### Smallest
**bold**  *italic*  __underline__  ~~strike~~  `code`
{red}coloured{/}  {#ff8800}any colour{/}  {big}bigger{/}  {small}  {huge}
[a link](https://q0id.com)
- a bullet           1. a numbered line   > a callout
---                  (a divider)
![a caption](picture)    a picture from the plugin's blobs
->centred line<-     ->pushed to the right
```

## the Plugin Studio

**Plugins > Plugin Studio** makes plugins, from scratch or from one of the examples.

**Simple** builds a plugin from blocks. Drag blocks out of the palette and click them into place; hats
start a stack (`when` blocks, functions), and stacks lying loose run when the plugin turns on. Click a
white field to type in it, use the ▾ to pick, drop a block on the palette to delete it, ctrl and the
wheel to zoom. The Card, Page, Settings and Pictures tabs fill in the rest of the file.

**Pro** is the whole file as text, with colours, line numbers and the line of a mistake marked. Typing
part of a word offers the rest (Tab takes it), the Reference lists every word, and clicking one types it.

Both edit the same file: switch whenever you like. **Run** saves the plugin and turns it on (or restarts
it), **Save** keeps it in the plugin folder, and **Export .qx** writes a copy to share.
