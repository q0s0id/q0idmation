//! The players an app is built from: one q0player per platform. Q0Build
//! looks in `runtimes/` beside itself, then, when run from a q0idmation
//! checkout, in that checkout's build outputs.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    WindowsX64,
    WindowsArm64,
    LinuxAppImage,
    AndroidApk,
}

impl Target {
    pub const ALL: [Target; 4] = [
        Target::WindowsX64,
        Target::WindowsArm64,
        Target::LinuxAppImage,
        Target::AndroidApk,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Target::WindowsX64 => "Windows x64 (.exe)",
            Target::WindowsArm64 => "Windows ARM64 (.exe)",
            Target::LinuxAppImage => "Linux x86-64 (.AppImage)",
            Target::AndroidApk => "Android arm64 (.apk)",
        }
    }

    /// What `{target}` becomes in file names.
    pub fn slug(self) -> &'static str {
        match self {
            Target::WindowsX64 => "windows-x64",
            Target::WindowsArm64 => "windows-arm64",
            Target::LinuxAppImage => "linux-x86_64",
            Target::AndroidApk => "android-arm64",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Target::WindowsX64 | Target::WindowsArm64 => "exe",
            Target::LinuxAppImage => "AppImage",
            Target::AndroidApk => "apk",
        }
    }

    /// The runtime's place under `runtimes/`.
    fn runtime_file(self) -> &'static str {
        match self {
            Target::WindowsX64 => "windows-x64/q0player.exe",
            Target::WindowsArm64 => "windows-arm64/q0player.exe",
            Target::LinuxAppImage => "linux-x86_64/q0player",
            Target::AndroidApk => "android-arm64/q0player.apk",
        }
    }

    /// Where a q0idmation checkout builds this runtime.
    fn checkout_files(self) -> &'static [&'static str] {
        match self {
            Target::WindowsX64 => &[
                "target/x86_64-pc-windows-msvc/release/q0player.exe",
                "target/release/q0player.exe",
            ],
            Target::WindowsArm64 => &["target/aarch64-pc-windows-msvc/release/q0player.exe"],
            Target::LinuxAppImage => &["target/x86_64-unknown-linux-gnu/release/q0player"],
            Target::AndroidApk => &[
                "android/player/build/outputs/apk/release/player-release.apk",
                "android/player/build/outputs/apk/debug/player-debug.apk",
            ],
        }
    }

    /// How to get the runtime when it is missing.
    pub fn how_to_get(self) -> &'static str {
        match self {
            Target::WindowsX64 => "put q0player.exe (x64) in runtimes/windows-x64/, or build it: cargo build --release -p q0player",
            Target::WindowsArm64 => "put q0player.exe (ARM64) in runtimes/windows-arm64/, or build it: cargo build --release -p q0player --target aarch64-pc-windows-msvc",
            Target::LinuxAppImage => "put a Linux q0player in runtimes/linux-x86_64/, built on Linux: cargo build --release -p q0player",
            Target::AndroidApk => "put q0player.apk in runtimes/android-arm64/, or build it: android\\build.ps1 -Mode apk -App player",
        }
    }
}

/// The runtimes folder beside Q0Build.
pub fn runtimes_folder() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join("runtimes")
}

/// The q0idmation checkout Q0Build runs from, if it does.
fn checkout() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|folder| folder.join("q0player").join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
}

/// Where the runtime for `target` is, if anywhere.
pub fn locate(target: Target) -> Option<PathBuf> {
    let shipped = runtimes_folder().join(target.runtime_file());
    if shipped.is_file() {
        return Some(shipped);
    }
    let root = checkout()?;
    target
        .checkout_files()
        .iter()
        .map(|file| root.join(file))
        .find(|path| path.is_file())
}

/// The AppImage type-2 runtime: the small ELF program that mounts the
/// squashfs image glued behind it.
pub fn appimage_runtime() -> Result<PathBuf, String> {
    let shipped = runtimes_folder()
        .join("linux-x86_64")
        .join("appimage-runtime-x86_64");
    if shipped.is_file() {
        return Ok(shipped);
    }
    let cache = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("q0build")
        .join("appimage-runtime-x86_64");
    if cache.is_file() {
        return Ok(cache);
    }
    std::fs::create_dir_all(cache.parent().expect("cache file has a folder"))
        .map_err(|error| error.to_string())?;
    const URL: &str =
        "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64";
    let status = std::process::Command::new("curl")
        .args(["-L", "-f", "-s", "-S", "-o"])
        .arg(&cache)
        .arg(URL)
        .status()
        .map_err(|error| format!("could not run curl to download the AppImage runtime: {error}"))?;
    if !status.success() {
        std::fs::remove_file(&cache).ok();
        return Err(format!(
            "could not download the AppImage runtime from {URL}; put it in runtimes/linux-x86_64/appimage-runtime-x86_64"
        ));
    }
    Ok(cache)
}
