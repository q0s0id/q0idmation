//! Windows-only per-user file associations for q0player media.
//!
//! Each extension owns a distinct ProgID and backup. Registering preserves a
//! foreign default, and unregistering restores it only while q0player remains
//! the current owner. A later user choice is never overwritten.

#![cfg(windows)]

use std::ffi::c_void;

use winreg::enums::*;
use winreg::RegKey;

const PREVIOUS_PROG_ID_VALUE: &str = "PreviousDefaultProgId";
const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
const SHCNF_IDLIST: u32 = 0;

#[link(name = "shell32")]
extern "system" {
    fn SHChangeNotify(event_id: i32, flags: u32, item1: *const c_void, item2: *const c_void);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationKind {
    Q0s,
    Q0v,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AssociationSpec {
    extension: &'static str,
    prog_id: &'static str,
    friendly: &'static str,
    icon_file: Option<&'static str>,
}

impl AssociationKind {
    fn spec(self) -> AssociationSpec {
        match self {
            Self::Q0s => AssociationSpec {
                extension: ".q0s",
                prog_id: "q0player.Movie",
                friendly: "q0s Movie",
                icon_file: Some("q0s.ico"),
            },
            Self::Q0v => AssociationSpec {
                extension: ".q0v",
                prog_id: "q0player.Q0v",
                friendly: "q0v Media",
                icon_file: None,
            },
        }
    }

    pub fn extension(self) -> &'static str {
        self.spec().extension
    }
}

#[derive(Debug, PartialEq, Eq)]
enum BackupAction<'a> {
    KeepExisting,
    ReplaceWith(&'a str),
    Clear,
}

#[derive(Debug, PartialEq, Eq)]
enum UnregisterAction<'a> {
    PreserveForeign,
    Restore(&'a str),
    RemovePlayerDefault,
}

fn extension_default_belongs_to(spec: AssociationSpec, current: Option<&str>) -> bool {
    current.is_some_and(|value| value.eq_ignore_ascii_case(spec.prog_id))
}

fn restorable_foreign_prog_id(spec: AssociationSpec, value: Option<&str>) -> Option<&str> {
    value.filter(|value| {
        !value.is_empty() && *value == value.trim() && !value.eq_ignore_ascii_case(spec.prog_id)
    })
}

fn backup_action_for_registration(
    spec: AssociationSpec,
    current: Option<&str>,
) -> BackupAction<'_> {
    if extension_default_belongs_to(spec, current) {
        BackupAction::KeepExisting
    } else if let Some(previous) = restorable_foreign_prog_id(spec, current) {
        BackupAction::ReplaceWith(previous)
    } else {
        BackupAction::Clear
    }
}

fn action_for_unregistration<'a>(
    spec: AssociationSpec,
    current: Option<&str>,
    previous: Option<&'a str>,
) -> UnregisterAction<'a> {
    if !extension_default_belongs_to(spec, current) {
        UnregisterAction::PreserveForeign
    } else if let Some(previous) = restorable_foreign_prog_id(spec, previous) {
        UnregisterAction::Restore(previous)
    } else {
        UnregisterAction::RemovePlayerDefault
    }
}

