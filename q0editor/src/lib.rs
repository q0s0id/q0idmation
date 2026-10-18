pub mod advanced_brush;
pub mod app;
#[cfg(feature = "appearance-mask-eraser")]
pub mod appearance;
#[cfg(windows)]
pub mod assoc;
pub mod audio;
pub mod autosave;
mod bitmap_import;
pub mod brush;
mod brush_geometry;
pub(crate) mod color_picker;
pub mod easing;
pub mod export;
pub(crate) mod fast_preview;
pub mod file_io;
mod font_replacement;
pub(crate) mod frame_edit;
pub mod hotkeys;
pub mod l10n;
pub mod launch;
pub mod library_transfer;
pub mod panels;
pub mod perf;
pub mod physics;
pub mod platform;
pub mod q0enc;
pub mod q0lang;
pub mod release_feed;
pub mod render;
pub mod rigging;
pub mod selection_edit;
pub mod settings;
pub mod single_instance;
pub mod state;
mod system_clipboard;
mod text_fonts;
pub mod text_tool;
pub mod theme;
pub mod timeline_edit;
mod touch;
mod ui;
mod video_preview;
pub mod workspace;

pub mod tools;

pub use app::EditorApp;
