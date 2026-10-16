//! Disposable runtime snapshots, exported off the UI thread.
#[cfg(not(target_os = "android"))]
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::app::EditorApp;

#[derive(Default)]
pub(crate) struct FastPreview {
    pending: Option<Receiver<Result<(), String>>>,
}

#[cfg(not(target_os = "android"))]
fn player_path() -> Option<PathBuf> {
    let sibling = std::env::current_exe().ok()?.with_file_name("q0player.exe");
    if sibling.is_file() {
        return Some(sibling);
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let installed = PathBuf::from(local).join("Programs/q0idmation/q0player/q0player.exe");
        if installed.is_file() {
            return Some(installed);
        }
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("q0player.exe"))
        .find(|candidate| candidate.is_file())
}

pub(crate) fn start(app: &mut EditorApp, ctx: &egui::Context, current_only: bool) {
    if app.fast_preview.pending.is_some() {
        app.session.status = "fast preview is already preparing".into();
        return;
    }
    if !crate::text_tool::finish_edit(app) {
        return;
    }
    #[cfg(not(target_os = "android"))]
    let Some(player) = player_path() else {
        app.session.status =
            "fast preview needs q0player: install it or place q0player.exe beside q0editor.exe"
                .into();
        return;
    };
    let scenes = app.state.project.scenes();
    // Editing a nested symbol still previews its owning document scene.
    let current_root = app
        .session
        .breadcrumb
        .first()
        .copied()
        .unwrap_or(app.session.current_q0rg_id);
    let root = if current_only {
        scenes
            .iter()
            .find(|scene| scene.q0rg_id == current_root)
            .map(|scene| scene.q0rg_id)
            .unwrap_or(app.state.project.meta.entry_q0rg_id)
    } else {
        scenes
            .first()
            .map(|scene| scene.q0rg_id)
            .unwrap_or(app.state.project.meta.entry_q0rg_id)
    };
    let mut project = app.state.project.clone();
    project.meta.entry_q0rg_id = root;
    app.session.playing = false;
    app.session.animation_preview_playing = false;
    app.stop_audio_playback();
    let source_path = app.state.file_path.clone();
    let (sender, receiver) = mpsc::channel();
    app.fast_preview.pending = Some(receiver);
    app.session.status = "preparing fast preview…".into();
    let ctx = ctx.clone();
    #[cfg(target_os = "android")]
    std::thread::spawn(move || {
        let result = show_in_player_app(&project, source_path.as_deref(), current_only);
        let _ = sender.send(result);
        ctx.request_repaint();
    });
    #[cfg(not(target_os = "android"))]
    std::thread::spawn(move || {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("q0e-preview-{}-{stamp}", std::process::id()));
        let path = directory.join("preview.q0s");
        let result = (|| -> Result<std::process::Child, String> {
            let capabilities = std::process::Command::new(&player)
                .arg("--help")
                .output()
                .map_err(|error| format!("check q0player: {error}"))?;
            if !capabilities.status.success()
                || !String::from_utf8_lossy(&capabilities.stdout).contains("--preview")
            {
                return Err(
                    "update q0player: this installed version does not support fast preview".into(),
                );
            }
            std::fs::create_dir(&directory)
                .map_err(|error| format!("create preview directory: {error}"))?;
            let export =
                crate::export::export_with_embedded_dependencies(&project, source_path.as_deref())?;
            crate::file_io::write_bytes_atomic(&path, &export.bytes)
                .map_err(|error| format!("write preview: {error}"))?;
            let mut command = std::process::Command::new(player);
            command.arg("--preview").arg("--delete-after-load");
            if !current_only {
                command.arg("--all-scenes");
            }
            command.arg(&path);
            command
                .spawn()
                .map_err(|error| format!("launch q0player: {error}"))
        })();
        match result {
            Ok(mut child) => {
                let _ = sender.send(Ok(()));
                ctx.request_repaint();
                // The worker owns cleanup even when the editor closes first.
                let _ = child.wait();
            }
            Err(error) => {
                let _ = sender.send(Err(error));
                ctx.request_repaint();
            }
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&directory);
    });
}

/// Android: q0player is another app, handed the snapshot through the one
/// folder the editor shares. Each preview replaces the last, so nothing piles
/// up, and q0player keeps its own copy.
#[cfg(target_os = "android")]
fn show_in_player_app(
    project: &q0s_format::v2::ProjectV2,
    source_path: Option<&std::path::Path>,
    current_only: bool,
) -> Result<(), String> {
    let directory = crate::platform::preview_dir();
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create preview directory: {error}"))?;
    let path = directory.join("preview.q0s");
    let export = crate::export::export_with_embedded_dependencies(project, source_path)?;
    crate::file_io::write_bytes_atomic(&path, &export.bytes)
        .map_err(|error| format!("write preview: {error}"))?;
    crate::platform::open_in_player(&path, !current_only)
}

pub(crate) fn poll(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(receiver) = app.fast_preview.pending.as_ref() else {
        return;
    };
    match receiver.try_recv() {
        Ok(result) => {
            app.fast_preview.pending = None;
            app.session.status = match result {
                Ok(()) => "fast preview opened in q0player".into(),
                Err(error) => format!("fast preview failed: {error}"),
            };
        }
        Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
        Err(TryRecvError::Disconnected) => {
            app.fast_preview.pending = None;
            app.session.status = "fast preview worker stopped unexpectedly".into();
        }
    }
}
