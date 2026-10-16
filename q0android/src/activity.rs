//! Requests to the activity, and what it reports back.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use jni::objects::{JObject, JString, JValue};
use jni::sys::{jint, jsize};
use jni::{JNIEnv, JavaVM};

/// The tag of a document another app handed over, as opposed to one picked
/// for a request of ours.
pub const EXTERNAL: i32 = 0;
/// q0editor handing q0player a snapshot to preview: the current scene only.
pub const PREVIEW_SCENE: i32 = -1;
/// The same, playing every scene in order.
pub const PREVIEW_ALL_SCENES: i32 = -2;

#[derive(Debug)]
pub enum Event {
    /// A readable copy of a document is ready at `path`. `tag` is the one
    /// given to [`pick_document`], or [`EXTERNAL`] or a preview tag for a
    /// document another app sent.
    Opened { tag: i32, path: PathBuf },
    /// Request `tag` ended without a document. The message is empty when the
    /// person just backed out of the picker.
    Failed { tag: i32, message: String },
    /// Something worth telling the person, such as a write-back that failed.
    Status(String),
    /// The Back button or gesture.
    Back,
}

/// Filled on the activity's threads, drained on the app's.
static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
/// Lets those threads wake the egui loop when something arrives.
static WAKER: Mutex<Option<egui::Context>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Remembers which egui context to wake when events arrive. Cheap enough to
/// call every frame.
pub fn set_waker(ctx: &egui::Context) {
    let mut waker = lock(&WAKER);
    if waker.is_none() {
        *waker = Some(ctx.clone());
    }
}

/// Everything the activity reported since the last call, oldest first.
pub fn take_events() -> Vec<Event> {
    std::mem::take(&mut *lock(&EVENTS))
}

fn push(event: Event) {
    log::debug!("activity event: {event:?}");
    lock(&EVENTS).push(event);
    if let Some(ctx) = lock(&WAKER).as_ref() {
        ctx.request_repaint();
    }
}

fn string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    env.get_string(value).ok().map(String::from)
}

#[no_mangle]
pub extern "system" fn Java_com_q0id_android_Q0Activity_nativeDocumentOpened(
    mut env: JNIEnv,
    _activity: JObject,
    tag: jint,
    path: JString,
) {
    if let Some(path) = string(&mut env, &path) {
        push(Event::Opened {
            tag,
            path: PathBuf::from(path),
        });
    }
}

#[no_mangle]
pub extern "system" fn Java_com_q0id_android_Q0Activity_nativeDocumentFailed(
    mut env: JNIEnv,
    _activity: JObject,
    tag: jint,
    message: JString,
) {
    let message = string(&mut env, &message).unwrap_or_default();
    push(Event::Failed { tag, message });
}

#[no_mangle]
pub extern "system" fn Java_com_q0id_android_Q0Activity_nativeStatus(
    mut env: JNIEnv,
    _activity: JObject,
    message: JString,
) {
    if let Some(message) = string(&mut env, &message) {
        push(Event::Status(message));
    }
}

#[no_mangle]
pub extern "system" fn Java_com_q0id_android_Q0Activity_nativeBack(
    _env: JNIEnv,
    _activity: JObject,
) {
    push(Event::Back);
}

/// Runs `call` against the activity from the current thread.
///
/// The native main thread never returns to Java, so nothing would ever free
/// the local references a call creates; the local frame does that instead.
fn with_activity<T>(
    call: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let context = ndk_context::android_context();
    // SAFETY: android-activity sets the context up before `android_main` runs
    // and keeps the VM and its reference to the activity alive until it ends.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }.map_err(|e| e.to_string())?;
    let activity = unsafe { JObject::from_raw(context.context().cast()) };
    let mut env = vm
        .attach_current_thread_permanently()
        .map_err(|e| e.to_string())?;
    let result = env.with_local_frame(8, |env| call(env, &activity));
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result.map_err(|error| error.to_string())
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{} is not a valid UTF-8 path", path.display()))
}

