# q0ex api reference

[guide](README.md) · [examples](examples/)

Every word a plugin can use besides the language itself. Words ending in `!` do something; the others give back a value. Frames count from 1, angles are degrees and positions are stage pixels. This page is written from the editor's own list, the same one the Plugin Studio's reference and blocks use.

## events

| when | gives | runs |
| --- | --- | --- |
| `when enabled` |  | Runs when the plugin is switched on, and when the editor starts with it on. |
| `when disabled` |  | Runs just before the plugin is switched off. |
| `when menu "Do the thing"` |  | Adds an item to Plugins > (this plugin). |
| `when window "My window"` |  | A window under Plugins > (this plugin). Runs every frame while it is open; use ui to fill it. |
| `when tool "My tool"` |  | Adds a stage tool. While it is in use, press, drag, release and hover get the pointer. |
| `when press` | x, y | The plugin's tool is in use and the stage was pressed, at stage x, y. |
| `when drag` | x, y | Every frame of a drag with the plugin's tool. |
| `when release` | x, y | The press of the plugin's tool ended. |
| `when hover` | x, y | The pointer moved over the stage with the plugin's tool. |
| `when draw` |  | Draw over the stage every frame with draw. |
| `when tick` | dt | Runs every frame. Keep it quick. |
| `when frame` | n | The current frame changed; n counts from 1. |
| `when selection` |  | Something else got selected. |
| `when opened` |  | Another project or tab became the current one. |
| `when saved` | path | The project was saved. |
| `when key "ctrl+k"` |  | A key or chord like ctrl+shift+k, while no text field is being typed in. |
| `when setting "name"` | value | A setting of this plugin changed, or its button was pressed. |

## control

| word | takes | does |
| --- | --- | --- |
| `oops!` | message | Stops with an error; `try` can catch it. |

## editor

| word | takes | does |
| --- | --- | --- |
| `editor.frame` |  | The frame the playhead is on, from 1. → value |
| `editor.goto!` | frame | Moves the playhead. |
| `editor.frames` |  | How long the current timeline is. → value |
| `editor.play!` |  | Starts playing. |
| `editor.stop!` |  | Stops playing. |
| `editor.playing` |  | Whether it is playing. → value |
| `editor.tool` |  | The tool in use, like brush or select. → value |
| `editor.tool!` | name | Picks a built-in tool: select, brush, pen, line, rectangle, oval... |
| `editor.zoom` |  | The stage zoom; 1 fits the window. → value |
| `editor.zoom!` | zoom | Zooms the stage. |
| `editor.status!` | text | Writes the status line at the bottom. |
| `editor.action!` | name | Runs any editor command by name, like insert-keyframe or undo. |
| `editor.actions` |  | The names editor.action! takes. → value |
| `editor.color` |  | The colour new lines get. → value |
| `editor.color!` | color | Sets the colour new lines get. |
| `editor.fill` |  | The colour new fills get, or null. → value |
| `editor.fill!` | color | Sets the fill colour; null for no fill. |
| `editor.width` |  | The width new lines get. → value |
| `editor.width!` | width | Sets the width new lines get. |
| `editor.mouse` |  | Where the pointer is on the stage, as {x, y}, or null. → value |
| `editor.version` |  | q0editor's version. → value |
| `editor.dark` |  | Whether the theme is dark. → value |
| `editor.accent` |  | The theme's accent colour. → value |
| `keys.down` | key | Whether a key is held, like Space or A. → value |
| `keys.pressed` | key | Whether a key went down this frame. → value |
| `keys.shift` |  | Whether shift is held. → value |
| `keys.ctrl` |  | Whether ctrl (cmd on a Mac) is held. → value |
| `mouse.x` |  | The pointer's stage x, or null. → value |
| `mouse.y` |  | The pointer's stage y, or null. → value |

## project

