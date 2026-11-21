# AGENTS.md — `sn-tui` Engineering Guide & Invariants

## Build, Lint & Test Commands
- Check: `cargo check --all-targets`
- Unit & integration tests: `cargo test`
- Strict lints: `cargo clippy --all-targets -- -D warnings`
- Release build: `cargo build --release`
- Verify zero GUI shared-lib links: `ldd target/release/sn-tui`

## Core Architectural Invariants
1. **Zero GUI Toolkits / Zero Bloat:** Never depend on GTK, Qt, WebKit2GTK, Electron, X11, or `tokio`/`reqwest`. Target OS is Artix Linux (systemd-free) under MangoWC (Wayland).
2. **Lean Concurrency:** Use OS threads (`std::thread`) + `std::sync::mpsc` + bounded `rayon` pool (`RAYON_NUM_THREADS` / `ThreadPoolBuilder`) + blocking `ureq` (`rustls`).
3. **Minimal Dependencies:** Keep `image` limited to `png` codec only (`default-features = false`). Parse `.note`/`.mark` `<KEY:VALUE>` blocks linearly without `regex`. Write `.note` raster PDFs directly with `/DeviceGray` + `flate2`; use `lopdf` specifically for lossless `.mark` XObject overlays onto existing PDFs.
4. **Zero-Panic TUI:** Restore terminal raw mode and alternate screen reliably on exit/error. Never panic on corrupt `.note` files, missing `pdftoppm`, missing `supernote-tool`, or network errors—surface actionable messages in the TUI status/log pane.
5. **Safe Sync by Default:** Cloud/Private Cloud/WebDAV sync is **pull-only** (never deletes or mutates remote notes). Uploads happen only via explicit `sn-tui upload` or the TUI `u` action.
6. **Credentials Hygiene:** Plaintext credentials in `~/.config/sn-tui/config.toml` trigger a Unix permission check (`0600`) with a warning if overly permissive. Environment variables (`SN_ACCOUNT`, `SN_PASSWORD`, `SN_TOKEN`) always override config file values.
7. **Git Hygiene:** Local repository only (`main` branch, no remotes). Commit atomically after each milestone using Conventional Commits and update `PROGRESS.md`.

## Module Boundaries
- `src/note/`: Pure-Rust `.note` & `.mark` binary parser, `RATTA_RLE` X/X2 & `PNG` layer decoder, `RECOGNTEXT` JSON extractor, and synthetic test fixture builder.
- `src/pdf/`: Lightweight PDF page inspection (`pdfinfo`), single-page on-demand rasterization (`pdftoppm`), and `.pdf.mark` RGBA alpha compositing.
- `src/export/`: Direct `/DeviceGray` minimal raster PDF writer, `lopdf` lossless `.mark` PDF overlay exporter, PNG/TXT/MD exporters, and optional `supernote-tool` subprocess bridge (Vector PDF & SVG).
- `src/cache.rs`: Bounded disk LRU (`~/.cache/sn-tui/pages/`, default 128 MB), lazy OCR index (`ocr_index.json`), and small in-memory `LruCache` (4–6 pages).
- `src/sync/`: Native `ureq` clients for Official Ratta Private Cloud / Free Supernote Cloud / `allenporter/supernote` Bearer Cloud and WebDAV, plus `notify` debounced filesystem watcher.
- `src/tui/`: 3-pane Yazi-style `ratatui` + `ratatui-image` UI, background render/sync/export worker threads, and interactive modals.
- `src/config.rs` & `src/cli.rs`: TOML config loading/scaffolding and `clap` CLI subcommands.

## TUI Keybindings
| Key | Action |
|---|---|
| `j` / `k` (or `G` / `Home`) | Move selection down / up in library tree (`G` bottom, `Home` top) |
| `↑` / `↓` / `←` / `→` | Pan zoomed canvas in all 4 directions (or navigate tree/folders when 1.0x) |
| `h` / `l` or `Enter` | Collapse / expand directory |
| `n` / `p` or `J` / `K` | Next / previous page |
| `g` | Jump to page number |
| `v` | Cycle view layout (`Standard3Pane` -> `WideReader` -> `BottomInspector` -> `FullFocus`) |
| `f` / `F` | Toggle Full Focus / Fullscreen canvas mode |
| `i` / `I` | Toggle Dark / Invert color mode (white e-ink -> dark slate) |
| `1`–`5` | Toggle layer visibility (`.note` layers or `.mark` overlay on PDF) |
| `+` / `-` / `0` | Zoom in / out / reset zoom |
| `Left-click Drag` | Pan zoomed canvas smoothly with mouse |
| `/` | Fuzzy search filenames & indexed OCR text |
| `e` / `E` | Open Export modal (current file or directory) |
| `s` / `S` | Run pull-sync for active cloud/WebDAV profile |
| `u` / `U` | Upload selected local PDF/template to remote folder |
| `r` / `R` | Refresh workspace tree & invalidate active page cache |
| `?` | Toggle keybindings help overlay |
| `q` / `Q` / `Ctrl+C` | Quit cleanly |
