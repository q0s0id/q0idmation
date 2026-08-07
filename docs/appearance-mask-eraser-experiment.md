# appearance-mask-eraser experiment

this branch keeps the classic brush and eraser engine intact while adding a separate Advanced brush path. the appearance experiment no longer swaps in a reduced copy of `tools.rs`, so classic nibs, eraser nib sync and eraser preview remain on the original tool path.

## rollback

- switch back to `main`, or
- build q0editor with `--no-default-features` on this branch.

## data and compatibility

- classic vector paint still commits as fill-only raw geometry.
- q1s v9 introduced sparse vector appearance metadata; q1s v10 adds frozen material-source and post-material clip paths; q1s v11 adds an exact 2x3 affine for the already-resolved appearance field.
- q0s v8 carries the original appearance masks, q0s v9 carries post-material fragments, and q0s v10 carries the resolved-field affine into q0player.
- older q1s/q0s versions remain readable and load with no synthetic appearance state.
- writing an older format version with appearance data fails instead of silently dropping it.
- v1 migration creates no appearance metadata.

## appearance and eraser semantics

- classic brush paint is always plain vector fill and has no runtime material toggle. legacy Classic `Glow` settings migrate once into Advanced mode; Glow now belongs to Advanced materials.
- only the soft halo is rasterized from a gaussian alpha blur; the brush body remains the normal tessellated vector fill in q0editor.
- source fill alpha and halo alpha are resolved first. the erase mask is multiplied into that final material alpha afterwards.
- the mask eraser never subtracts from the source vector paths.
- the mask receives the exact classic eraser-nib coverage clipped to finite visible material support; halo radius is never added to the eraser footprint.
- regenerating the halo cannot paint back into an erased mask region.
- appearance metadata belongs to the vector asset inside the project, so undo/redo, save/reopen, nested q0rg rendering, transforms, export and q0player all see the same state.
- normal Select resolves the visible material surface: erased mask regions are not selectable, while finite glow support participates in hit-testing and bounds. Subselect still exposes only real vector anchors.
- ordinary assets without appearance metadata continue through the legacy vector renderer and classic geometry eraser.

this remains feature-gated while the visual/material model is being proven, but it is no longer runtime-only state.

- partial raw-fill split does not re-evaluate filters from each new contour. both fragments retain the pre-split material source and complementary post-filter clips; translating a fragment translates its material/erase/clip state together.
- a fragment's post-material clip is authoritative for rendering, hit-testing and selection bounds. hidden portions of the frozen source cannot start a drag, while a marquee containing only visible halo can still create and move a real fragment.
- raw move/scale/rotate/skew use one drag-start affine for the carrier vector and compose that same matrix into the resolved appearance field. the frozen material source is not skewed and blurred again: its already-resolved halo texture is rotated/scaled/sheared as a field, with clip and erase masks staying in that canonical field space.
- fragment rasterization is bounded around the post-material clip plus one finite filter radius instead of allocating a texture for the complete frozen source. editor halo quality is capped at 4 pixels per stage unit at extreme zoom so interactive cache rebuilds remain bounded.
- raw clipboard snapshots carry `vector + appearance` together. copy/paste/duplicate and Convert to q0rg therefore preserve frozen glow state under fresh asset ids instead of materialising clean vectors; halo-only marquee fragments keep an internal carrier plus their post-material clip.
- the selection hot path never rebuilds a soft-halo polygon buffer per frame: halo hit-testing uses distance to the finite source support, transform bounds use a conservative envelope, and selected glow uses the lightweight transform box instead of re-tessellating the filtered material.
- halo textures use transparent overscan and remain continuous underneath the real vector fill, avoiding raster/vector AA cracks. once a material source is frozen, affine edits reuse that canonical halo texture; changing the frozen source/clip/erase content invalidates it.
