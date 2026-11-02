# Spriting tools

Tools for changing drawn sprites in place: breaking a pan, denting a hat or
wounding a shield without redrawing them as new symbols.

## Snapping

The magnet next to the zoom at the top of the stage, **View > Snapping** or
**M** turns it on and off. While it is on, points pull onto anchors, line ends, object origins and the
stage's corners and middle within 10 screen pixels, or failing that onto the
nearest line. It applies to the line, rectangle, oval and pen tools, to things
dragged with the Select tool (the dragged thing's point nearest the grab is what
snaps), to Subselect's points and to a line's dragged end. A ring marks a snap
to a point and a square a snap onto a line. Hold Shift to drag without snapping.

## Lines

- New lines are cut where they cross or meet other lines on the layer, so each
  piece between crossings can be picked, moved, recoloured or deleted on its
  own. Turn this off with **Cut lines where they cross** in the Line or Pen
  tool options. A filled shape's outline is not cut; its sides are picked one
  by one as before.
- Click a line to pick that piece. Double-click it to pick every line joined
  to it.
- A picked line shows dotted over its whole painted width, like a picked
  fill. With Subselect it shows the line it runs along and its points.
- Dragging the middle of a straight line with Select or Subselect bends it
  into one even arc whose ends lean towards the bend. Curves that are not
  plain arcs keep their shape and are pulled along.

## Editing what is picked

With strokes or fills picked, the Properties panel edits them in place:
stroke colour, opacity, width, ends (round, flat, square) and corners (round,
sharp, bevel); fill colour and opacity. Parts that share a drawing with
unpicked parts are lifted into their own drawing first, so only what is picked
changes. Mixed styles are shown by the first one and made the same once edited.

## Subselect

Drag over empty stage to pick every point inside the box, on as many lines and
shapes as it covers; Shift adds to what is picked. Drag one of the picked points
to move them all.

## Rectangle, oval and line

Hold Alt, or tick **Draw from the middle** in the tool options, to grow the
shape both ways from where the drag starts. Shift still keeps rectangles and
ovals square and lines at 45 degrees, so Shift and Alt together draw a
centred square.

## Select without the transform box

Untick **Transform box** in the Select tool options and the Select tool only
moves things: no handles to catch by mistake.

## Paint bucket gaps

**Close gaps** in the bucket options fills outlines that are not quite closed:
small gaps are up to 3 pixels, medium 8 and large 18. Everything drawn on the
layer is thickened until such gaps shut, the enclosed hole is found and grown
back. When an area is closed anyway, the smaller of the two wins.

## Strokes to their own layer

**Modify > Strokes to Own Layer** moves the current layer's outlines to a new
layer just above it, named after it with "lines", keyed on the same frames and
kept in the same folder and under the same mask. Fills stay. Drawings with both
are split in two; drawings also used elsewhere are copied rather than changed.

## Knife

The knife (**C**) cuts what is drawn on the current layer along a dragged line;
Shift cuts straight. Fills fall into pieces that can be picked, moved and
deleted on their own, and outlines open along the cut. A filled shape the cut
reaches into has its fill and outline parted. **Gap** sets how wide a crack the
cut leaves, 0 only parting the pieces, and **Jagged crack** cuts along a zigzag
that follows the line drawn. One cut is one undo step.

## Variants

**Make Variant** in the Properties panel of a symbol instance copies its symbol
as a variant of it, shows the copy in that instance and opens it in place, ready
to be broken or wounded. Variants are named after the symbol with a tilde and a
number, such as "Pan ~2"; they can be renamed to "Pan ~broken" and stay in the
family as long as the part before the tilde is the same. The **<** and **>**
buttons next to **Variant** show the previous or next one in that instance,
keyed on the current frame.

## Lights and particles on turned objects

Lights and particle emitters that follow an object now keep their offset in the
object's own space: they swing round, stretch and flip with it, and a spot
light's aim and an emitter's direction and shape turn with it. Relative
particles turn with the object as they go. Offsets saved before were measured
on the stage; on followed objects that are already turned, scaled or flipped,
lights and emitters move to where the object's turn now carries them.
