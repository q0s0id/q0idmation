# q0idmation Windows installers

q0idmation has two independent per-user NSIS installers for Windows x64:

| File pattern | Contents | Association |
|---|---|---|
| `dist\q0editor-<version>-windows-x64-setup.exe` | `q0editor.exe` only | `.q1s` |
| `dist\q0player-<version>-windows-x64-setup.exe` | `q0player.exe` only | `.q0s`, `.q0v` |

The release version comes from `cargo metadata --no-deps`. The `q0editor` and
`q0player` package versions must match or packaging stops before NSIS runs.

The `dist` directory also receives:

- `SHA256SUMS.txt` with both installer hashes;
- `BUILD-INFO.txt` with the Build ID, version, UTC build time, Rust/Cargo/NSIS
  versions, Rust host triple, and exact artifact sizes and hashes.

The old ambiguous names `q0editor-setup.exe` and `q0player-setup.exe` are
removed. Artifacts for other explicit versions are not removed.

Each application has its own install directory:

- `%LOCALAPPDATA%\Programs\q0idmation\q0editor\`
- `%LOCALAPPDATA%\Programs\q0idmation\q0player\`

Installing or removing one application does not alter the other application's
files.

## Requirements

- Windows x64 with Rust host `x86_64-pc-windows-msvc`;
- Rust and Cargo with the `rustfmt` and `clippy` components;
- Python 3.10 or newer for the generated icons and BMP files;
- NSIS 3.x. The script also checks the standard NSIS install directories.
- Git with a clean working tree whose current branch is pushed to `origin`.

## Publication build

Run from the workspace root:

```powershell
powershell -ExecutionPolicy Bypass -File .\installer\build.ps1
```

Alternatively, start `installer\build.bat`. The script:

1. locates Python, Git, NSIS and the complete Rust toolchain, including custom
   `.rustup` and `.codex` toolchain paths;
2. reads the shared q0editor/q0player version from Cargo metadata;
3. requires a clean, pushed Git commit and records it in `BUILD-INFO.txt`;
4. requires formatting, workspace check, tests and strict Clippy to pass;
5. generates installer assets with `installer\build_assets.py` and verifies
   that generation does not modify the committed source tree;
6. builds locked release binaries for q0editor and q0player;
7. builds both versioned installers in an isolated staging directory;
8. verifies sizes and SHA256 hashes, creates the manifests, then publishes the
   complete set to `installer\dist`.

On failure, only files from the current staging attempt are cleaned. Previous
versioned builds remain in place.

Before sharing the files, complete [`BETA_TEST.md`](../BETA_TEST.md).

## Installer behavior

q0editor registers `.q1s`; q0player registers both `.q0s` and `.q0v`. All associations live
under `HKCU\Software\Classes` and pass the selected path as the first command
line argument. Each installer creates a Start menu shortcut and offers an
optional desktop shortcut.

## Manual local NSIS build

Use `build.ps1` for anything that will be published because it creates the
manifests and enforces matching versions. For a local NSIS check, pass the Cargo
version manually:

```powershell
python .\installer\build_assets.py
cargo build --release --locked -p q0editor -p q0player

$metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
$editorVersion = ($metadata.packages | Where-Object name -eq 'q0editor').version
$playerVersion = ($metadata.packages | Where-Object name -eq 'q0player').version
if ($editorVersion -ne $playerVersion) { throw "package versions do not match" }
if ($editorVersion -notmatch '^(\d+)\.(\d+)\.(\d+)') { throw "invalid version" }
$numericVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"

Set-Location .\installer
makensis /V2 "/DAPP_VERSION=$editorVersion" "/DAPP_VERSION_NUMERIC=$numericVersion" q0player-setup.nsi
makensis /V2 "/DAPP_VERSION=$editorVersion" "/DAPP_VERSION_NUMERIC=$numericVersion" q0editor-setup.nsi
```

A direct `makensis q0editor-setup.nsi` or `makensis q0player-setup.nsi` is also
safe for a local check. Both scripts use the shared fallback in `version.nsh`
and still produce a versioned filename. Update that fallback when changing the
Cargo package version.

The beta installers are not code-signed, so Windows may warn about an unknown
publisher.
