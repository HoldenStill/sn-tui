//! `RATTA_RLE` (X & X2 series) and `PNG` layer decoding and compositing.

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::{Context, Result, bail};
use image::{GrayImage, Luma, Rgba, RgbaImage, imageops};

use crate::note::parser::{Notebook, Page, read_block};

/// Safety cap for compressed bitmap block reads (32 MB).
const MAX_BITMAP_BLOCK_BYTES: usize = 32 * 1024 * 1024;

const SPECIAL_LENGTH_MARKER: u8 = 0xff;
const SPECIAL_LENGTH: usize = 0x4000;
const SPECIAL_LENGTH_FOR_BLANK: usize = 0x0400;

/// Decodes a `RATTA_RLE` byte stream into raw Supernote color code bytes of length `width * height`.
pub fn decode_ratta_rle(
    compressed: &[u8],
    width: u32,
    height: u32,
    all_blank: bool,
) -> Result<Vec<u8>> {
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .context("page dimensions overflow")?;
    let mut out = Vec::with_capacity(expected_len);

    let mut i = 0usize;
    let mut holder: Option<(u8, u8)> = None;

    while i + 1 < compressed.len() && out.len() < expected_len {
        let color = compressed[i];
        let length_code = compressed[i + 1];
        i += 2;

        if let Some((prev_color, prev_len_code)) = holder.take() {
            if color == prev_color {
                let combined = 1usize + length_code as usize + (((prev_len_code & 0x7f) as usize + 1) << 7);
                push_clamped(&mut out, color, combined, expected_len);
                continue;
            } else {
                let held_len = ((prev_len_code & 0x7f) as usize + 1) << 7;
                push_clamped(&mut out, prev_color, held_len, expected_len);
                if out.len() >= expected_len {
                    break;
                }
            }
        }

        if length_code == SPECIAL_LENGTH_MARKER {
            let len = if all_blank {
                SPECIAL_LENGTH_FOR_BLANK
            } else {
                SPECIAL_LENGTH
            };
            push_clamped(&mut out, color, len, expected_len);
        } else if length_code & 0x80 != 0 {
            holder = Some((color, length_code));
        } else {
            push_clamped(&mut out, color, length_code as usize + 1, expected_len);
        }
    }

    if let Some((color, tail_code)) = holder
        && out.len() < expected_len
    {
        let gap = expected_len - out.len();
        let mut tail_len = 0usize;
        for shift in (0..8).rev() {
            let candidate = ((tail_code & 0x7f) as usize + 1) << shift;
            if candidate <= gap {
                tail_len = candidate;
                break;
            }
        }
        if tail_len > 0 {
            push_clamped(&mut out, color, tail_len, expected_len);
        }
    }

    if out.len() < expected_len {
        out.resize(expected_len, 0x62); // Pad missing tail with transparent
    }

    Ok(out)
}

#[inline]
fn push_clamped(out: &mut Vec<u8>, color: u8, count: usize, max_len: usize) {
    let remaining = max_len.saturating_sub(out.len());
    let n = count.min(remaining);
    if n > 0 {
        out.resize(out.len() + n, color);
    }
}

/// Maps a Supernote X / X2 pixel byte code to RGBA.
#[inline]
pub fn color_code_to_rgba(code: u8) -> Rgba<u8> {
    match code {
        0x61 | 0x66 => Rgba([0, 0, 0, 255]),          // Black / Marker black
        0x62 => Rgba([0, 0, 0, 0]),                   // Transparent background
        0x65 => Rgba([255, 255, 255, 255]),           // Opaque white
        0x63 | 0x67 | 0x9d | 0x9e => Rgba([0x9d, 0x9d, 0x9d, 255]), // Dark gray (X & X2)
        0x64 | 0x68 | 0xc9 | 0xca => Rgba([0xc9, 0xc9, 0xc9, 255]), // Gray (X & X2)
        // X2 anti-aliased stroke grayscale intensity
        v => Rgba([v, v, v, 255]),
    }
}

