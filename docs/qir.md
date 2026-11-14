# QIR and Q0Build

QIR (q0 Integrated Runtime) turns a q0s movie into an app of its own, the
way a Flash projector carried its SWF. **Q0Build** is the program that makes
them: Windows `.exe` (x64 and ARM64), Linux `.AppImage` and Android `.apk`.

An app is q0player with a **bundle** attached: the movie, a manifest of
settings and an icon. The runtime finds the bundle from the end of its own
file, so nothing is unpacked to disk and the app is one file you can send.

```text
[manifest JSON][icon PNG][movie][trailer: 40 bytes]
trailer = "Q0QIR\0\0\x01", payload length (u64), manifest length (u32),
          icon length (u32), movie length (u64), FNV-1a 64 of the payload
```

| target | how the bundle travels |
| --- | --- |
| Windows | appended to `q0player.exe`; the exe's icon and file details are replaced first |
| Linux | appended to the Linux q0player inside the AppImage's squashfs image |
| Android | `assets/qir/app.qir` in q0player's APK, renamed to the app's package and signed |

## Using Q0Build

1. **Open movie or profile**: pick the `.q0s`/`.q1s`. Its settings are kept
   beside it as `<movie>.q0build`.
2. Fill in **App** (name, id, version), **Window**, **Runtime** and **Data &
   debug**. Every setting has a line under it saying what it does.
3. Tick **Targets**. Each needs its runtime (see below).
4. **Test run** (F5) plays the app exactly as built, without building.
5. **Build** (Ctrl+B) writes the apps, checks each one by reading its bundle
   back, and can zip them and write `SHA256SUMS.txt`.

Builds also run without a window, for scripts and CI:

```text
q0build --build game.q0build
```

### Tools

- **Inspector**: a movie's scenes, timelines and scripts, and which assets
  make it heavy.
- **Script check**: every symbol and frame script through the q0lang parser,
  with each problem's place and line.
- **Bundle reader**: opens any app Q0Build made, shows its settings, takes
  them into the profile or saves the movie back out.
- **Icon studio**: the icon at every size on light and dark, `.ico` and
  512 px PNG export.

## Runtimes

Q0Build looks in `runtimes/` beside itself, then (run from a q0idmation
checkout) in that checkout's build outputs:

| target | file | build it with |
| --- | --- | --- |
| Windows x64 | `runtimes/windows-x64/q0player.exe` | `cargo build --release -p q0player` |
| Windows ARM64 | `runtimes/windows-arm64/q0player.exe` | `cargo build --release -p q0player --target aarch64-pc-windows-msvc` |
| Linux x86-64 | `runtimes/linux-x86_64/q0player` | `cargo build --release -p q0player` on Linux |
| Android arm64 | `runtimes/android-arm64/q0player.apk` | `android\build.ps1 -Mode apk -App player` |

AppImages also need the AppImage type-2 runtime. Q0Build downloads it once
from the AppImage project with `curl`, or uses
`runtimes/linux-x86_64/appimage-runtime-x86_64`. The image inside is stored
uncompressed (the movie is packed already); people make it runnable with
`chmod +x` after downloading.

APKs are signed with APK Signature Scheme v2 and a key Q0Build makes on first
use in `<config>/q0build/android-signing/`. Android installs an update only
over an app signed with the same key, so back that folder up.

## The runtime's settings

The manifest decides, among others: the window's size, frame and start mode;
anti-aliasing and v-sync; whether the movie pauses behind other windows; what
Esc, F11 and q0player's F9/F4 do; the cursor; one copy at a time; the splash;
where saves go (or beside the app, portable); a checksum check at start; the
log file, a console window and script-error boxes. Scripts reach the app
through [`q0.app`](q0lang/builtins.md#q0app).

When the window and the stage differ in shape, the space around the movie is
black by default. The Window page can make it the stage colour or any colour,
and a script can change it at any time:

```
import q0.window
q0window.background! "#1b1b22"   // or "stage", "black", 27, 27, 34, or null for the app's choice
```

The splash (off by default) has its own colour, length and line of text. It
can show the icon, and a click, tap or key can skip it.

`q0player --qir file.qir` runs any bundle as its app would; Q0Build's test
runs use it.
