//! The seam between the editor and the platform it runs on.

#[cfg(target_os = "android")]
mod android;
mod text_input;
#[cfg(any(target_os = "android", test))]
mod workspace;

#[cfg(target_os = "android")]
pub(crate) use android::FileDialog;
#[cfg(not(target_os = "android"))]
pub(crate) use rfd::FileDialog;
pub use text_input::{ImeAction, ImeTranslator, SoftKeyboard};
