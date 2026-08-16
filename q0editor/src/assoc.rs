//! Windows-only per-user `.q1s` association for q0editor.
//!
//! Registration preserves a foreign default. Unregistration restores it only
//! while q0editor remains the current owner, so a later user choice is safe.

#![cfg(windows)]

use std::ffi::c_void;

use winreg::enums::*;
use winreg::RegKey;

const PROJECT_PROG_ID: &str = "q0editor.Project";
const PROJECT_FRIENDLY: &str = "q0s Project";
const Q0LANG_PROG_ID: &str = "q0editor.Q0langSource";
const Q0LANG_FRIENDLY: &str = "q0lang source";
const PREVIOUS_PROG_ID_VALUE: &str = "PreviousDefaultProgId";
const PREVIOUS_Q0L_PROG_ID_VALUE: &str = "PreviousDefaultProgId.q0l";
const PREVIOUS_Q0LANG_PROG_ID_VALUE: &str = "PreviousDefaultProgId.q0lang";

#[derive(Clone, Copy)]
struct AssociationSpec {
    extension: &'static str,
    prog_id: &'static str,
    friendly: &'static str,
    previous_value: &'static str,
}

const PROJECT_ASSOCIATION: AssociationSpec = AssociationSpec {
    extension: ".q1s",
    prog_id: PROJECT_PROG_ID,
    friendly: PROJECT_FRIENDLY,
    previous_value: PREVIOUS_PROG_ID_VALUE,
};
const Q0L_ASSOCIATION: AssociationSpec = AssociationSpec {
    extension: ".q0l",
    prog_id: Q0LANG_PROG_ID,
    friendly: Q0LANG_FRIENDLY,
    previous_value: PREVIOUS_Q0L_PROG_ID_VALUE,
};
const Q0LANG_ASSOCIATION: AssociationSpec = AssociationSpec {
    extension: ".q0lang",
    prog_id: Q0LANG_PROG_ID,
    friendly: Q0LANG_FRIENDLY,
    previous_value: PREVIOUS_Q0LANG_PROG_ID_VALUE,
};
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

fn extension_default_belongs_to(current: Option<&str>, prog_id: &str) -> bool {
    current.is_some_and(|value| value.eq_ignore_ascii_case(prog_id))
}

fn restorable_foreign_prog_id<'a>(value: Option<&'a str>, prog_id: &str) -> Option<&'a str> {
    value.filter(|value| {
        !value.is_empty() && *value == value.trim() && !value.eq_ignore_ascii_case(prog_id)
    })
}

fn backup_action_for_registration<'a>(current: Option<&'a str>, prog_id: &str) -> BackupAction<'a> {
    if extension_default_belongs_to(current, prog_id) {
        BackupAction::KeepExisting
    } else if let Some(previous) = restorable_foreign_prog_id(current, prog_id) {
        BackupAction::ReplaceWith(previous)
    } else {
        BackupAction::Clear
    }
}

