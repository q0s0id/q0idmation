# egui_dock 0.12.0, patched

The crates.io release of egui_dock 0.12.0 (MIT,
https://github.com/Adanos020/egui_dock), with hooks that let q0editor draw
the dock its own way, animated. Every change is marked `q0s:`.

- `src/q0s.rs` (new): `TabLook`, what one tab button is this frame, and
  `Overlay` / `take_overlay`, what the drop overlay would show.
- `TabViewer` (`src/widgets/tab_viewer.rs`): `tab_leading_width` (room for
  an icon before the title), `paints_tabs` and `paint_tab`. With
  `paints_tabs` on, the dock still lays tabs out and takes their clicks and
  drags but draws nothing of them (`show/leaf.rs`).
- `OverlayStyle::q0s_custom` (`src/style.rs`): the dock draws none of the
  drop overlay and records it instead (`drag_and_drop.rs`). With icon
  buttons it also records the area a split would take, which upstream only
  hints at inside the buttons.
- Separators ease into their hover and drag colours and swell while held
  (`show/mod.rs`).
- Stroke widths written as `1.0_f32`, not `1.0`, where newer compilers warn
  about the float literal falling back to `f32` (not marked `q0s:`).

Without those switches it behaves as upstream. The folder and the
`[patch.crates-io]` entry in the workspace `Cargo.toml` go when q0editor
draws the dock some other way or upstream grows the same hooks.
