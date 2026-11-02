# Simple mode

[home](README.md) · [syntax](syntax.md) · [host api](hosts.md)

Every q0lang editor in q0editor (a q0rg's script window, frame code, and `.q0l` tabs) has a SIMPLE / PRO switch at the top left. Pro is the text. Simple is the same code as blocks: square slabs on black, one column, top to bottom in the order the code runs, with the line numbers of the text down the side. Both edit the same script; switch whenever you like. The editor remembers which one you used last.

## reading the blocks

| colour of the spine | blocks |
| --- | --- |
| red | FLOW: `IF`, `ELSE`, `WHILE`, `FOR`, `DO`, `BREAK`, `NEXT ROUND` |
| orange | DATA: `SET … TO`, `CHANGE … BY`, variables |
| green | MATH: operators, comparisons, `NOT`, `SIN`, `CLAMP`, `LERP` and the rest |
| purple | FUNC: functions (their header is purple, words in black), `CALL`, `RETURN`, `ON mouse.click CALL` |
| cyan | STAGE: going to frames, scenes, named instances, `input.*`, `mouse.*`, `self.*` |
| lime | LAYER: hiding, shifting, fading and walling layers, and `TOUCHES` |
| white | WINDOW: fullscreen, the player's UI, F9 and F4, moving and sizing the window |
| indigo | FX: particles, lights, physics, rigs |
| grey | MISC: imports, notes, printing |

Values sit in slots: numbers in blue, text in yellow quotes, `true` and `false` in pink. Questions that are true or false have pointed ends. A `# comment` is a note block in dim yellow. A line the blocks do not know shows as a dark red block with the line as it was typed.

## working with them

- Drag a block in from the palette on the left; the lime line shows where it will land. It lands in the nearest gap, inside an `IF` or a function too.
- Drag a block in the column to move it, with everything inside it. Drag it back onto the palette, or point at it and press Delete, to remove it.
- Drag a value block onto a slot to put it there; a slot with a value block in it takes another one the same way.
- Click a slot to type into it. A number slot takes any expression, `speed * 2` too, and turns it into blocks. A text slot keeps what you type as text. Layer, instance, emitter and light slots suggest the names in your project while you type.
- Click a name with ▾ to pick another: an operator, a variable, `SET` or `CHANGE`, `AND PLAY` or `AND STOP`, the mouse event of an `ON` block.
- `+` after `IF` adds an `ELSE`; `−` takes an empty one away. `RETURN` gets or drops its value the same way.
- Click a red block's line to retype it; when the line reads as code it turns into a block.
- Ctrl and the wheel zoom; the wheel scrolls; dragging the empty floor scrolls too.

Blocks that need a library bring their `import` along when they land: a layer block adds `import q0.layer`, `input.left` adds `import q0.input`.

## the text Simple mode writes

The blocks are the code, so changing a block writes the script anew in the plain style: two spaces for each level, one statement a line, parentheses only where they change the meaning, a blank line around each function. A comment at the end of a line moves onto its own line above. Looking at the blocks changes nothing; only an edit writes.

Code with a mistake the parser reports does not show as blocks, so nothing is lost: Simple mode says which line is wrong; fix it in Pro and come back. Lines with an issue the checks report (an unknown layer function, a `break` outside a loop) get a red edge and a red line number.

Sources: [blocks](../../q0editor/src/q0lang/blocks.rs), [printer](../../q0s-format/src/q0lang/printer.rs).
