# raster layers

A raster layer is a layer whose keyframes hold drawings made of pixels
instead of vector shapes. Everything else about it is an ordinary layer:
keyframes, holds, blank keyframes, onion skin, masks, symbols, tweens,
export and the player all work on it as they do on any other layer.

## making one

- The timeline's **Add raster layer** button (the sheet of pixels, next to
  Add layer), or **New raster layer** in a layer's right-click menu.
- An empty layer can switch with **Make it a raster layer** (and back) in
  its right-click menu.

Raster layers show a small grid of pixels as their icon. As soon as the
project has one, the toolbox grows a **Raster** section, the way 3D tools
appear once there is 3D.

## the tools

On a raster layer, the usual tool keys and buttons pick their raster
versions: Brush and Pencil paint, Eraser erases, the Bucket fills, the
Eyedropper picks and the Lasso selects pixels.

- **Raster brush**: paints with the *stroke* colour. The Properties panel
  shows a library of brushes, each tile painted by the brush itself in your
  colour: pencil, rough sketch, ink pen, fineliner, brush pen, calligraphy,
  marker, hard and soft round, airbrush, watercolour, gouache, dry brush,
  chalk, charcoal, crayon, spray paint and speckle. Change anything about a
  brush and the change stays with that brush (a `*` marks it; Reset takes it
  back).
  - `[` and `]` size the brush. Shift-click draws a straight line from where
    the last stroke ended. Alt-click picks a colour into the stroke swatch.
  - Pen pressure thins and lightens strokes as each brush says. The
    stabilizer trails the nib on a rope for steady lines; tapers give
    pointed ends even with a mouse.
  - **Paint behind** puts paint under what is already there; **Keep
    transparency** paints only where there is paint (recolour lines without
    spilling).
- **Raster eraser**: hard, soft, pencil, kneaded and big erasers.
- **Smudge**: finger and blender drag the paint along; soften blurs it.
- **Raster fill**: pours the *fill* colour.
  - **Close gaps**: lines with gaps up to this wide still hold the paint.
  - **Under the lines**: the colour reaches a few pixels under the lines'
    soft edges, so no pale halo is left.
  - **Lines on other raster layers count**: keep the colour on its own
    layer under the ink and still fill between the ink's lines.
- **Colour picker**: takes the colour under the click into the active
  swatch, from every layer or only the current drawing.
- **Rectangle select** and **Lasso select**: Shift adds, Alt takes away.
  Drag inside the selection to lift and move the pixels; drag a corner to
  size them (Shift keeps the shape), just outside a corner to turn them
  (Shift steps by 15 degrees). Enter, Escape or a click outside puts them
  down. Delete clears; Copy, Cut and Paste carry pixels to another frame,
  where they land in the same place.

## frames

Painting on a frame that holds an earlier keyframe changes that keyframe's
drawing, as drawing on a held vector frame does. On a blank keyframe, or
past the layer's end, painting makes a new drawing. A keyframe copied from
another (F6) shares its pixels until you paint on it; then only the parts
you touch are copied.

## files

Projects with raster save as `.q1s` version 49. Pixels are stored in
64 x 64 tiles, only where something is drawn, compressed, and a tile that
repeats anywhere in the file is stored once, so held and duplicated
drawings cost almost nothing. Projects without raster still save as before.
Older editors refuse version 49 files rather than misread them.

## online rooms

Raster layers do not travel to online rooms yet: in a room the raster tools
say so instead of painting, and a project with raster cannot start a room.
Share the file instead.
