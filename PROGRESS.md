# PROGRESS.md — `sn-tui` Engineering Log

## Milestone Checklist
- [x] **Milestone 1** (`3d64102`): Repo init, living docs (`AGENTS.md`, `ARCHITECTURE.md`, `PROGRESS.md`), `Cargo.toml`, and synthetic `.note`/`.mark`/PDF test fixture generator.
- [x] **Milestone 2** (`6d7491e`): Core pure-Rust `.note` & `.mark` parser, `RATTA_RLE`/`PNG` renderer, `RECOGNTEXT` extractor, and PDF + `.pdf.mark` composite viewer engine.
- [x] **Milestone 3** (`fd54de7`): Dual export pipeline (minimal `/DeviceGray` raster PDF, `lopdf` lossless `.mark` PDF overlay, PNG/TXT/MD, optional `supernote-tool` bridge), config, bounded LRU cache, and `inspect`/`convert` CLI subcommands.
- [x] **Milestone 4** (`5fcb75b`): 3-pane Yazi-style TUI (`ratatui` + `ratatui-image`), non-blocking background render worker, Vim keybindings, fuzzy `/` search, and Export/Sync/Upload modals.
- [x] **Milestone 5** (`2a1354e`): Native Sync engine (Official Private Cloud, Free Supernote Cloud, `allenporter/supernote` Bearer, WebDAV) with pull-only mirror + explicit upload, plus `notify` live filesystem watcher.
- [x] **Milestone 6** (final): `README.md`, release build (`6.0 MB` stripped LTO), `ldd` audit (links only `libc`, `libm`, `libgcc_s`), `cargo clippy --all-targets -- -D warnings`, and 17/17 passing tests.
- [x] **Milestone 7** (`00e05d8`): UI Ergonomics, Alternative Layouts & Critical Rendering Fixes:
  - Decoupled zoom pan (`↑↓←→` in all 4 directions when zoomed) from library navigation (`j`/`k` strictly move file tree, `G` bottom, `Home` top).
  - Caps Lock harmonization: all action commands (`q`/`Q`, `e`/`E`, `s`/`S`, `u`/`U`, `r`/`R`, `v`/`V`, `f`/`F`, `i`/`I`) are case-insensitive.
  - 4 View Layouts (`Standard3Pane`, `WideReader`, `BottomInspector`, `FullFocus`) cycled via `v`, instant Full Focus / Fullscreen canvas via `f`/`F` with `Esc` restore.
  - Dark / Invert color mode (`i`/`I`) for dark terminal environments.
  - Visual zoom and pan indicator with reset hints on the canvas border.
  - Letterboxing fix for non-4:3 PDFs (e.g. 16:9 Calculus III lecture slides on 4:3 Manta canvas) in both live preview and lossless PDF export.
  - Marker/highlighter semitransparency (`0x66`, `0x67`, `0x68`, `0x9e`, `0xca` ~43% opacity) with Porter-Duff source-over alpha blending so highlighters don't black out text.
  - 22/22 unit and integration tests passing; installed release binary in `~/.local/bin/sn-tui`.
- [x] **Milestone 8** (`750cea9`): PDF Canvas Margin Expansion, Zoom Viewport Scaling & Mouse Panning:
  - Top-aligned PDF slide placement (`Y=0`, `X=centered`) with remaining margin at bottom in both preview and lossless export, aligning handwriting with 100% precision with zero text collisions.
  - Expanded PDF MediaBox & CropBox in `src/export/mark_pdf.rs` to match the Supernote canvas (e.g. 4:3) with clean white background and shifted vector stream, preserving 100% of annotations written off the slide edge.
  - Zoom viewport upscaling: `crop_for_zoom` resizes the cropped buffer back to full canvas dimensions, eliminating the viewer shrinking issue and filling the center pane completely.
  - Mouse left-click drag panning in zoomed canvas with `crossterm::event::EnableMouseCapture` and clean cleanup on drop.
  - Automatic disk cache invalidation versioning (`CACHE_VERSION = "v3"`) to prevent stale cached renders.
  - 23/23 unit and integration tests passing; release binary compiled and installed to `~/.local/bin/sn-tui`.

## Engineering Log
- **2026-11-17 (Phase 0):** Inspected `RohanGautam/supernote_pdf`, `jya-dev/supernote-tool`, `julianprester/sncloud`, `allenporter/supernote`, and verified crates.io versions (`ratatui 0.30.2`, `ratatui-image 11.1.0`, `crossterm 0.29`, `ureq 3.5`, `lopdf 0.47`). Confirmed Artix host has `rustc 1.99.1`, `pdftoppm`, and `pdfinfo`. Locked in non-`tokio` architecture with `ureq` + bounded `rayon`.
- **2026-11-17 (Milestone 1–2):** Built zero-regex `<KEY:VALUE>` parser supporting `.note` and sparse `.mark` page indices, X/X2 `RATTA_RLE` (including `all_blank` `0x400` runs & anti-aliased grays), `PNG` layers, landscape `ORIENTATION:1090`, native `RECOGNTEXT` JSON extraction, and `pdftoppm` + `.pdf.mark` alpha compositing.
- **2026-11-17 (Milestone 3):** Implemented direct `/DeviceGray` PDF 1.7 writer (~3x leaner than RGB), pure-Rust `lopdf` lossless `.mark` XObject overlay preserving selectable PDF vector text, optional `supernote-tool`/`uvx` bridge, `0600` config checks, and bounded disk LRU cache + lazy OCR index.
- **2026-11-17 (Milestone 4):** Implemented 3-pane `ratatui` + `ratatui-image` TUI with `Picker::from_query_stdio()` fallback, coalesced background render worker + page prefetch, zoom/pan, layer toggles, OCR fuzzy `/` search, and Export/Upload/Help modals.
- **2026-11-17 (Milestone 5):** Implemented native `ureq` clients for Official Private Cloud, Free Supernote Cloud, `allenporter/supernote` Bearer Cloud, and WebDAV (`PROPFIND` with `allow_non_standard_methods(true)` caught by loopback wire test!), incremental pull-only mirror manifest, explicit upload, and debounced `notify` watcher.
- **2026-11-17 (Milestone 6):** Finalized `README.md`, verified `6.0 MB` stripped release binary with zero GUI/X11/systemd shared library links (`libc`, `libm`, `libgcc_s` only), zero clippy warnings (`-D warnings`), and 17/17 passing tests.
- **2026-11-17 (Post-release fix):** Fixed Private Cloud reverse-proxy URL mismatch (`https://<host>:8080/api/oss/download` rewritten to configured `base_url` origin), added periodic `sync_state.json` checkpointing and TUI in-flight sync guard, and verified live pull + `1920x2560` Manta `SN_FILE_VER_20260016` `.note` rendering (20/20 tests passing).
