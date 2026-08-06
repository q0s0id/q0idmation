//! Windows-only per-user `.q1s` association for q0editor.
//!
//! Registration preserves a foreign default. Unregistration restores it only
//! while q0editor remains the current owner, so a later user choice is safe.

#![cfg(windows)]

use std::ffi::c_void;

use winreg::enums::*;
use winreg::RegKey;

const PROG_ID: &str = "q0editor.Project";
const FRIENDLY: &str = "q0s Project";
const EXTENSION: &str = ".q1s";
const PREVIOUS_PROG_ID_VALUE: &str = "PreviousDefaultProgId";
const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
const SHCNF_IDLIST: u32 = 0;

#[link(name = "shell32")]
extern "system" {
    fn SHChangeNotify(event_id: i32, flags: u32, item1: *const c_void, item2: *const c_void);
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
    RemoveEditorDefault,
}

fn extension_default_belongs_to_editor(current: Option<&str>) -> bool {
    current.is_some_and(|value| value.eq_ignore_ascii_case(PROG_ID))
}

fn restorable_foreign_prog_id(value: Option<&str>) -> Option<&str> {
    value.filter(|value| {
        !value.is_empty() && *value == value.trim() && !value.eq_ignore_ascii_case(PROG_ID)
    })
}

fn backup_action_for_registration(current: Option<&str>) -> BackupAction<'_> {
    if extension_default_belongs_to_editor(current) {
        BackupAction::KeepExisting
    } else if let Some(previous) = restorable_foreign_prog_id(current) {
        BackupAction::ReplaceWith(previous)
    } else {
        BackupAction::Clear
    }
}

fn action_for_unregistration<'a>(
    current: Option<&str>,
    previous: Option<&'a str>,
) -> UnregisterAction<'a> {
    if !extension_default_belongs_to_editor(current) {
        UnregisterAction::PreserveForeign
    } else if let Some(previous) = restorable_foreign_prog_id(previous) {
        UnregisterAction::Restore(previous)
    } else {
        UnregisterAction::RemoveEditorDefault
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

fn read_extension_default(classes: &RegKey) -> std::io::Result<Option<String>> {
    match classes.open_subkey_with_flags(EXTENSION, KEY_READ) {
        Ok(ext_key) => read_optional_string(&ext_key, ""),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_saved_previous_prog_id(classes: &RegKey) -> std::io::Result<Option<String>> {
    match classes.open_subkey_with_flags(PROG_ID, KEY_READ) {
        Ok(prog_key) => read_optional_string(&prog_key, PREVIOUS_PROG_ID_VALUE),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn expected_open_command() -> std::io::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\" \"%1\"", exe.to_string_lossy()))
}

fn read_open_command(classes: &RegKey) -> std::io::Result<Option<String>> {
    match classes.open_subkey_with_flags(format!("{PROG_ID}\\shell\\open\\command"), KEY_READ) {
        Ok(command_key) => read_optional_string(&command_key, ""),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn notify_association_changed() {
    // SAFETY: documented shell association refresh with null item pointers.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

pub fn is_registered_for_current_user() -> std::io::Result<bool> {
    let classes = match open_classes(KEY_READ) {
        Ok(classes) => classes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let current = read_extension_default(&classes)?;
    if !extension_default_belongs_to_editor(current.as_deref()) {
        return Ok(false);
    }
    let actual_command = read_open_command(&classes)?;
    let expected_command = expected_open_command()?;
    Ok(actual_command
        .as_deref()
        .is_some_and(|command| command.eq_ignore_ascii_case(&expected_command)))
}

pub fn register_for_current_user() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy().to_string();
    let classes = open_classes(KEY_READ | KEY_WRITE)?;
    let current = read_extension_default(&classes)?;
    let backup_action = backup_action_for_registration(current.as_deref());

    // Build the private ProgID before publishing the shared extension default.
    let (prog_key, _) = classes.create_subkey(PROG_ID)?;
    match backup_action {
        BackupAction::KeepExisting => {
            let saved = read_optional_string(&prog_key, PREVIOUS_PROG_ID_VALUE)?;
            if restorable_foreign_prog_id(saved.as_deref()).is_none() {
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

    prog_key.set_value("", &FRIENDLY)?;
    let (icon_key, _) = prog_key.create_subkey("DefaultIcon")?;
    icon_key.set_value("", &format!("\"{}\",0", exe_str))?;
    let (command_key, _) = prog_key.create_subkey("shell\\open\\command")?;
    command_key.set_value("", &format!("\"{}\" \"%1\"", exe_str))?;

    let (ext_key, _) = classes.create_subkey(EXTENSION)?;
    ext_key.set_value("", &PROG_ID)?;
    notify_association_changed();
    Ok(())
}

pub fn unregister_for_current_user() -> std::io::Result<()> {
    let classes = match open_classes(KEY_READ | KEY_WRITE) {
        Ok(classes) => classes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    let previous = read_saved_previous_prog_id(&classes)?;
    match classes.open_subkey_with_flags(EXTENSION, KEY_READ | KEY_WRITE) {
        Ok(ext_key) => {
            let current = read_optional_string(&ext_key, "")?;
            match action_for_unregistration(current.as_deref(), previous.as_deref()) {
                UnregisterAction::PreserveForeign => {}
                UnregisterAction::Restore(previous) => ext_key.set_value("", &previous)?,
                UnregisterAction::RemoveEditorDefault => {
                    delete_value_if_present(&ext_key, "")?;
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    match classes.delete_subkey_all(PROG_ID) {
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
    fn editor_prog_id_comparison_is_case_insensitive() {
        assert!(extension_default_belongs_to_editor(Some(PROG_ID)));
        assert!(extension_default_belongs_to_editor(Some(
            "Q0EDITOR.PROJECT"
        )));
    }

    #[test]
    fn registration_preserves_a_foreign_default() {
        assert_eq!(
            backup_action_for_registration(Some("another-editor.Project")),
            BackupAction::ReplaceWith("another-editor.Project")
        );
    }

    #[test]
    fn registration_keeps_the_original_backup_on_upgrade() {
        assert_eq!(
            backup_action_for_registration(Some(PROG_ID)),
            BackupAction::KeepExisting
        );
    }

    #[test]
    fn unregister_restores_only_while_editor_is_current() {
        assert_eq!(
            action_for_unregistration(Some(PROG_ID), Some("another-editor.Project")),
            UnregisterAction::Restore("another-editor.Project")
        );
        assert_eq!(
            action_for_unregistration(
                Some("users-new-choice.Project"),
                Some("another-editor.Project")
            ),
            UnregisterAction::PreserveForeign
        );
    }

    #[test]
    fn invalid_backup_removes_only_our_default() {
        assert_eq!(
            action_for_unregistration(Some(PROG_ID), Some(PROG_ID)),
            UnregisterAction::RemoveEditorDefault
        );
    }
}