fn action_for_unregistration<'a>(
    current: Option<&str>,
    previous: Option<&'a str>,
    prog_id: &str,
) -> UnregisterAction<'a> {
    if !extension_default_belongs_to(current, prog_id) {
        UnregisterAction::PreserveForeign
    } else if let Some(previous) = restorable_foreign_prog_id(previous, prog_id) {
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

fn read_extension_default(classes: &RegKey, extension: &str) -> std::io::Result<Option<String>> {
    match classes.open_subkey_with_flags(extension, KEY_READ) {
        Ok(ext_key) => read_optional_string(&ext_key, ""),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_saved_previous_prog_id(
    classes: &RegKey,
    prog_id: &str,
    previous_value: &str,
) -> std::io::Result<Option<String>> {
    match classes.open_subkey_with_flags(prog_id, KEY_READ) {
        Ok(prog_key) => read_optional_string(&prog_key, previous_value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn expected_open_command() -> std::io::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\" \"%1\"", exe.to_string_lossy()))
}

fn read_open_command(classes: &RegKey, prog_id: &str) -> std::io::Result<Option<String>> {
    match classes.open_subkey_with_flags(format!("{prog_id}\\shell\\open\\command"), KEY_READ) {
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
    is_spec_registered(PROJECT_ASSOCIATION)
}

pub fn register_for_current_user() -> std::io::Result<()> {
    register_spec(PROJECT_ASSOCIATION)?;
    notify_association_changed();
    Ok(())
}

pub fn unregister_for_current_user() -> std::io::Result<()> {
    unregister_spec(PROJECT_ASSOCIATION, true)?;
    notify_association_changed();
    Ok(())
}

pub fn is_q0lang_registered_for_current_user() -> std::io::Result<bool> {
    Ok(is_spec_registered(Q0L_ASSOCIATION)? && is_spec_registered(Q0LANG_ASSOCIATION)?)
}

pub fn register_q0lang_for_current_user() -> std::io::Result<()> {
    register_spec(Q0L_ASSOCIATION)?;
    register_spec(Q0LANG_ASSOCIATION)?;
    notify_association_changed();
    Ok(())
}

pub fn unregister_q0lang_for_current_user() -> std::io::Result<()> {
    unregister_spec(Q0L_ASSOCIATION, false)?;
    unregister_spec(Q0LANG_ASSOCIATION, true)?;
    notify_association_changed();
    Ok(())
}

fn is_spec_registered(spec: AssociationSpec) -> std::io::Result<bool> {
    let classes = match open_classes(KEY_READ) {
        Ok(classes) => classes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let current = read_extension_default(&classes, spec.extension)?;
    if !extension_default_belongs_to(current.as_deref(), spec.prog_id) {
        return Ok(false);
    }
    let actual_command = read_open_command(&classes, spec.prog_id)?;
    let expected_command = expected_open_command()?;
    Ok(actual_command
        .as_deref()
        .is_some_and(|command| command.eq_ignore_ascii_case(&expected_command)))
}

fn register_spec(spec: AssociationSpec) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy().to_string();
    let classes = open_classes(KEY_READ | KEY_WRITE)?;
    let current = read_extension_default(&classes, spec.extension)?;
    let backup_action = backup_action_for_registration(current.as_deref(), spec.prog_id);

    let (prog_key, _) = classes.create_subkey(spec.prog_id)?;
    match backup_action {
        BackupAction::KeepExisting => {
            let saved = read_optional_string(&prog_key, spec.previous_value)?;
            if restorable_foreign_prog_id(saved.as_deref(), spec.prog_id).is_none() {
                delete_value_if_present(&prog_key, spec.previous_value)?;
            }
        }
        BackupAction::ReplaceWith(previous) => {
            prog_key.set_value(spec.previous_value, &previous)?;
        }
        BackupAction::Clear => {
            delete_value_if_present(&prog_key, spec.previous_value)?;
        }
    }

    prog_key.set_value("", &spec.friendly)?;
    let (icon_key, _) = prog_key.create_subkey("DefaultIcon")?;
    icon_key.set_value("", &format!("\"{}\",0", exe_str))?;
    let (command_key, _) = prog_key.create_subkey("shell\\open\\command")?;
    command_key.set_value("", &format!("\"{}\" \"%1\"", exe_str))?;

    let (ext_key, _) = classes.create_subkey(spec.extension)?;
    ext_key.set_value("", &spec.prog_id)?;
    Ok(())
}

fn unregister_spec(spec: AssociationSpec, remove_prog_id: bool) -> std::io::Result<()> {
    let classes = match open_classes(KEY_READ | KEY_WRITE) {
        Ok(classes) => classes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    let previous = read_saved_previous_prog_id(&classes, spec.prog_id, spec.previous_value)?;
    match classes.open_subkey_with_flags(spec.extension, KEY_READ | KEY_WRITE) {
        Ok(ext_key) => {
            let current = read_optional_string(&ext_key, "")?;
            match action_for_unregistration(current.as_deref(), previous.as_deref(), spec.prog_id) {
                UnregisterAction::PreserveForeign => {}
                UnregisterAction::Restore(previous) => ext_key.set_value("", &previous)?,
                UnregisterAction::RemoveEditorDefault => delete_value_if_present(&ext_key, "")?,
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    if remove_prog_id {
        match classes.delete_subkey_all(spec.prog_id) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    } else if let Ok(prog_key) = classes.open_subkey_with_flags(spec.prog_id, KEY_READ | KEY_WRITE)
    {
        delete_value_if_present(&prog_key, spec.previous_value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_prog_id_comparison_is_case_insensitive() {
        assert!(extension_default_belongs_to(
            Some(PROJECT_PROG_ID),
            PROJECT_PROG_ID
        ));
        assert!(extension_default_belongs_to(
            Some("Q0EDITOR.PROJECT"),
            PROJECT_PROG_ID
        ));
    }

    #[test]
    fn registration_preserves_a_foreign_default() {
        assert_eq!(
            backup_action_for_registration(Some("another-editor.Project"), PROJECT_PROG_ID),
            BackupAction::ReplaceWith("another-editor.Project")
        );
    }

    #[test]
    fn registration_keeps_the_original_backup_on_upgrade() {
        assert_eq!(
            backup_action_for_registration(Some(PROJECT_PROG_ID), PROJECT_PROG_ID),
            BackupAction::KeepExisting
        );
    }

    #[test]
    fn unregister_restores_only_while_editor_is_current() {
        assert_eq!(
            action_for_unregistration(
                Some(PROJECT_PROG_ID),
                Some("another-editor.Project"),
                PROJECT_PROG_ID
            ),
            UnregisterAction::Restore("another-editor.Project")
        );
        assert_eq!(
            action_for_unregistration(
                Some("users-new-choice.Project"),
                Some("another-editor.Project"),
                PROJECT_PROG_ID
            ),
            UnregisterAction::PreserveForeign
        );
    }

    #[test]
    fn invalid_backup_removes_only_our_default() {
        assert_eq!(
            action_for_unregistration(
                Some(PROJECT_PROG_ID),
                Some(PROJECT_PROG_ID),
                PROJECT_PROG_ID
            ),
            UnregisterAction::RemoveEditorDefault
        );
    }

    #[test]
    fn q0l_and_q0lang_share_a_prog_id_but_keep_independent_backups() {
        assert_eq!(Q0L_ASSOCIATION.prog_id, Q0LANG_ASSOCIATION.prog_id);
        assert_ne!(Q0L_ASSOCIATION.extension, Q0LANG_ASSOCIATION.extension);
        assert_ne!(
            Q0L_ASSOCIATION.previous_value,
            Q0LANG_ASSOCIATION.previous_value
        );
    }
}
