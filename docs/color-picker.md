# the colour picker

Every colour box in q0editor (fills, strokes, gradient stops, effects, text,
the stage background, the theme editor, 3D materials and physics) opens the
same picker.

## on top

- **old | new**: the colour the box had when you opened it, and the one you
  have now. Click the old half to go back to it.
- **hex**: type 3, 6 or 8 digits, with or without `#`. The last two of eight
  are the opacity. The colour follows as you type.
- **square / wheel**: a big saturation and brightness square with a hue
  slider under it, or a hue ring around a smaller square. The choice is
  remembered.
- **eyedropper**: click anywhere outside the picker, even in another window,
  to take that pixel's colour (see [screen color sampling](screen-color-picker.md)).
  On a phone or tablet, tap the eyedropper first, then tap where the colour is.

The checkered slider under the field sets the opacity.

## tabs

- **Sliders**: red, green and blue, or hue, saturation and value, or hue,
  saturation and lightness. Each track shows the colours it leads to from
  where you are, and the numbers can be dragged or typed.
- **Harmony**: colours that go with the current one, by rule: complement,
  analogous, triad, split complement, square, or shades of the same hue.
  Click one to use it; "Save these to my swatches" keeps them all.
- **Swatches**: the colours you picked recently, your saved colours (the
  dashed + tile saves the current one, right-click removes one), and the
  colours the project's drawings already use, most used first.

Recent and saved colours, and which field and tabs you prefer, are kept in
`colors.json` next to the editor's `settings.json`.

## gradients

A fill's picker switches between solid, linear and radial paint. Click the
ramp to add a stop, drag a stop's pin to move it, drag it down or
double-click it to remove it. The picker under the ramp edits the selected
stop.

## none

Fills and strokes can be **none**: the last choice in the row on top of
their picker (Solid, Linear, Radial, None for a fill; Solid, None for a
stroke). A box set to none shows white with a red line across it. Picking a
colour, or a pixel off the screen, turns none off again, and the colour it
had before comes back.

The fill and the stroke can't both be none, because a shape has to keep one
of them. While one is none, the other's None is greyed out. A rectangle or
oval drawn with no stroke is all fill, and one drawn with no fill is all
outline. Lines (pencil, line tool, open pen paths) are nothing but stroke,
so they aren't drawn while the stroke is none. The paint bucket always
pours, using the fill's last colour.

## fill and stroke in the toolbar

Under the tools sit two swatches: the fill, a solid square, and the stroke,
a hollow frame below and to the right of it. The one you're editing is
drawn in front with an accent edge. Click either to open its picker. The
three small buttons under them are:

- **default colours**: black stroke, white fill
- **swap**: the fill and stroke trade colours
- **none**: no fill or no stroke, whichever is active

## the Color panel

Layout > Panels > Color (at the right end of the menu bar). It starts as a
tab beside Properties, and layouts saved before it existed get that tab
once. From the top:

- the fill and stroke selectors, the same three small buttons, and the
  square and wheel buttons (the same choice as in the picker);
- the kind: Solid, Linear, Radial or None (strokes are solid or none);
- a gradient's stops, when it is one, where the fields below edit the
  selected stop;
- a saturation and brightness square with a hue strip beside it, or the
  hue wheel around a smaller square;
- H, S and B (degrees and percent), R, G and B (0 to 255), and A, the
  opacity;
- the hex code and a preview of the paint;
- the swatches. Click one to use it and right-click to remove it. **Add to
  swatches** keeps the current colour. These are the same saved colours as
  the picker's Swatches tab.

## painting the selection

While something is selected, the toolbar swatches and the Color panel show
its paint. Changing them repaints it, and sets the tools' colour as well.
This covers raw fills and lines, and drawings placed as objects. A drawing
that other keyframes or objects share is copied first, so only what's
selected changes. Symbols, bitmaps, text and drawings you named in the
library keep their own paint.

One change is one undo step: a drag across the square, a picker kept open
while you try colours, a click on a swatch. None only takes a fill or stroke
off where the drawing keeps the other. Raw fills and lines on their own
stay, and Delete removes them.

With nothing selected, these controls set only the colours new shapes and
lines are drawn with. The tool sections in Properties set the same tool
colours, and never touch the selection.
