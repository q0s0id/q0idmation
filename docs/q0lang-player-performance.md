# q0lang player performance

## reported case

`out/zagotovka.q0s`: vector character, five translucent trail instances,
18 scripted particles, vector labels and three interactive modes.

The repeated work was in the player host and renderer:

- group opacity sent each moving trail through the software rasterizer;
- unchanged vector paths and text were flattened/tessellated every paint;
- object globals repeatedly flattened the same asset geometry for bounds.

## changes

- cache authored solid fill/stroke meshes in local coordinates; transforms,
  zoom and tint update their vertices without retessellating paths;
- compose plain vector groups with opacity/blend using the existing GPU mesh
  group compositor; opacity still applies once to overlapping children;
- cache immutable asset bounds per loaded project, while recomputing instance
  transforms, nested playback, effect bounds and rig deformation dynamically;
- use each placement's runtime child frame/state in native drawing and bounds;
- apply a solid mesh's uniform tint once rather than once per vertex.

Complex appearances, gradients, masks, media, nested group effects and rigged
contents keep their existing rendering paths. Nonuniform stroke transforms
also use the existing renderer. Project changes invalidate cached geometry;
the runtime bounds cache is rebuilt when its project Arc changes.

## measurement

`out/q0lang-showcase-tools/src/bin/profile.rs` loads the actual movie and runs
100 fixed 60 Hz ticks in each mode, discarding the first 20. It invokes the
player, native egui paint function and egui tessellation at 1440 x 900.

Raw captures: `out/q0lang-showcase/performance-before.txt` and
`out/q0lang-showcase/performance-after.txt`.

Before: mean player tick 6.9–7.3 ms; mean native paint preparation 92.7–96.1 ms.
After: mean player tick 1.2–1.4 ms; mean native paint preparation 1.7–3.6 ms.
The scene/scripts were not simplified.

| mode | tick before / after, ms | paint before / after, ms | egui tessellation before / after, ms |
| --- | --- | --- | --- |
| physics | 6.882 / 1.379 | 92.689 / 3.614 | 3.991 / 3.952 |
| magnet | 7.158 / 1.215 | 95.075 / 1.701 | 3.348 / 2.885 |
| orbit | 7.298 / 1.192 | 96.112 / 1.711 | 3.059 / 3.024 |

All three final runs reported zero script diagnostics. `cargo build --release
--locked -p q0editor -p q0player --target-dir target` completed successfully.

These are CPU timings, not measured screen FPS. The profiler does not execute
OpenGL callbacks, present a window or include GPU/backend costs. Release
editor/player builds include the fix; native visual playback remains to be
confirmed in the user session.

## reproduce

```powershell
cargo run --manifest-path out/q0lang-showcase-tools/Cargo.toml --bin profile --target-dir target --offline --release
```

Use `--bin profile`: the other executable in this utility package rebuilds
the authored showcase rather than profiling its existing exported movie.
