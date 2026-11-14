//! A build, on its own thread: the bundle once, then each target from its
//! runtime, each result read back and checked.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use crate::icons::AppIcon;
use crate::profile::Profile;
use crate::runtimes::{self, Target};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Good,
    Warn,
    Bad,
}

pub enum Event {
    Log(Level, String),
    Progress(f32, String),
    Finished(Result<Vec<Artifact>, String>),
}

#[derive(Debug, Clone)]
pub struct Artifact {
    pub target: Target,
    pub path: PathBuf,
    pub bytes: u64,
}

struct Report {
    sender: Sender<Event>,
    lines: Vec<String>,
}

impl Report {
    fn log(&mut self, level: Level, text: impl Into<String>) {
        let text = text.into();
        let mark = match level {
            Level::Info => "   ",
            Level::Good => " ✓ ",
            Level::Warn => " ! ",
            Level::Bad => " ✗ ",
        };
        self.lines.push(format!("{mark}{text}"));
        let _ = self.sender.send(Event::Log(level, text));
    }

    fn progress(&self, fraction: f32, text: impl Into<String>) {
        let _ = self.sender.send(Event::Progress(fraction, text.into()));
    }
}

pub fn targets(profile: &Profile) -> Vec<Target> {
    let wanted = &profile.targets;
    Target::ALL
        .into_iter()
        .filter(|target| match target {
            Target::WindowsX64 => wanted.windows_x64,
            Target::WindowsArm64 => wanted.windows_arm64,
            Target::LinuxAppImage => wanted.linux_appimage,
            Target::AndroidApk => wanted.android_apk,
        })
        .collect()
}

pub fn spawn(profile: Profile) -> Receiver<Event> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut report = Report {
            sender: sender.clone(),
            lines: Vec::new(),
        };
        let result = run(&profile, &mut report);
        if let Err(error) = &result {
            report.log(Level::Bad, error.clone());
        }
        if profile.output.keep_log {
            if let Some(folder) = profile.output_folder() {
                let _ = std::fs::create_dir_all(&folder);
                let _ =
                    std::fs::write(folder.join("build-log.txt"), report.lines.join("\n") + "\n");
            }
        }
        let _ = sender.send(Event::Finished(result));
    });
    receiver
}

/// Today as YYYY-MM-DD, from the clock alone.
pub fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() / 86_400) as i64;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The movie as it goes into the bundle.
pub fn movie_bytes(
    profile: &Profile,
    report: &mut dyn FnMut(Level, String),
) -> Result<Vec<u8>, String> {
    let path = profile
        .movie
        .as_ref()
        .ok_or("choose a movie on the Project page")?;
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    // Older formats go in as they are: the runtime reads them all.
    if !profile.repack || q0s_format::v2::parse(&bytes).is_err() {
        return Ok(bytes);
    }
    match q0s_format::v2::parse(&bytes).and_then(|project| q0s_format::v2::write_packed(&project)) {
        Ok(packed) if packed.len() < bytes.len() => {
            report(
                Level::Info,
                format!(
                    "re-packed the movie: {} → {}",
                    size(bytes.len() as u64),
                    size(packed.len() as u64)
                ),
            );
            Ok(packed)
        }
        Ok(_) => Ok(bytes),
        Err(error) => {
            report(
                Level::Warn,
                format!("the movie could not be re-packed ({error}); it goes in as it is"),
            );
            Ok(bytes)
        }
    }
}

pub fn app_icon(profile: &Profile) -> Result<AppIcon, String> {
    match &profile.icon {
        Some(path) => {
            let bytes =
                std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
            AppIcon::load(&bytes)
        }
        None => Ok(crate::icons::placeholder(
            &profile.manifest.app.name,
            profile.manifest.splash.background,
        )),
    }
}

