# advanced brush

q0editor has two deliberately separate brush engines.

## classic

classic is the flash/animate-style vector brush: static nib, continuous sweep, boundary smoothing, fill-only raw graphics. it does not contain stabilizer, taper, velocity dynamics or raster materials.

## advanced

advanced shares only low-level pointer input with classic. each sample can carry pressure and time. the engine applies a trailing stabilizer, centre-line smoothing, pressure/velocity size dynamics, start/end taper, elliptical tip roundness and fixed/trajectory angle, then commits one fill-only vector surface. sparse input samples are bridged continuously.

advanced live preview is emitted as `egui::Mesh`; with q0editor's current `eframe/glow` backend those meshes are rasterized by the gpu through opengl. this is gpu preview/compositing, not gpu compute. authoritative commit geometry remains deterministic cpu/vector data so save/export/tests do not depend on a graphics driver.

Glow is an Advanced material. the live preview uses gpu mesh passes; committed projects use the existing canonical `VectorAppearance::SoftHalo` renderer, post-material masks and q0s/q0player persistence. disabling `appearance-mask-eraser` makes the material request fall back to plain vector paint.

## presets

advanced settings are global editor preferences. built-ins currently include Advanced Ink, Soft Glow, Calligraphy and Dynamic Taper. custom presets persist across projects and support save/update, rename and delete.

future Advanced-only extensions should live in this engine instead of growing Classic: textured/custom tips, flow/hardness, scatter/spacing, pressure-opacity, richer effect stacks and optional gpu filter compute.
