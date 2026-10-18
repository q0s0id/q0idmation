//! Android services that q0editor and q0player share.
//!
//! Both apps run in a subclass of `com.q0id.android.Q0Activity`, which does
//! what the native side cannot: it shows the system document picker, takes
//! files other apps hand over, writes saved documents back to where they came
//! from and catches the Back button. What it reports comes out of
//! [`take_events`]; what the native side asks of it goes through the
//! functions below.

#![cfg(target_os = "android")]

mod activity;
mod startup;

pub use activity::{
    create_document, document_written, export_finished, export_folder_finished, immersive,
    keep_screen_on, leave, open_in_player, pick_document, set_waker, take_events, Event,
    Mp4Encoder, EXTERNAL, PREVIEW_ALL_SCENES, PREVIEW_SCENE,
};
pub use startup::{adopt_app_storage, choose_multisampling, egl_offers_multisampling};
