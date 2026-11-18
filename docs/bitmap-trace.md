# trace bitmap

Trace Bitmap turns a bitmap on the stage into filled vector shapes, one per
colour. Select the bitmap, then pick **Modify → Trace Bitmap…**, right-click
it on the stage, or use the button under Break Apart in Properties.

The shapes land on the same layer and frame, exactly where the bitmap was,
as raw graphics (the way Break Apart leaves a vector). The bitmap stays in
the library, and Undo brings the placed bitmap back.

## settings

- **Colours** (2–255): how many colours the picture is reduced to. Each one
  becomes one shape. The swatches under the settings show the colours you
  will get, and follow the slider.
- **Opacity cutoff**: pixels less opaque than this are left out. Traced
  shapes are always fully opaque.
- **Smallest patch**: patches of fewer pixels than this join a colour next
  to them. It clears up noise, JPEG speckle and the soft fringe of
  antialiased edges. A speck with only transparent pixels round it is
  dropped, and a pinhole of transparency inside a shape is filled. 1 keeps
  every pixel.
- **Curve fit**: how far, in pixels, curves may stray from the pixel
  outline. Higher is smoother with fewer points. **0** keeps the exact pixel
  edges, for pixel art.
- **Sharp corners**: straight edges at least this many pixels long meet in
  a sharp corner instead of a rounded one. 0 rounds every corner.
- **Defaults** puts the settings back. The dialog remembers what you last
  traced with until q0editor closes.

The dialog follows the selection: click another bitmap on the stage and it
traces that one instead.

## how it works

1. The colours are reduced with k-means over a 15-bit colour histogram, so
   the cost of picking colours does not grow with the size of the picture.
2. Each pixel takes the nearest colour, then patches below the smallest
   size are folded into a neighbour.
3. Each colour's outline is walked along the pixel edges. Where two
   patches of a colour touch only at a corner, they stay separate shapes.
4. Stair steps are smoothed by running the outline through the middle of
   each pixel edge, and curves are fitted with the same fitter the brush
   uses.

The shapes are **stacked**, most used colour at the bottom, and each shape
reaches a pixel or two under the colours above it where they meet. Shapes
fitted one at a time never quite agree on a shared edge; the colour
underneath shows there instead of a crack. Moving a shape away shows that
small rim.

## limits

- Big bitmaps take a moment; the trace runs when you press Trace.
- Gradients become bands of flat colour; more colours, thinner bands.
- Instance effects (opacity, blend, filters) on the bitmap must be reset
  first, as with Break Apart.