/// Renders a single page from an open reader into an `RgbaImage`.
///
/// - `layer_mask`: optional per-layer visibility override (aligned with `page.layers`).
///   When `None`, uses each layer's stored `visible` flag.
/// - `transparent_bg`: when `true` (used for `.mark` overlays onto PDFs), starts with a
///   transparent canvas and skips `BGLAYER`.
pub fn render_page_rgba<R: Read + Seek>(
    reader: &mut R,
    page: &Page,
    layer_mask: Option<&[bool]>,
    transparent_bg: bool,
) -> Result<RgbaImage> {
    let bg_pixel = if transparent_bg {
        Rgba([0, 0, 0, 0])
    } else {
        Rgba([255, 255, 255, 255])
    };
    let mut canvas = RgbaImage::from_pixel(page.width, page.height, bg_pixel);

    for (idx, layer) in page.layers.iter().enumerate() {
        if transparent_bg && layer.key == "BGLAYER" {
            continue;
        }
        let is_visible = layer_mask
            .and_then(|m| m.get(idx).copied())
            .unwrap_or(layer.visible);
        if !is_visible || layer.bitmap_address == 0 {
            continue;
        }

        let block = read_block(reader, layer.bitmap_address, MAX_BITMAP_BLOCK_BYTES)?;
        if block.is_empty() {
            continue;
        }

        match layer.protocol.as_str() {
            "RATTA_RLE" => {
                let all_blank = layer.key == "BGLAYER"
                    && (page.style == "style_white" || page.style.is_empty());
                let decoded = decode_ratta_rle(&block, page.width, page.height, all_blank)?;
                // Direct composite without allocating an extra RGBA buffer per layer!
                let canvas_buf = canvas.as_mut();
                for (px_idx, &code) in decoded.iter().enumerate() {
                    if code == 0x62 {
                        continue; // Transparent: skip
                    }
                    let rgba = color_code_to_rgba(code);
                    let off = px_idx * 4;
                    canvas_buf[off] = rgba[0];
                    canvas_buf[off + 1] = rgba[1];
                    canvas_buf[off + 2] = rgba[2];
                    canvas_buf[off + 3] = rgba[3];
                }
            }
            "PNG" => {
                let mut png_img = image::load_from_memory(&block)
                    .with_context(|| format!("failed to decode PNG layer {}", layer.key))?
                    .to_rgba8();
                if png_img.width() != page.width || png_img.height() != page.height {
                    png_img = imageops::resize(
                        &png_img,
                        page.width,
                        page.height,
                        imageops::FilterType::Triangle,
                    );
                }
                imageops::overlay(&mut canvas, &png_img, 0, 0);
            }
            _ => {
                // Unknown/unsupported layer protocol (e.g., empty placeholder): skip gracefully.
            }
        }
    }

    Ok(canvas)
}

/// Renders a single page directly into an 8-bit grayscale image (1/4 the RAM of RGBA).
/// Ideal for `.note` raster PDFs and compact disk caching.
pub fn render_page_gray<R: Read + Seek>(
    reader: &mut R,
    page: &Page,
    layer_mask: Option<&[bool]>,
) -> Result<GrayImage> {
    let rgba = render_page_rgba(reader, page, layer_mask, false)?;
    let (w, h) = rgba.dimensions();
    let mut gray = GrayImage::new(w, h);
    let src = rgba.as_raw();
    let dst = gray.as_mut();
    for (i, chunk) in src.as_chunks::<4>().0.iter().enumerate() {
        // Fast luminance approximation (exact for grayscale e-ink strokes)
        let l = if chunk[0] == chunk[1] && chunk[1] == chunk[2] {
            chunk[0]
        } else {
            ((chunk[0] as u32 * 77 + chunk[1] as u32 * 150 + chunk[2] as u32 * 29) >> 8) as u8
        };
        dst[i] = l;
    }
    Ok(gray)
}

/// Convenience helper to render a page index (0-based) directly from a file path.
pub fn render_file_page_rgba(
    path: &Path,
    notebook: &Notebook,
    page_index: usize,
    layer_mask: Option<&[bool]>,
    transparent_bg: bool,
) -> Result<RgbaImage> {
    let page = notebook
        .pages
        .get(page_index)
        .with_context(|| format!("page index {page_index} out of range ({} pages)", notebook.pages.len()))?;
    let mut file = File::open(path)
        .with_context(|| format!("failed to open {} for rendering", path.display()))?;
    render_page_rgba(&mut file, page, layer_mask, transparent_bg)
}

/// Convenience helper for generating a crisp white fallback `Luma` pixel check.
#[allow(dead_code)]
pub fn is_page_blank(img: &GrayImage) -> bool {
    img.pixels().all(|Luma([v])| *v == 255)
}

/// Validates that a file has at least one renderable page.
pub fn ensure_non_empty(notebook: &Notebook) -> Result<()> {
    if notebook.pages.is_empty() {
        bail!("notebook contains 0 pages");
    }
    Ok(())
}
