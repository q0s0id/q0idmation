//! Keeps the q0id sign-in between runs, so q0editor signs back in by itself.
//!
//! What is kept is the refresh token (the server rotates it at every use),
//! in a file beside the settings. On Windows it is sealed with the person's
//! own Windows login (DPAPI): a copy of the file is no use on another
//! account or computer. On Android the file sits in the app's private
//! storage, which no other app can read. Signing out deletes it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Kept {
    /// The server the token belongs to; another server's is never sent.
    server: String,
    refresh: String,
}

const FILE: &str = "q0id-sign-in.bin";

fn default_path() -> Option<PathBuf> {
    // Tests never read or write the person's own sign-in.
    if cfg!(test) {
        return None;
    }
    Some(dirs::config_dir()?.join("q0editor").join(FILE))
}

/// Whether there is a sign-in kept for `server`, without unsealing it.
pub(super) fn remembered(server: &str) -> bool {
    load(server).is_some()
}

/// The refresh token kept for `server`.
pub(super) fn load(server: &str) -> Option<String> {
    load_from(&default_path()?, server)
}

/// Keeps `refresh` as the sign-in for `server`, replacing the last one.
pub(super) fn save(server: &str, refresh: &str) {
    if let Some(path) = default_path() {
        if let Err(error) = save_to(&path, server, refresh) {
            eprintln!("could not keep the q0id sign-in: {error}");
        }
    }
}

/// Forgets the kept sign-in: signing out, or the server no longer takes it.
pub(super) fn forget() {
    if let Some(path) = default_path() {
        let _ = std::fs::remove_file(path);
    }
}

fn load_from(path: &Path, server: &str) -> Option<String> {
    let sealed = std::fs::read(path).ok()?;
    let kept: Kept = serde_json::from_slice(&unseal(&sealed)?).ok()?;
    (kept.server == server && !kept.refresh.is_empty()).then_some(kept.refresh)
}

fn save_to(path: &Path, server: &str, refresh: &str) -> std::io::Result<()> {
    if refresh.is_empty() {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    let json = serde_json::to_vec(&Kept {
        server: server.to_string(),
        refresh: refresh.to_string(),
    })?;
    let sealed = seal(&json).ok_or_else(|| std::io::Error::other("could not seal it"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::file_io::write_bytes_atomic(path, &sealed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(windows)]
fn seal(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, true)
}

#[cfg(windows)]
fn unseal(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, false)
}

/// Seals or unseals with the Windows login of whoever runs the editor.
#[cfg(windows)]
fn dpapi(data: &[u8], seal: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).ok()?,
        pbData: data.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: both blobs point at live memory for the call; DPAPI allocates
    // the output, which is copied and then freed with LocalFree as it asks.
    let ok = unsafe {
        if seal {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 || output.pbData.is_null() {
        return None;
    }
    // SAFETY: DPAPI says the output holds `cbData` bytes.
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    // SAFETY: the output was allocated by DPAPI with LocalAlloc.
    unsafe { LocalFree(output.pbData.cast()) };
    Some(bytes)
}

/// Elsewhere the file's folder is the protection: the app's own storage on
/// Android, a file only its owner may read on other systems.
#[cfg(not(windows))]
fn seal(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

#[cfg(not(windows))]
fn unseal(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_sign_in_comes_back_for_its_own_server_only() {
        let dir = std::env::temp_dir().join(format!("q0id-keep-test-{}", std::process::id()));
        let path = dir.join(FILE);
        save_to(&path, "https://q0id.com", "refresh-1").unwrap();
        assert_eq!(
            load_from(&path, "https://q0id.com").as_deref(),
            Some("refresh-1")
        );
        assert_eq!(load_from(&path, "https://elsewhere.test"), None);
        // On Windows the token is not lying in the file as it is.
        let raw = std::fs::read(&path).unwrap();
        if cfg!(windows) {
            assert!(!raw.windows(9).any(|bytes| bytes == b"refresh-1"));
        }
        // The rotated token replaces the last one; an empty one forgets.
        save_to(&path, "https://q0id.com", "refresh-2").unwrap();
        assert_eq!(
            load_from(&path, "https://q0id.com").as_deref(),
            Some("refresh-2")
        );
        save_to(&path, "https://q0id.com", "").unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
        assert!(default_path().is_none(), "tests never touch the real one");
    }
}