| word | takes | does |
| --- | --- | --- |
| `project.name` |  | The project's name. → value |
| `project.path` |  | Where the project is saved, or null. → value |
| `project.width` |  | The stage's width. → value |
| `project.height` |  | The stage's height. → value |
| `project.size!` | width, height | Resizes the stage. |
| `project.fps` |  | Frames a second. → value |
| `project.fps!` | fps | Sets the frame rate. |
| `project.background` |  | The stage colour. → value |
| `project.background!` | color | Sets the stage colour. |
| `project.dirty` |  | Whether there are unsaved changes. → value |
| `project.save!` |  | Saves, asking where when it was never saved. |
| `project.timeline` |  | The timeline being edited. → value |
| `project.main` |  | The project's main timeline. → value |
| `project.timelines` |  | The main timeline and every symbol's. → value |
| `project.layers` |  | The layers of the timeline being edited, top first. → value |
| `project.layer` | name | The current layer, the one with that name, or makes a given layer current. → value |
| `project.add_layer!` | name | Adds a layer above the current one and gives it back. |
| `project.objects` | frame | Every object on a frame (the current one when left out). → value |
| `project.selection` |  | The selected objects. → value |
| `project.select!` | objects | Selects objects. |
| `project.deselect!` |  | Clears the selection. |
| `project.assets` |  | Everything in the library. → value |
| `project.asset` | id | A library item by id or name. → value |
| `project.line!` | x1, y1, x2, y2, color, width | Draws a line on the current layer; colour and width are optional. |
| `project.rect!` | x, y, w, h, fill, line, width | Draws a rectangle; fill, line colour and width are optional. |
| `project.circle!` | x, y, r, fill, line, width | Draws a circle; fill, line colour and width are optional. |
| `project.path!` | points, closed, fill, line, width | Draws a path through [x, y] points. |
| `project.raw` |  | Everything in the project as maps and lists. Slow on big projects. → value |
| `project.raw!` | data | Replaces the whole project with edited data. |
| `project.runtime` |  | Physics, particles, lights, camera, labels and more, as data. → value |
| `project.runtime!` | data | Replaces them with edited data. |

## objects

| word | takes | does |
| --- | --- | --- |
| `object.x` |  | An object's x; set it to move it. Also y, rotation, scale_x, scale_y, skew_x, skew_y, alpha. → value |
| `object.kind` |  | drawing, bitmap, video, rig or symbol. → value |
| `object.name` |  | The instance name q0lang uses; set it to rename. → value |
| `object.raw` |  | Every detail of the object; set it to change any of them. → value |
| `object.move!` | dx, dy | Moves an object. |
| `object.copy!` | dx, dy | Copies an object and gives the copy back. |
| `object.remove!` |  | Deletes an object. |
| `object.select!` |  | Selects just this object. |
| `layer.objects` | frame | A layer's objects on a frame. Layers also have name, kind, visible, locked, keyframes and raw. → value |
| `layer.select!` |  | Makes a layer the current one. |
| `layer.remove!` |  | Deletes a layer. |
| `timeline.layers` |  | A timeline's layers. Timelines also have name, frames, script and raw. → value |
| `timeline.enter!` |  | Opens a symbol's timeline for editing. |

## windows

| word | takes | does |
| --- | --- | --- |
| `ui.text!` | text | A line of text. |
| `ui.heading!` | text | A big line of text. |
| `ui.small!` | text | A quiet line of text. |
| `ui.colored!` | color, text | A line of coloured text. |
| `ui.page!` | markup | Text in page markup: **bold**, {red}colour{/} and the rest. |
| `ui.button` | label | A button; true on the frame it is clicked. → value |
| `ui.link` | label | A link-looking button. → value |
| `ui.slider` | label, value, min, max | A slider; gives back the new value. → value |
| `ui.number` | label, value | A number field; gives back the new value. → value |
| `ui.toggle` | label, on | A checkbox; gives back true or false. → value |
| `ui.input` | label, text | A text field; gives back the text. → value |
| `ui.color` | label, color | A colour picker; gives back the colour. → value |
| `ui.choice` | label, value, options | A drop-down; gives back what is picked. → value |
| `ui.sep!` |  | A separating line. |
| `ui.space!` | px | Empty space. |
| `ui.progress!` | amount | A bar filled from 0 to 1. |
| `ui.row!` | fn | Lays out what a function adds side by side. |
| `ui.column!` | fn | Lays out what a function adds one under another. |
| `ui.group!` | title, fn | A section that folds open and shut. |
| `ui.image!` | blob, width | One of the plugin's pictures. |

## draw

| word | takes | does |
| --- | --- | --- |
| `draw.line!` | x1, y1, x2, y2, color, width | A line over the stage, in stage pixels. |
| `draw.circle!` | x, y, r, color | A filled circle. |
| `draw.ring!` | x, y, r, color, width | A circle's outline. |
| `draw.rect!` | x, y, w, h, color | A filled rectangle. |
| `draw.box!` | x, y, w, h, color, width | A rectangle's outline. |
| `draw.text!` | x, y, text, color, size | Text over the stage. |
| `draw.path!` | points, color, width | Lines through [x, y] points. |
| `draw.poly!` | points, color | A filled convex shape. |

## math

