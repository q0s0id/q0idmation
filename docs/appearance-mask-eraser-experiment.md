# appearance-mask-eraser experiment

this branch keeps the classic brush and eraser engine as the only tool path. the appearance experiment no longer swaps in a reduced copy of `tools.rs`, so brush nibs, eraser nib sync and the in-progress eraser preview all use the same code as the classic editor.

## rollback

- switch back to `main`, or
- build q0editor with `--no-default-features` on this branch.

## data and compatibility

- classic vector paint still commits as fill-only raw geometry.
- q1s v9 stores sparse vector appearance metadata: material plus asset-local erase-mask paths.
- q0s v8 carries the same appearance data into q0player.
- older q1s/q0s versions remain readable and load with no synthetic appearance state.
- writing an older format version with appearance data fails instead of silently dropping it.
- v1 migration creates no appearance metadata.

## appearance and eraser semantics

- soft halo is rendered from a gaussian alpha blur rather than a fixed stack of buffered polygons.
- source fill alpha and halo alpha are resolved first. the erase mask is multiplied into that final material alpha afterwards.
- the mask eraser never subtracts from the source vector paths.
- the mask receives the exact classic eraser-nib coverage clipped to finite visible material support; halo radius is never added to the eraser footprint.
- regenerating the halo cannot paint back into an erased mask region.
- appearance metadata belongs to the vector asset inside the project, so undo/redo, save/reopen, nested q0rg rendering, transforms, export and q0player all see the same state.
- ordinary assets without appearance metadata continue through the legacy vector renderer and classic geometry eraser.

this remains feature-gated while the visual/material model is being proven, but it is no longer runtime-only state.
