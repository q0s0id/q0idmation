# artwork outside the document paper

the document rectangle defines the paper and the output size, not a limit on
world coordinates. the editor's canvas painter defines what is currently
visible. the camera can frame artwork on either side of the original paper.

- gradient and lighting gpu callbacks cover the visible canvas. lighting still
  runs after its configured layer, leaving layers above it unchanged.
- the software mask fallback covers the canvas with an overscan tile, including
  when the original paper is completely off screen.
- the authoring raster entry point uses the supplied transform exactly. movie
  raster entry points apply the project camera once; the editor controls its own
  edit-in-place transform and animation preview camera.
- a lit software mask tile includes the union of the paper and the camera
  background polygons, so darkness affects the background outside the original
  stage too. overlapping polygons are filled once; onion skins and partial
  renders remain transparent and unlit, as in the gpu path.
- lines, rectangles, ovals, lasso starts and text accept finite coordinates
  outside the paper. primitive endpoints are no longer clamped to document
  dimensions, so shift constraints work across the boundary.

these changes do not change project data or the output dimensions of a movie.
