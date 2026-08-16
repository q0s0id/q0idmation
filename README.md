# q0idmation

q0idmation is a free and open-source 2D animation toolkit written in Rust. The current beta
ships two Windows applications:

- `q0editor` creates and edits `.q1s` projects and exports `.q0s` movies.
- `q0player` opens and plays `.q0s` movies.

`q0shell` and `q0term` remain developer previews and are not included in the
beta installers.

## Build

Install the stable Rust toolchain and the Microsoft C++ build tools.
For the usual q0editor build, double-click `build.bat` or run:

```powershell
.\build.ps1
```

The script keeps the full Cargo output in `out\build-logs\`, writes a compact
result to `out\last-build.txt`, and prints only the useful status in the console.
Available modes are `debug` (default), `release`, `check`, `verify`, and `run`:

```powershell
.\build.ps1 release
.\build.ps1 verify
.\build.ps1 run
```

To build the complete workspace manually:

```powershell
cargo build --workspace --locked
```

Launch the editor manually:

```powershell
cargo run -p q0editor
```

Launch the player with a movie:

```powershell
cargo run -p q0player -- path\to\movie.q0s
```

## Verify

```powershell
cargo fmt --all -- --check
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## Windows installers

Build both release binaries and independent per-user installers with:

```powershell
powershell -ExecutionPolicy Bypass -File .\installer\build.ps1
```

See [installer/README.md](installer/README.md) for prerequisites and output
paths.

Before sharing the beta, follow the compact [10-minute beta test](BETA_TEST.md).

## Beta notes

- Keep `.q1s` source projects. `.q0s` is the playback export.
- `Paint Bucket` fills enclosed raw-vector regions and can use intersections
  between open strokes as region boundaries.
- Use the reproducible report template in [BETA_TEST.md](BETA_TEST.md), including
  the Build ID and installer SHA256.

Language documentation lives in [docs/q0lang/README.md](docs/q0lang/README.md).

## License

q0idmation is free software licensed under the GNU General Public License v3.0
or later (`GPL-3.0-or-later`). See [LICENSE](LICENSE).
