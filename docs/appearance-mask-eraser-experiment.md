# appearance-mask-eraser experiment

this branch keeps the trusted classic brush/eraser path intact and routes only q0editor through an optional cargo feature.

## rollback

- switch back to `main`, or
- build q0editor with `--no-default-features` on this branch.

`q0s-format` is unchanged. `.q1s` serialization is unchanged. experimental brush appearance metadata is runtime-only and is intentionally not written to project files yet. reopening a project therefore falls back to the ordinary solid vector appearance until a persistent format is designed and compatibility-tested.

## experiment semantics

- classic vector paint still commits as fill-only raw geometry.
- an editor-only finite soft halo is associated with the resulting raw asset.
- the eraser hit-tests the visible halo support, not only the source polygon.
- for a material support radius `r`, the source cut is the visible eraser region buffered by `r`. after the halo is regenerated, the original visible erase region cannot receive halo coverage again.
- solid assets use radius zero and therefore keep the exact classic boolean subtraction path.

this is a reversible feasibility branch, not a format migration.