fn run(profile: &Profile, report: &mut Report) -> Result<Vec<Artifact>, String> {
    let targets = targets(profile);
    if targets.is_empty() {
        return Err("tick at least one target on the Targets page".into());
    }
    let problems = profile.manifest.problems();
    if !problems.is_empty() {
        return Err(format!(
            "the app settings need fixing: {}",
            problems.join("; ")
        ));
    }
    let folder = profile
        .output_folder()
        .ok_or("choose an output folder on the Output page")?;
    std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    report.log(
        Level::Info,
        format!(
            "building {} {} into {}",
            profile.manifest.app.name,
            profile.manifest.app.version,
            folder.display()
        ),
    );

    report.progress(0.03, "reading the movie");
    let movie = movie_bytes(profile, &mut |level, text| report.log(level, text))?;
    if q0s_format::v2::parse(&movie).is_err() {
        report.log(
            Level::Info,
            "the movie is in an older q0s format: it goes in as it is",
        );
    }
    report.progress(0.08, "preparing the icon");
    let icon = app_icon(profile)?;
    for note in &icon.notes {
        report.log(Level::Warn, format!("icon: {note}"));
    }
    let bundle = q0qir::Bundle {
        manifest: profile.manifest.clone(),
        icon_png: Some(icon.png(256)),
        movie,
    }
    .encode();
    report.log(
        Level::Info,
        format!("bundle: {}", size(bundle.len() as u64)),
    );

    let date = today();
    let mut artifacts = Vec::new();
    let step = 0.85 / targets.len() as f32;
    for (index, target) in targets.iter().enumerate() {
        let base = 0.1 + step * index as f32;
        report.progress(base, format!("building {}", target.label()));
        let stem = profile.file_stem(target.slug(), &date);
        let path = folder.join(format!("{stem}.{}", target.extension()));
        let made = match target {
            Target::WindowsX64 | Target::WindowsArm64 => {
                build_windows(*target, profile, &bundle, &icon, &path, report)
            }
            Target::LinuxAppImage => build_appimage(profile, &bundle, &icon, &path),
            Target::AndroidApk => build_apk(profile, &bundle, &icon, &path, report),
        };
        if let Err(error) = made {
            report.log(Level::Bad, format!("{}: {error}", target.label()));
            continue;
        }
        if profile.output.verify {
            report.progress(base + step * 0.7, format!("checking {}", target.label()));
            verify(*target, &path)?;
            report.log(
                Level::Good,
                format!("{}: the bundle reads back intact", target.label()),
            );
        }
        if profile.output.zip {
            zip_one(&path).map_err(|error| format!("zipping {}: {error}", path.display()))?;
        }
        let bytes = std::fs::metadata(&path).map_or(0, |meta| meta.len());
        report.log(
            Level::Good,
            format!("{} → {} ({})", target.label(), path.display(), size(bytes)),
        );
        artifacts.push(Artifact {
            target: *target,
            path,
            bytes,
        });
    }
    if artifacts.is_empty() {
        return Err("nothing was built".into());
    }
    if profile.output.checksums {
        report.progress(0.97, "writing checksums");
        write_checksums(&folder, &artifacts, profile.output.zip)?;
        report.log(Level::Info, "SHA256SUMS.txt written");
    }
    report.progress(1.0, "done");
    if artifacts.len() < targets.len() {
        report.log(
            Level::Warn,
            format!("{} of {} targets built", artifacts.len(), targets.len()),
        );
    }
    Ok(artifacts)
}

fn runtime(target: Target) -> Result<Vec<u8>, String> {
    let path =
        runtimes::locate(target).ok_or_else(|| format!("no runtime: {}", target.how_to_get()))?;
    std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))
}

/// The runtime without any bundle a previous build left on it.
fn bare(mut runtime: Vec<u8>) -> Vec<u8> {
    if let Some(length) = q0qir::bundle_len(&runtime) {
        runtime.truncate(runtime.len() - length);
    }
    runtime
}

