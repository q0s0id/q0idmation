# Idle editor memory

The editor keeps Windows script fallback fonts in read-only file mappings. The
fixed cache owns the mapping and its file handle for the lifetime of the process;
egui receives borrowed font slices. On other platforms, or if mapping fails, one
owned buffer is shared by every font definition.

This matters with egui 0.27: `FontData::from_owned` clones the complete font file
when font definitions are cloned and again when ab_glyph parses the font. The
Chinese, Japanese, Korean and symbol fallbacks on the development machine total
49,291,072 bytes. Loading them as owned buffers cost roughly 141 MiB in duplicate
private memory before any project artwork was present. Cloning the definitions
for inline text editing could add another two complete copies.

The missing-font audit only starts system font discovery when the project
contains editable text assets. Empty projects and ordinary vector drawings do
not scan the font catalogue or poll for its completion.

Run the headless font/UI measurement with:

```powershell
cargo run -p q0editor --example perf_idle_memory --release --locked
```

It measures process working set and private memory on Windows after font
installation, 600 idle UI frames, multilingual rendering, and cloning font
definitions. It does not open or inspect the user's editor window. Its numbers
isolate the CPU font/UI cost; they are not total GUI process or GPU memory.

Recorded on 2026-10-26, using the same Windows fonts and release build:

| Stage | Before working set (MiB) | After working set (MiB) | Before private (MiB) | After private (MiB) |
| --- | ---: | ---: | ---: | ---: |
| First blank UI frame | 147.43 | 6.20 | 144.57 | 3.27 |
| After 600 blank UI frames | 147.57 | 6.32 | 144.79 | 3.50 |
| Multilingual UI frame | 147.61 | 6.54 | 144.79 | 3.50 |
| Two font-definition clones | 242.52 | 7.43 | 239.45 | 3.94 |

Both runs confirmed Cyrillic, Chinese, Japanese and Korean glyph coverage.
Definition-owned font data fell from 49,750,252 bytes to 459,180 bytes; the large
fallback files now use shared mappings whose untouched pages stay out of the
working set.

Validation: `cargo test -p q0editor --lib --release --locked --offline --quiet`
passed 742 tests with 1 ignored and no failures. The release editor binary was
rebuilt with `cargo build -p q0editor --bin q0editor --release --locked --offline`.
The user's running window was not used for the memory comparison.
