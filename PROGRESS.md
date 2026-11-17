# PROGRESS.md — `sn-tui` Engineering Log

## Milestone Checklist
- [x] **Milestone 1** (`3d64102`): Repo init, living docs (`AGENTS.md`, `ARCHITECTURE.md`, `PROGRESS.md`), `Cargo.toml`, and synthetic `.note`/`.mark`/PDF test fixture generator.
- [x] **Milestone 2** (`6d7491e`): Core pure-Rust `.note` & `.mark` parser, `RATTA_RLE`/`PNG` renderer, `RECOGNTEXT` extractor, and PDF + `.pdf.mark` composite viewer engine.
- [/] **Milestone 3:** Dual export pipeline (minimal `/DeviceGray` raster PDF, `lopdf` lossless `.mark` PDF overlay, PNG/TXT/MD, optional `supernote-tool` bridge), config, bounded LRU cache, and `inspect`/`convert` CLI subcommands.
- [ ] **Milestone 4:** 3-pane Yazi-style TUI (`ratatui` + `ratatui-image`), non-blocking background render worker, Vim keybindings, fuzzy `/` search, and Export/Sync/Upload modals.
- [ ] **Milestone 5:** Native Sync engine (Official Private Cloud, Free Supernote Cloud, `allenporter/supernote` Bearer, WebDAV) with pull-only mirror + explicit upload, plus `notify` live filesystem watcher.
- [ ] **Milestone 6:** Polish, `clippy -D warnings`, release binary size & `ldd` audit, `README.md`, and final verification.

## Engineering Log
- **2026-11-17 (Phase 0):** Inspected `RohanGautam/supernote_pdf`, `jya-dev/supernote-tool`, `julianprester/sncloud`, `allenporter/supernote`, and verified crates.io versions (`ratatui 0.30.2`, `ratatui-image 11.1.0`, `crossterm 0.29`, `ureq 3.5`, `lopdf 0.47`). Confirmed Artix host has `rustc 1.99.1`, `pdftoppm`, and `pdfinfo`. Locked in non-`tokio` architecture with `ureq` + bounded `rayon`.