fn build_windows(
    target: Target,
    profile: &Profile,
    bundle: &[u8],
    icon: &AppIcon,
    path: &Path,
    report: &mut Report,
) -> Result<(), String> {
    let runtime = bare(runtime(target)?);
    if !runtime.starts_with(b"MZ") {
        return Err("the runtime is not a Windows program".into());
    }
    std::fs::write(path, &runtime).map_err(|error| format!("{}: {error}", path.display()))?;
    if profile.windows.file_info {
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let info = crate::winres::FileInfo {
            app: &profile.manifest.app,
            file_name: &file_name,
        };
        match crate::winres::apply(path, icon, &info) {
            Ok(()) => report.log(Level::Info, "exe icon and file details written"),
            Err(error) => report.log(Level::Warn, error),
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    std::io::Write::write_all(&mut file, bundle)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn build_appimage(
    profile: &Profile,
    bundle: &[u8],
    icon: &AppIcon,
    path: &Path,
) -> Result<(), String> {
    let mut player = bare(runtime(Target::LinuxAppImage)?);
    player.extend_from_slice(bundle);
    let appimage_runtime = runtimes::appimage_runtime()?;
    let appimage_runtime = std::fs::read(&appimage_runtime)
        .map_err(|error| format!("{}: {error}", appimage_runtime.display()))?;
    let image = crate::appimage::build(
        &player,
        &profile.manifest,
        &icon.png(256),
        &profile.linux.category,
        &appimage_runtime,
    )?;
    std::fs::write(path, image).map_err(|error| format!("{}: {error}", path.display()))
}

fn build_apk(
    profile: &Profile,
    bundle: &[u8],
    icon: &AppIcon,
    path: &Path,
    report: &mut Report,
) -> Result<(), String> {
    let template = runtime(Target::AndroidApk)?;
    let signer = crate::apk::Signer::load_or_create()?;
    report.log(
        Level::Info,
        format!(
            "signing with this computer's key in {}",
            signer.folder.display()
        ),
    );
    let plan = crate::apk::ApkPlan {
        manifest: &profile.manifest,
        bundle,
        icon,
        drop_open_with: profile.android.drop_open_with,
    };
    let apk = crate::apk::build(&template, &plan, &signer)?;
    std::fs::write(path, apk).map_err(|error| format!("{}: {error}", path.display()))
}

/// Reads the bundle back out of a built app.
pub fn verify(target: Target, path: &Path) -> Result<q0qir::Bundle, String> {
    let fail = |error: String| format!("{} did not check out: {error}", path.display());
    match target {
        Target::WindowsX64 | Target::WindowsArm64 => {
            q0qir::Bundle::read_from_file(path, true).map_err(|error| fail(error.to_string()))
        }
        Target::LinuxAppImage => {
            let bytes = std::fs::read(path).map_err(|error| fail(error.to_string()))?;
            if elf_size(&bytes).is_none() {
                return Err(fail("no ELF runtime at the start".into()));
            }
            crate::appimage::find_bundle(&bytes).map_err(|error| fail(error.to_string()))
        }
        Target::AndroidApk => {
            let bytes = std::fs::read(path).map_err(|error| fail(error.to_string()))?;
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
                .map_err(|error| fail(error.to_string()))?;
            let mut asset = archive
                .by_name(&format!("assets/{}", q0qir::ANDROID_ASSET))
                .map_err(|error| fail(error.to_string()))?;
            let mut contents = Vec::new();
            std::io::Read::read_to_end(&mut asset, &mut contents)
                .map_err(|error| fail(error.to_string()))?;
            q0qir::Bundle::decode(&contents, true).map_err(|error| fail(error.to_string()))
        }
    }
}

/// Where an ELF program's last section header table ends: the start of the
/// image an AppImage runtime carries.
pub fn elf_size(bytes: &[u8]) -> Option<u64> {
    if bytes.get(..4)? != b"\x7fELF" || *bytes.get(4)? != 2 {
        return None;
    }
    let u64_at = |at: usize| Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?));
    let u16_at = |at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let section_headers = u64_at(0x28)?;
    let entry_size = u64::from(u16_at(0x3A)?);
    let count = u64::from(u16_at(0x3C)?);
    Some(section_headers + entry_size * count)
}

fn zip_one(path: &Path) -> std::io::Result<()> {
    let zip_path = path.with_extension(format!(
        "{}.zip",
        path.extension()
            .map(|ext| ext.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let file = std::fs::File::create(zip_path)?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    writer.start_file(name, options)?;
    std::io::Write::write_all(&mut writer, &std::fs::read(path)?)?;
    writer.finish()?;
    Ok(())
}

fn write_checksums(folder: &Path, artifacts: &[Artifact], zipped: bool) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let mut lines = Vec::new();
    for artifact in artifacts {
        let mut files = vec![artifact.path.clone()];
        if zipped {
            let ext = artifact
                .path
                .extension()
                .map(|ext| ext.to_string_lossy().into_owned())
                .unwrap_or_default();
            files.push(artifact.path.with_extension(format!("{ext}.zip")));
        }
        for file in files {
            let bytes =
                std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))?;
            let digest = Sha256::digest(&bytes);
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            lines.push(format!("{hex}  {name}"));
        }
    }
    std::fs::write(folder.join("SHA256SUMS.txt"), lines.join("\n") + "\n")
        .map_err(|error| error.to_string())
}

pub fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_reads_like_a_date() {
        let date = today();
        assert_eq!(date.len(), 10);
        assert!(date.starts_with("20"));
        assert_eq!(&date[4..5], "-");
    }

    #[test]
    fn a_rebuilt_runtime_loses_the_old_bundle() {
        let mut runtime = b"MZ program".to_vec();
        let plain = runtime.clone();
        runtime.extend(
            q0qir::Bundle {
                manifest: q0qir::Manifest::default(),
                icon_png: None,
                movie: vec![1, 2, 3],
            }
            .encode(),
        );
        assert_eq!(bare(runtime), plain);
    }

    #[test]
    fn sizes_read_for_people() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(4096), "4 KB");
        assert_eq!(size(5 << 20), "5.0 MB");
    }
}
