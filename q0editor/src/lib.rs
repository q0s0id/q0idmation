pub mod app;
#[cfg(windows)]
pub mod assoc;
#[cfg(feature = "appearance-mask-eraser")]
pub mod appearance;
mod bitmap_import;
pub mod brush;
pub mod easing;
pub mod export;
pub mod file_io;
pub mod l10n;
pub mod panels;
pub mod q0enc;
pub mod q0lang;
pub mod render;
pub mod selection_edit;
pub mod settings;
pub mod state;
pub mod theme;
pub mod timeline_edit;

#[cfg(not(feature = "appearance-mask-eraser"))]
pub mod tools;
#[cfg(feature = "appearance-mask-eraser")]
#[path = "tools.rs"]
mod tools_legacy;
#[cfg(feature = "appearance-mask-eraser")]
#[path = "tools_experimental.rs"]
pub mod tools;

pub use app::EditorApp;
