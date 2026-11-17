# ARCHITECTURE.md — `sn-tui` Design & Phase 0 Findings

## 1. Phase 0 Research Summary

### 1.1 Upstream `.note` / `.mark` Parsers
- **`RohanGautam/supernote_pdf` (MIT):** Single-file Rust CLI (`src/main.rs`, 582 lines, no `lib.rs`). Demonstrated that direct minimal PDF 1.7 generation using `flate2` zlib streams avoids heavy PDF writer crates and runs dramatically faster than Python converters. We adapt and extend its core `.note` parser/PDF concepts into a clean modular library, adding:
  - `/DeviceGray` PDF image encoding (1/3 RAM and ~3x smaller PDFs than RGB for monochrome e-ink pages).
  - Landscape orientation (`ORIENTATION=1090`), `all_blank` RLE handling (`0x400` vs `0x4000`), full X & X2 color tables, sparse `.mark` page numbers, selective layer visibility, and native `RECOGNTEXT` extraction.
- **`jya-dev/supernote-tool` (`supernotelib`, Apache-2.0):** Canonical reference for Supernote binary quirks:
  - `RATTA_RLE` X vs X2 color codes (`0x61` black, `0x62` transparent, `0x63`/`0x9d`/`0x9e` dark gray, `0x64`/`0xc9`/`0xca` gray, `0x65` white, plus raw anti-aliased grayscale bytes on X2).
  - `RECOGNTEXT` stores base64-encoded UTF-8 JSON (`{"elements": [{"type": "Text", "label": "...", "words": [{"bounding-box": {"y": ...}}]}]}`), enabling zero-Python OCR text extraction in pure Rust.
  - CLI flags verified for optional Vector PDF (`convert -t pdf --pdf-type vector -a`) and SVG (`convert -t svg -a`) subprocess exports.

### 1.2 TUI & Terminal Graphics
- **`ratatui 0.30.2` + `ratatui-image 11.1.0` + `crossterm 0.29.0`:** Verified version-compatible (`ratatui-image 11.1.0` depends on `ratatui ^0.30.1` and `image ^0.25.6`).
- `Picker::from_query_stdio()` auto-detects Kitty graphics protocol, Sixel, iTerm2, and cell pixel dimensions before entering raw mode, falling back safely to Unicode half-blocks when stdio querying is unsupported.

### 1.3 Sync Protocols (Primary Focus: Supernote Private Cloud & Supernote Cloud)
- **Official Ratta Private Cloud (Docker) & Free Supernote Cloud (`https://cloud.supernote.com/api`):**
  Verified against `julianprester/sncloud` and `allenporter/supernote`:
  - Challenge: `POST /api/official/user/query/random/code` (`{"countryCode":"1","account":"<email>"}`) -> `{"randomCode","timestamp"}`
  - Login: `POST /api/official/user/account/login/new` with `password = SHA256(MD5(password) + randomCode)` -> `{"token"}` sent as `x-access-token` header.
  - Open Private Cloud (`allenporter/supernote`) additionally accepts static `Authorization: Bearer sn_...` tokens on the same file routes.
  - Directory listing: `POST /api/file/list/query` (`{"directoryId":0,"pageNo":1,"pageSize":100,"order":"time","sequence":"desc"}`) -> `userFileVOList[]` (`id`, `fileName`, `isFolder` `"Y"`/`"N"`, `size`, `md5`, `updateTime`).
  - Download: `POST /api/file/download/url` (`{"id":<id>,"type":0}`) -> `{"url"}` -> HTTP `GET`.
  - Explicit upload: `POST /api/file/upload/apply` -> `PUT` file bytes (with AWS S3 headers if returned, else `x-access-token`) -> `POST /api/file/upload/finish`.
- **Standard WebDAV:** `PROPFIND` (`Depth: 1`) parsed with `quick-xml`, Basic or Bearer auth, `GET` pull and `PUT` explicit upload.
- **No external sync wrappers:** Deliberately omits `rclone` and port-8089 scraping to keep the binary lean and focused.

---

## 2. Supernote `.note` & `.mark` Binary Format

```
+-------------------------------------------------------------+
| 0x00..0x04  File Type Magic: "note" or "mark" (4 bytes)     |
| 0x04..0x18  Signature: "SN_FILE_VER_YYYYNNNN" (20 bytes)    |
+-------------------------------------------------------------+
| Variable-length blocks referenced by 32-bit LE offsets:     |
|   Each block = [u32_le length][length bytes payload]        |
|   - Header metadata block (`FILE_FEATURE`): <KEY:VALUE>...  |
|   - Page metadata blocks (`PAGE1`, `PAGE2`, ... sparse ok)  |
|   - Layer metadata blocks (`BGLAYER`, `MAINLAYER`, `LAYER1..3`)
|   - Layer bitmap blocks (`RATTA_RLE` or `PNG`)              |
|   - Recognition text blocks (`RECOGNTEXT`: base64 JSON)     |
+-------------------------------------------------------------+
| Footer metadata block: <FILE_FEATURE:off><PAGE1:off>...     |
+-------------------------------------------------------------+
| EOF-4..EOF  Footer offset (u32 little-endian)               |
+-------------------------------------------------------------+
```

### Device Canvas Dimensions
| `APPLY_EQUIPMENT` | Device | Portrait Dimensions |
|---|---|---|
| `N5` | Supernote A5 X2 (Manta) | `1920 × 2560` |
| `N6` / `A6X2` | Supernote A6 X2 (Nomad) | `1404 × 1872` |
| `A5X` / `A6X` / default | Supernote A5 X / A6 X | `1404 × 1872` |

When a page has `ORIENTATION:1090`, width and height are swapped (`height × width`).

---

## 3. PDF + `.pdf.mark` Hybrid Architecture
- **Viewing (Zero linked C PDF bloat):** Uses host `pdfinfo` for page count and `pdftoppm -png -r <dpi> -f <p> -l <p> -singlefile` (from Artix `poppler`) to rasterize only the currently viewed PDF page directly into memory (`stdout`), then alpha-composites the matching `.mark` `PAGE<p>` RGBA bitmap on top.
- **Exporting (Lossless Vector Preservation):** Pure-Rust `lopdf` loads the original `<doc>.pdf`, embeds each annotated `.mark` page as a compressed RGB/Gray + 8-bit Alpha `SMask` PDF XObject, and appends a lightweight `q w 0 0 h 0 0 cm /SnMark Do Q` drawing command to that page—preserving original selectable text, fonts, and hyperlinks.

---

## 4. Resource Efficiency & Caching Strategy
- **Memory Budget:** Small 6-page in-memory `LruCache` per session (~15–30 MB peak decoded pages); bounded `rayon` pool (`min(cpus, 4)`) prevents CPU/RAM spikes during batch exports.
- **Disk Cache:** `~/.cache/sn-tui/pages/<hash>_p<page>.png` (compact 8-bit grayscale PNGs for `.note`, RGBA PNGs for PDF+`.mark`) with automatic LRU eviction capped at `128 MB` (configurable).
- **OCR Index:** `~/.cache/sn-tui/ocr_index.json` keyed by file path + `(mtime, size)` so `/` fuzzy search scans text instantly without re-reading `.note` files on disk.