/// Shows the system document picker. The answer arrives later as
/// [`Event::Opened`] or [`Event::Failed`] carrying `tag`. An empty list of
/// MIME types offers every file.
pub fn pick_document(tag: i32, mime_types: &[&str]) -> Result<(), String> {
    with_activity(|env, activity| {
        let types = env.new_object_array(
            mime_types.len() as jsize,
            "java/lang/String",
            JObject::null(),
        )?;
        for (index, mime_type) in mime_types.iter().enumerate() {
            let value = env.new_string(mime_type)?;
            env.set_object_array_element(&types, index as jsize, value)?;
        }
        env.call_method(
            activity,
            "pickDocument",
            "(I[Ljava/lang/String;)V",
            &[JValue::Int(tag), JValue::Object(&types)],
        )?;
        Ok(())
    })
}

/// Asks where a copy of the file being written at `path` should go. Once the
/// person chooses, and [`document_written`] has been called for `path`, the
/// activity copies it there, and again after every later write.
pub fn create_document(path: &Path, mime_type: &str) -> Result<(), String> {
    let path = path_text(path)?;
    with_activity(|env, activity| {
        let path = env.new_string(path)?;
        let mime_type = env.new_string(mime_type)?;
        env.call_method(
            activity,
            "createDocument",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&path), JValue::Object(&mime_type)],
        )?;
        Ok(())
    })
}

/// Tells the activity that `path` was just written in full, so a document it
/// was opened from, or created for, gets the new contents too.
pub fn document_written(path: &Path) -> Result<(), String> {
    let path = path_text(path)?;
    with_activity(|env, activity| {
        let path = env.new_string(path)?;
        env.call_method(
            activity,
            "documentWritten",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&path)],
        )?;
        Ok(())
    })
}

/// Tells the activity that an export finished at `path`. Its copy goes to the
/// place chosen for it before, or the person is asked for one now: exports
/// left inside the app would be out of everyone's reach.
pub fn export_finished(path: &Path, mime_type: &str) -> Result<(), String> {
    let path = path_text(path)?;
    with_activity(|env, activity| {
        let path = env.new_string(path)?;
        let mime_type = env.new_string(mime_type)?;
        env.call_method(
            activity,
            "exportFinished",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&path), JValue::Object(&mime_type)],
        )?;
        Ok(())
    })
}

/// Hands the movie at `path` to q0player. It must sit in the app's cache
/// `preview` folder, the only one shared with other apps.
pub fn open_in_player(path: &Path, tag: i32) -> Result<(), String> {
    let path = path_text(path)?;
    with_activity(|env, activity| {
        let path = env.new_string(path)?;
        env.call_method(
            activity,
            "openInPlayer",
            "(Ljava/lang/String;I)V",
            &[JValue::Object(&path), JValue::Int(tag)],
        )?;
        Ok(())
    })
}

fn call_flag(method: &str, on: bool) {
    let result = with_activity(|env, activity| {
        env.call_method(activity, method, "(Z)V", &[JValue::Bool(u8::from(on))])?;
        Ok(())
    });
    if let Err(error) = result {
        log::error!("the activity did not take {method}({on}): {error}");
    }
}

/// Sends the app to the background, the way Back leaves other apps, without
/// ending it: the native event loop cannot start again in the same process.
pub fn leave() {
    if let Err(error) = with_activity(|env, activity| {
        env.call_method(activity, "leave", "()V", &[])?;
        Ok(())
    }) {
        log::error!("the activity did not leave: {error}");
    }
}

/// Keeps the display from sleeping, for playback.
pub fn keep_screen_on(on: bool) {
    call_flag("keepScreenOn", on);
}

/// Hides the status and navigation bars until a swipe brings them back.
pub fn immersive(on: bool) {
    call_flag("immersive", on);
}
