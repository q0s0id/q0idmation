# Shape tweens

A shape tween morphs the drawing on one keyframe into the drawing on the next:
outline, fill colour, stroke colour and stroke width all change gradually.
Use a motion tween to move one object; use a shape tween to turn one drawing
into another.

![A star morphing into a ring over seven frames](shape-tween-demo.png)

*Seven frames of one tween, rendered by the export rasterizer: the outline,
both colours and the stroke width blend, and the ring's hole opens from the
centre.*

## Creating one

1. Draw on a keyframe.
2. Create a later keyframe on the same layer (F6 or F7) and draw the shape it
   should become.
3. Select any frame between the two and choose **Create shape tween** from the
   timeline context menu, or press **Ctrl+Alt+S**. The shortcut can be changed
   in Settings > Keyboard shortcuts.

The span turns green in the timeline. Motion tweens keep their usual colour.

Both keyframes need drawn vector shapes: brush strokes, pencil lines, shapes
and bucket fills. Symbols, bitmaps and text on the same keyframe are left
alone and simply held for the span; break them apart first if they should
morph.

To change the pacing or remove the tween, select a frame inside it and use the
tween section in Properties. Shape tweens accept the same easing presets and
custom curves as motion tweens.

## How shapes are matched

There are no shape hints. The match is worked out from the drawings:

- Shapes pair up by position and size, closest and most similar first.
- If the next keyframe has more shapes, the nearest shape splits; if it has
  fewer, the spare shapes merge into their nearest neighbour.
- Inside a shape, outlines pair by area, so an outer edge morphs into an outer
  edge and a hole into a hole. An outline with no partner grows out of, or
  shrinks into, the centre of the other shape.
- Drawing direction and the point where an outline starts do not matter.
- A fill or stroke that exists on only one side fades in or out.
- If the next keyframe is blank, the shapes shrink and fade away.

When a morph takes an unexpected route, add a keyframe in the middle and
correct the drawing there.

## Keyframes inside a tween

Pressing **F6** on a frame inside a shape tween creates a keyframe holding the
in-between drawing exactly as it was shown. The tween now ends at that
keyframe; the stretch after it is a plain hold until you tween it again.

Editing artwork on an in-between frame does the same thing first, like any
edit on a held frame.

Removing the keyframe a tween points to removes the tween.

## Where it plays

Shape tweens play in the editor stage, outline mode, Animation Preview, Fast
Preview, q0player and every q0enc export (q0s, MP4, q0v, PNG sequence, GIF,
still images).

## FLA and XFL import

Animate frames with `tweenType="shape"` import as shape tweens, including
classic ease values and custom ease curves. Animate shape hints are not read.
Motion tweens import as before. The legacy binary FLA reader does not import
tweens of either kind.

## File compatibility

A project that contains a shape tween is saved as **q1s v39** and exported as
**q0s v38**. Editors and players older than this version cannot open those
files. Projects without shape tweens keep their previous file version and
still open in older builds.

## Limits

- Gradient fills and mask-eraser appearance are not morphed; in-between frames
  use the flat fill colour.
- Placement effects other than tint and opacity (blur, glow, shadow, blend
  modes) are not applied to in-between frames in the editor stage and
  q0player. Exports apply them.
- On an in-between frame, clicking and hovering still follow the first
  keyframe's outline, not the morphed one.
- In the editor, a layer that masks or is masked while it morphs is drawn by
  the slower raster path for those frames.
- Rig bindings do not apply to in-between shapes.