| word | takes | does |
| --- | --- | --- |
| `random` | low, high | A random whole number between two whole numbers, or any number between others. → value |
| `round` | x | Rounds; a second value keeps that many decimals. → value |
| `floor` | x | Rounds down. → value |
| `ceil` | x | Rounds up. → value |
| `abs` | x | Without its minus. → value |
| `min` | a, b | The smallest of numbers or a list. → value |
| `max` | a, b | The biggest of numbers or a list. → value |
| `clamp` | x, low, high | Keeps a number inside a range. → value |
| `lerp` | a, b, t | In between two numbers. → value |
| `sqrt` | x | The square root. → value |
| `sin` | deg | Sine of an angle in degrees. → value |
| `cos` | deg | Cosine of an angle in degrees. → value |
| `atan2` | y, x | The angle of a direction, in degrees. → value |
| `hypot` | x, y | The distance to a point. → value |
| `num` | text | Reads a number from text, or null. → value |
| `rgb` | r, g, b | A colour from red, green and blue, 0 to 255. → value |
| `rgba` | r, g, b, a | A colour with how see-through it is. → value |
| `hsv` | hue, sat, val | A colour from hue in degrees, saturation and value from 0 to 1. → value |
| `mix` | a, b, t | Blends two colours. → value |
| `clock` |  | How long the plugin has been on. → value |
| `now` |  | Seconds since 1970, for dates. → value |

## text

| word | takes | does |
| --- | --- | --- |
| `str` | value | Any value as text. → value |
| `len` | value | How many letters, items or keys. → value |
| `upper` | text | In capital letters. → value |
| `lower` | text | In small letters. → value |
| `trim` | text | Without spaces at the ends. → value |
| `split` | text, by | Cuts text into a list. → value |
| `join` | list, with | Joins a list into text. → value |
| `replace` | text, old, new | Swaps every piece of text. → value |
| `contains` | where, what | Whether text, a list or a map has something. → value |
| `starts` | text, start | Whether text begins so. → value |
| `slice` | value, from, to | A piece of text or a list. → value |

## lists

| word | takes | does |
| --- | --- | --- |
| `push!` | list, item | Adds to the end of a list. |
| `pop!` | list | Removes the last item and gives it back. → value |
| `insert!` | list, at, item | Puts an item in a list. |
| `remove!` | list, at | Removes an item and gives it back. → value |
| `clear!` | list | Empties a list or map. |
| `sort!` | list | Sorts a list; a function picks what to sort by. |
| `shuffle!` | list | Mixes a list up. |
| `map` | list, fn | A new list of each item put through a function. → value |
| `filter` | list, fn | A new list of the items a function says yes to. → value |
| `sum` | list | Adds a list of numbers. → value |
| `first` | list | The first item. → value |
| `last` | list | The last item. → value |
| `pick` | list | Any one item. → value |
| `range` | from, to | A list of numbers, without the last. → value |
| `keys` | map | A map's keys. → value |
| `values` | map | A map's values. → value |
| `has` | map, key | Whether a map has a key. → value |
| `copy` | value | A copy that does not share its lists. → value |
| `json` | value | Any value as json text. → value |
| `parse_json` | text | Reads json text. → value |

## files

| word | takes | does |
| --- | --- | --- |
| `file.read` | path | Reads a text file. → value |
| `file.write!` | path, text | Writes a text file, replacing it. |
| `file.append!` | path, text | Adds to the end of a text file. |
| `file.exists` | path | Whether something is there. → value |
| `file.list` | folder | What is in a folder. → value |
| `file.delete!` | path | Deletes a file. |
| `file.folder!` | path | Makes a folder and those above it. |
| `file.dir` |  | A folder for this plugin's files. → value |
| `file.home` |  | Your home folder. → value |
| `settings.get` | path | One of q0editor's own settings, like theme.accent or autosave_enabled. → value |
| `settings.set!` | path, value | Changes one of q0editor's settings. |
| `settings.all` |  | All of q0editor's settings as data. → value |
| `clip.copy!` | text | Puts text on the clipboard. |

## plugin

| word | takes | does |
| --- | --- | --- |
| `say!` | text | Shows a bubble from the plugin and writes to its console. |
| `print!` | text | Writes to the plugin's console. |
| `plugin.name` |  | This plugin's name. → value |
| `plugin.id` |  | This plugin's id. → value |
| `plugin.stop!` |  | Switches the plugin off after this handler. |
| `plugin.open!` | title | Opens one of the plugin's windows. |
| `plugin.close!` | title | Closes one of the plugin's windows. |
| `plugin.tool!` | name | Makes the plugin's tool the one in use; null puts it away. |
| `plugin.blob` | name | A blob of the plugin as text. → value |
| `config.reset!` |  | Puts every setting back to its default. |
| `config.all` |  | Every setting as a map. → value |
