# egui-winit 0.27.2, patched

Unmodified crates.io release of egui-winit 0.27.2 (MIT OR Apache-2.0,
https://github.com/emilk/egui) with one change, marked `q0s:` in
`src/lib.rs`: key events fall back to the physical key when the layout's
logical key has no Latin name. Without it, shortcuts did nothing on Russian
and other non-Latin keyboard layouts. egui 0.28 ships the same fallback, so
this folder and the `[patch.crates-io]` entry in the workspace `Cargo.toml`
go away when the workspace moves to egui 0.28 or later.