fn read_optional_string(key: &RegKey, name: &str) -> std::io::Result<Option<String>> {
    match key.get_value(name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn delete_value_if_present(key: &RegKey, name: &str) -> std::io::Result<()> {
    match key.delete_value(name) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn open_classes(access: u32) -> std::io::Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.open_subkey_with_flags("Software\\Classes", access) {
        Ok(classes) => Ok(classes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && access & KEY_WRITE != 0 => {
            let (classes, _) = hkcu.create_subkey("Software\\Classes")?;
            Ok(classes)
        }
        Err(error) => Err(error),
    }
}

fn read_extension_default(
    classes: &RegKey,
    spec: AssociationSpec,
) -> std::io::Result<Option<String>> {
    match classes.open_subkey_with_flags(spec.extension, KEY_READ) {
        Ok(ext_key) => read_optional_string(&ext_key, ""),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_saved_previous_prog_id(
    classes: &RegKey,
    spec: AssociationSpec,
) -> std::io::Result<Option<String>> {
    match classes.open_subkey_with_flags(spec.prog_id, KEY_READ) {
        Ok(prog_key) => read_optional_string(&prog_key, PREVIOUS_PROG_ID_VALUE),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn expected_open_command() -> std::io::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\" \"%1\"", exe.to_string_lossy()))
}

fn read_open_command(classes: &RegKey, spec: AssociationSpec) -> std::io::Result<Option<String>> {
    let path = format!("{}\\shell\\open\\command", spec.prog_id);
    match classes.open_subkey_with_flags(path, KEY_READ) {
        Ok(command_key) => read_optional_string(&command_key, ""),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn notify_association_changed() {
    // SAFETY: this is the documented shell association refresh call; no item
    // pointers are dereferenced when both are null.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

pub fn is_registered_for_current_user(kind: AssociationKind) -> std::io::Result<bool> {
    let spec = kind.spec();
    let classes = match open_classes(KEY_READ) {
        Ok(classes) => classes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let current = read_extension_default(&classes, spec)?;
    if !extension_default_belongs_to(spec, current.as_deref()) {
        return Ok(false);
    }
    let actual_command = read_open_command(&classes, spec)?;
    let expected_command = expected_open_command()?;
    Ok(actual_command
        .as_deref()
        .is_some_and(|command| command.eq_ignore_ascii_case(&expected_command)))
}

pub fn register_for_current_user(kind: AssociationKind) -> std::io::Result<()> {
    let spec = kind.spec();
    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy().to_string();
    let icon_value = spec
        .icon_file
        .and_then(|file| exe.parent().map(|directory| directory.join(file)))
        .filter(|path| path.is_file())
        .map(|path| format!("\"{}\"", path.to_string_lossy()))
        .unwrap_or_else(|| format!("\"{}\",0", exe_str));

    let classes = open_classes(KEY_READ | KEY_WRITE)?;
    let current = read_extension_default(&classes, spec)?;
    let backup_action = backup_action_for_registration(spec, current.as_deref());

    // Build the private ProgID before publishing the shared extension default.
    let (prog_key, _) = classes.create_subkey(spec.prog_id)?;
    match backup_action {
        BackupAction::KeepExisting => {
            let saved = read_optional_string(&prog_key, PREVIOUS_PROG_ID_VALUE)?;
            if restorable_foreign_prog_id(spec, saved.as_deref()).is_none() {
                delete_value_if_present(&prog_key, PREVIOUS_PROG_ID_VALUE)?;
            }
        }
        BackupAction::ReplaceWith(previous) => {
            prog_key.set_value(PREVIOUS_PROG_ID_VALUE, &previous)?;
        }
        BackupAction::Clear => {
            delete_value_if_present(&prog_key, PREVIOUS_PROG_ID_VALUE)?;
        }
    }

    prog_key.set_value("", &spec.friendly)?;
    let (icon_key, _) = prog_key.create_subkey("DefaultIcon")?;
    icon_key.set_value("", &icon_value)?;
    let (command_key, _) = prog_key.create_subkey("shell\\open\\command")?;
    command_key.set_value("", &format!("\"{}\" \"%1\"", exe_str))?;

    let (ext_key, _) = classes.create_subkey(spec.extension)?;
    ext_key.set_value("", &spec.prog_id)?;
    notify_association_changed();
    Ok(())
}

pub fn unregister_for_current_user(kind: AssociationKind) -> std::io::Result<()> {
    let spec = kind.spec();
    let classes = match open_classes(KEY_READ | KEY_WRITE) {
        Ok(classes) => classes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    let previous = read_saved_previous_prog_id(&classes, spec)?;
    match classes.open_subkey_with_flags(spec.extension, KEY_READ | KEY_WRITE) {
        Ok(ext_key) => {
            let current = read_optional_string(&ext_key, "")?;
            match action_for_unregistration(spec, current.as_deref(), previous.as_deref()) {
                UnregisterAction::PreserveForeign => {}
                UnregisterAction::Restore(previous) => {
                    ext_key.set_value("", &previous)?;
                }
                UnregisterAction::RemovePlayerDefault => {
                    delete_value_if_present(&ext_key, "")?;
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    match classes.delete_subkey_all(spec.prog_id) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    notify_association_changed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_types_use_distinct_extensions_and_prog_ids() {
        let q0s = AssociationKind::Q0s.spec();
        let q0v = AssociationKind::Q0v.spec();
        assert_ne!(q0s.extension, q0v.extension);
        assert_ne!(q0s.prog_id, q0v.prog_id);
        assert_eq!(q0v.extension, ".q0v");
    }

    #[test]
    fn prog_id_comparison_is_case_insensitive_for_each_kind() {
        let spec = AssociationKind::Q0v.spec();
        assert!(extension_default_belongs_to(spec, Some("Q0PLAYER.Q0V")));
        assert!(!extension_default_belongs_to(spec, Some("q0player.Movie")));
    }

    #[test]
    fn registration_saves_a_foreign_default() {
        let spec = AssociationKind::Q0v.spec();
        assert_eq!(
            backup_action_for_registration(spec, Some("another-player.Media")),
            BackupAction::ReplaceWith("another-player.Media")
        );
    }

    #[test]
    fn registration_does_not_replace_the_original_backup_with_itself() {
        let spec = AssociationKind::Q0v.spec();
        assert_eq!(
            backup_action_for_registration(spec, Some(spec.prog_id)),
            BackupAction::KeepExisting
        );
        assert_eq!(
            backup_action_for_registration(spec, Some("Q0PLAYER.Q0V")),
            BackupAction::KeepExisting
        );
    }

    #[test]
    fn registration_clears_stale_backup_when_no_valid_default_exists() {
        let spec = AssociationKind::Q0v.spec();
        assert_eq!(
            backup_action_for_registration(spec, None),
            BackupAction::Clear
        );
        assert_eq!(
            backup_action_for_registration(spec, Some(" invalid.Media ")),
            BackupAction::Clear
        );
    }

    #[test]
    fn unregister_restores_saved_default_only_if_player_is_current() {
        let spec = AssociationKind::Q0v.spec();
        assert_eq!(
            action_for_unregistration(spec, Some(spec.prog_id), Some("another-player.Media")),
            UnregisterAction::Restore("another-player.Media")
        );
    }

    #[test]
    fn unregister_never_changes_a_later_user_choice() {
        let spec = AssociationKind::Q0v.spec();
        assert_eq!(
            action_for_unregistration(
                spec,
                Some("users-new-choice.Media"),
                Some("old-choice.Media")
            ),
            UnregisterAction::PreserveForeign
        );
    }

    #[test]
    fn unregister_removes_our_default_when_backup_is_missing_or_invalid() {
        let spec = AssociationKind::Q0v.spec();
        assert_eq!(
            action_for_unregistration(spec, Some(spec.prog_id), None),
            UnregisterAction::RemovePlayerDefault
        );
        assert_eq!(
            action_for_unregistration(spec, Some(spec.prog_id), Some(spec.prog_id)),
            UnregisterAction::RemovePlayerDefault
        );
    }
}
