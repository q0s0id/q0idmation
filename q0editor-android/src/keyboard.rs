//! The on-screen keyboard, bridged through `MainActivity`.
//!
//! winit neither delivers typed text nor opens the keyboard on Android, so
//! both go through the activity instead. It owns an invisible text target,
//! reports what the input method does to the `nativeIme*` functions below, and
//! shows or hides the keyboard when asked.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use jni::objects::{JObject, JString, JValue};
use jni::sys::jint;
use jni::{JNIEnv, JavaVM};
use q0editor::platform::{ImeAction, ImeTranslator, SoftKeyboard};
use winit::platform::android::activity::AndroidApp;

/// Filled on the activity's UI thread, drained on the editor's.
static TYPED: Mutex<Vec<ImeAction>> = Mutex::new(Vec::new());
/// Height in pixels of the part of the surface the keyboard covers.
static COVERED: AtomicU32 = AtomicU32::new(0);
/// Lets the UI thread wake the editor when something here changed.
static EDITOR: OnceLock<egui::Context> = OnceLock::new();

fn wake_editor() {
    if let Some(editor) = EDITOR.get() {
        editor.request_repaint();
    }
}

fn report(action: ImeAction) {
    log::debug!("input method: {action:?}");
    if let Ok(mut typed) = TYPED.lock() {
        typed.push(action);
    }
    wake_editor();
}

#[no_mangle]
pub extern "system" fn Java_com_q0id_q0editor_MainActivity_nativeKeyboardCovers(
    _env: JNIEnv,
    _activity: JObject,
    pixels: jint,
) {
    COVERED.store(u32::try_from(pixels).unwrap_or(0), Ordering::Relaxed);
    wake_editor();
}

#[no_mangle]
pub extern "system" fn Java_com_q0id_q0editor_MainActivity_nativeImeCompose(
    mut env: JNIEnv,
    _activity: JObject,
    text: JString,
) {
    if let Ok(text) = env.get_string(&text) {
        report(ImeAction::Compose(text.into()));
    }
}

#[no_mangle]
pub extern "system" fn Java_com_q0id_q0editor_MainActivity_nativeImeCommit(
    mut env: JNIEnv,
    _activity: JObject,
    text: JString,
) {
    if let Ok(text) = env.get_string(&text) {
        report(ImeAction::Commit(text.into()));
    }
}

#[no_mangle]
pub extern "system" fn Java_com_q0id_q0editor_MainActivity_nativeImeFinish(
    _env: JNIEnv,
    _activity: JObject,
) {
    report(ImeAction::FinishComposing);
}

#[no_mangle]
pub extern "system" fn Java_com_q0id_q0editor_MainActivity_nativeImeDelete(
    _env: JNIEnv,
    _activity: JObject,
    before: jint,
    after: jint,
) {
    report(ImeAction::Delete {
        before: usize::try_from(before).unwrap_or(0),
        after: usize::try_from(after).unwrap_or(0),
    });
}

pub struct ActivityKeyboard {
    app: AndroidApp,
    shown: bool,
    translator: ImeTranslator,
}

impl ActivityKeyboard {
    pub fn new(app: AndroidApp) -> Self {
        Self {
            app,
            shown: false,
            translator: ImeTranslator::default(),
        }
    }

    fn set_shown(&mut self, shown: bool) {
        log::debug!("soft keyboard wanted: {shown}");
        self.shown = shown;
        if let Err(error) = self.ask_activity(shown) {
            log::error!("the activity did not take the keyboard request: {error}");
        }
    }

    fn ask_activity(&self, shown: bool) -> jni::errors::Result<()> {
        // SAFETY: the activity glue keeps both the virtual machine and its
        // reference to the activity alive for as long as `android_main` runs.
        let vm = unsafe { JavaVM::from_raw(self.app.vm_as_ptr().cast()) }?;
        let activity = unsafe { JObject::from_raw(self.app.activity_as_ptr().cast()) };
        let mut env = vm.attach_current_thread_permanently()?;
        let called = env.call_method(
            &activity,
            "setSoftKeyboardVisible",
            "(Z)V",
            &[JValue::Bool(shown.into())],
        );
        if called.is_err() {
            // A pending Java exception would fail every later call as well.
            let _ = env.exception_clear();
        }
        called.map(drop)
    }
}

impl SoftKeyboard for ActivityKeyboard {
    fn update(
        &mut self,
        ctx: &egui::Context,
        text_field_focused: bool,
        input: &mut egui::RawInput,
    ) {
        EDITOR.get_or_init(|| ctx.clone());

        // A tap while a field is focused brings back a keyboard the user
        // dismissed without leaving the field.
        let tapped = input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::PointerButton { pressed: true, .. }));
        if text_field_focused != self.shown || (text_field_focused && tapped) {
            self.set_shown(text_field_focused);
        }

        let typed = TYPED
            .lock()
            .map(|mut typed| std::mem::take(&mut *typed))
            .unwrap_or_default();
        for action in typed {
            self.translator.translate(action, &mut input.events);
        }

        let covered = COVERED.load(Ordering::Relaxed) as f32 / ctx.pixels_per_point();
        if let Some(screen) = input.screen_rect.as_mut() {
            screen.max.y = (screen.max.y - covered).max(screen.min.y);
        }
    }
}
