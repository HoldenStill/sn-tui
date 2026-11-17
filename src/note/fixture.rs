//! Deterministic synthetic Supernote `.note`, `.mark`, and PDF fixture builders for testing.

use std::io::Cursor;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::{ImageFormat, Rgba, RgbaImage};

pub const DEFAULT_SIGNATURE: &str = "SN_FILE_VER_20230015";

/// Protocol used by a synthetic layer.
#[derive(Debug, Clone)]
pub enum FixtureLayerData {
    /// Raw Supernote color codes (`0x61` black, `0x62` transparent, `0x65` white, etc.)
    /// encoded using `RATTA_RLE`.
    RattaRle(Vec<u8>),
    /// Embedded PNG bytes (`PNG` protocol).
    Png(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct FixtureLayer {
    pub name: String,
    pub data: FixtureLayerData,
    pub visible: bool,
}

#[derive(Debug, Clone)]
pub struct FixturePage {
    /// 1-based page number (`PAGE1`, `PAGE2`, or sparse e.g. `PAGE3` in `.mark` files).
    pub page_number: u32,
    pub landscape: bool,
    pub layers: Vec<FixtureLayer>,
    /// Optional recognized text lines `(y_position, label)`.
    pub recognition_lines: Option<Vec<(i32, String)>>,
}

#[derive(Debug, Clone)]
pub struct FixtureNotebook {
    pub is_mark: bool,
    pub equipment: String,
    pub signature: String,
    pub pages: Vec<FixturePage>,
}

/// Encodes Supernote color code pixels into `RATTA_RLE` byte pairs.
pub fn encode_ratta_rle(pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pixels.len() {
        let color = pixels[i];
        let mut run = 1usize;
        while i + run < pixels.len() && pixels[i + run] == color {
            run += 1;
        }
        i += run;

        let mut remaining = run;
        while remaining >= 0x4000 {
            out.push(color);
            out.push(0xff);
            remaining -= 0x4000;
        }
        while remaining > 0 {
            if remaining <= 128 {
                out.push(color);
                out.push((remaining - 1) as u8);
                remaining = 0;
            } else {
                // Multi-byte length: high chunk encodes ((prev & 0x7f) + 1) << 7 (128..=16384),
                // combined with second pair: 1 + low + high_chunk.
                // Maximum combined run is 16384 + 128 = 16512, but remaining < 16384 here.
                let high_units = ((remaining - 1) >> 7).min(127);
                let high_val = high_units << 7; // 128..=16256
                let low = remaining - high_val - 1;
                debug_assert!(high_units >= 1 && low <= 127);
                out.push(color);
                out.push(0x80 | ((high_units - 1) as u8));
                out.push(color);
                out.push(low as u8);
                remaining = 0;
            }
        }
    }
    out
}

fn push_block(buf: &mut Vec<u8>, payload: &[u8]) -> u32 {
    let offset = buf.len() as u32;
    let len = payload.len() as u32;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(payload);
    offset
}

/// Serializes a `FixtureNotebook` into authentic Supernote X/X2 binary format.
pub fn build_supernote_bytes(spec: &FixtureNotebook) -> Vec<u8> {
    let mut buf = Vec::new();
    let magic = if spec.is_mark { b"mark" } else { b"note" };
    buf.extend_from_slice(magic);

    let sig = if spec.signature.len() == 20 {
        spec.signature.as_bytes().to_vec()
    } else {
        DEFAULT_SIGNATURE.as_bytes().to_vec()
    };
    buf.extend_from_slice(&sig);

    let has_recogn = spec.pages.iter().any(|p| p.recognition_lines.is_some());
    let file_type = if spec.is_mark { "MARK" } else { "NOTE" };
    let recogn_type = if has_recogn { "1" } else { "0" };
    let header_meta = format!(
        "<MODULE_LABEL:SNFILE_FEATURE><FILE_TYPE:{file_type}><APPLY_EQUIPMENT:{eq}>\
         <FINALOPERATION_PAGE:1><FINALOPERATION_LAYER:1><ORIGINAL_STYLE:style_white>\
         <FILE_RECOGN_TYPE:{recogn_type}><FILE_ID:F20261117000001>",
        eq = spec.equipment,
    );
    let header_addr = push_block(&mut buf, header_meta.as_bytes());

    let mut page_entries = Vec::with_capacity(spec.pages.len());

    for page in &spec.pages {
        let mut layer_entries = Vec::with_capacity(page.layers.len());
        let mut layer_seq = Vec::with_capacity(page.layers.len());

        for layer in &page.layers {
            let (protocol, bitmap_bytes) = match &layer.data {
                FixtureLayerData::RattaRle(pixels) => ("RATTA_RLE", encode_ratta_rle(pixels)),
                FixtureLayerData::Png(png) => ("PNG", png.clone()),
            };
            let bitmap_addr = push_block(&mut buf, &bitmap_bytes);
            let visible_str = if layer.visible { "Y" } else { "N" };
            let layer_meta = format!(
                "<LAYERTYPE:NOTE><LAYERPROTOCOL:{protocol}><LAYERNAME:{name}>\
                 <LAYERPATH:0><LAYERBITMAP:{bitmap_addr}><LAYERVECTORGRAPH:0>\
                 <LAYERRECOGN:0><LAYERVISIBLE:{visible_str}>",
                name = layer.name,
            );
            let layer_addr = push_block(&mut buf, layer_meta.as_bytes());
            layer_entries.push((layer.name.clone(), layer_addr));
            layer_seq.push(layer.name.clone());
        }

        let (recogn_status, recogn_addr) = if let Some(lines) = &page.recognition_lines {
            let elements: Vec<serde_json::Value> = lines
                .iter()
                .map(|(y, label)| {
                    serde_json::json!({
                        "type": "Text",
                        "label": label,
                        "words": [
                            { "label": label, "bounding-box": { "x": 10, "y": y, "width": 100, "height": 20 } }
                        ]
                    })
                })
                .collect();
            let json = serde_json::json!({ "type": "Text", "elements": elements });
            let b64 = BASE64.encode(json.to_string().as_bytes());
            let addr = push_block(&mut buf, b64.as_bytes());
            ("1", addr)
        } else {
            ("0", 0)
        };

        let orientation = if page.landscape { "1090" } else { "1000" };
        let mut page_meta = format!(
            "<PAGESTYLE:style_white><ORIENTATION:{orientation}>\
             <LAYERSEQ:{seq}><RECOGNSTATUS:{recogn_status}><RECOGNTEXT:{recogn_addr}>",
            seq = layer_seq.join(","),
        );
        for (name, addr) in layer_entries {
            page_meta.push_str(&format!("<{name}:{addr}>"));
        }

        let page_addr = push_block(&mut buf, page_meta.as_bytes());
        page_entries.push((page.page_number, page_addr));
    }

    let mut footer_meta = format!("<FILE_FEATURE:{header_addr}>");
    for (num, addr) in page_entries {
        footer_meta.push_str(&format!("<PAGE{num}:{addr}>"));
    }
    let footer_addr = push_block(&mut buf, footer_meta.as_bytes());
    buf.extend_from_slice(&footer_addr.to_le_bytes());
    buf
}

/// Generates a minimal solid-color PNG image buffer of the given dimensions.
pub fn make_solid_png(width: u32, height: u32, pixel: Rgba<u8>) -> Vec<u8> {
    let img = RgbaImage::from_pixel(width, height, pixel);
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .expect("encode solid png");
    out.into_inner()
}

/// Generates a minimal valid multi-page PDF 1.4 document with selectable Helvetica text
/// on each page (for `.pdf.mark` viewing & lossless overlay export tests).
pub fn build_sample_pdf(page_texts: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");

    let mut offsets: Vec<usize> = Vec::new();
    let num_pages = page_texts.len().max(1);

    // 1: Catalog, 2: Pages, 3: Helvetica Font, then per page: (Page, Contents)
    let font_obj = 3usize;
    let first_page_obj = 4usize;

    // Object 1: Catalog
    offsets.push(out.len());
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    // Object 2: Pages
    offsets.push(out.len());
    let kids: Vec<String> = (0..num_pages)
        .map(|i| format!("{} 0 R", first_page_obj + i * 2))
        .collect();
    out.extend_from_slice(
        format!(
            "2 0 obj\n<< /Type /Pages /Kids [ {} ] /Count {} >>\nendobj\n",
            kids.join(" "),
            num_pages
        )
        .as_bytes(),
    );

    // Object 3: Standard Type1 Font
    offsets.push(out.len());
    out.extend_from_slice(
        b"3 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
    );

    for (i, text) in page_texts.iter().enumerate() {
        let page_id = first_page_obj + i * 2;
        let content_id = page_id + 1;
        let escaped = text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let stream = format!("BT /F1 24 Tf 72 720 Td ({escaped}) Tj ET\n");

        offsets.push(out.len());
        out.extend_from_slice(
            format!(
                "{page_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
                 /Resources << /Font << /F1 {font_obj} 0 R >> >> /Contents {content_id} 0 R >>\nendobj\n"
            )
            .as_bytes(),
        );

        offsets.push(out.len());
        out.extend_from_slice(
            format!(
                "{content_id} 0 obj\n<< /Length {} >>\nstream\n{stream}endstream\nendobj\n",
                stream.len()
            )
            .as_bytes(),
        );
    }

    let xref_offset = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", offsets.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_note_header_and_footer_pointer() {
        let width = 1404usize;
        let height = 1872usize;
        let mut pixels = vec![0x62u8; width * height];
        for x in 100..300 {
            pixels[200 * width + x] = 0x61; // black stroke
        }
        let bytes = build_supernote_bytes(&FixtureNotebook {
            is_mark: false,
            equipment: "A5X".to_string(),
            signature: DEFAULT_SIGNATURE.to_string(),
            pages: vec![FixturePage {
                page_number: 1,
                landscape: false,
                layers: vec![FixtureLayer {
                    name: "MAINLAYER".to_string(),
                    data: FixtureLayerData::RattaRle(pixels),
                    visible: true,
                }],
                recognition_lines: Some(vec![(100, "Hello Supernote".to_string())]),
            }],
        });

        assert_eq!(&bytes[0..4], b"note");
        assert_eq!(&bytes[4..24], DEFAULT_SIGNATURE.as_bytes());
        let footer_off = u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap()) as usize;
        assert!(footer_off > 24 && footer_off < bytes.len() - 4);
    }

    #[test]
    fn generates_minimal_pdf() {
        let pdf = build_sample_pdf(&["Page One Text", "Page Two Text"]);
        assert!(pdf.starts_with(b"%PDF-1.4\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
    }
}
