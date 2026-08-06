//! GUI scaffolding for the q0player app — kept out of `lib.rs` so the
//! library half (the headless `Player`) stays embeddable for tests.

pub mod analytics;
pub mod app;
mod atomic_file;
pub mod credits;
pub mod recent;
pub mod render;
pub mod settings;
pub mod theme;

#[cfg(windows)]
pub mod assoc;

pub use app::PlayerApp;
