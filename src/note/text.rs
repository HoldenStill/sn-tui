//! Pure-Rust Supernote `RECOGNTEXT` (real-time handwriting recognition) extractor.

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::Deserialize;

use crate::note::parser::{Page, parse_notebook, read_block};

const MAX_RECOGN_BLOCK_BYTES: usize = 4 * 1024 * 1024;
const RECOGNSTATUS_DONE: u8 = 1;

#[derive(Debug, Deserialize)]
struct RecognPayload {
    #[serde(default)]
    elements: Vec<RecognElement>,
}

#[derive(Debug, Deserialize)]
struct RecognElement {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    words: Vec<RecognWord>,
}

#[derive(Debug, Deserialize)]
struct RecognWord {
    #[serde(rename = "bounding-box")]
    bounding_box: Option<BoundingBox>,
}

#[derive(Debug, Deserialize)]
struct BoundingBox {
    #[serde(default)]
    y: f64,
}

/// Extracts recognized text from a single page, sorted top-to-bottom by y-coordinate.
pub fn extract_page_text<R: Read + Seek>(reader: &mut R, page: &Page) -> Result<Option<String>> {
    if page.recogn_status != RECOGNSTATUS_DONE || page.recogn_text_addr == 0 {
        return Ok(None);
    }

    let raw = read_block(reader, page.recogn_text_addr, MAX_RECOGN_BLOCK_BYTES)?;
    if raw.is_empty() {
        return Ok(None);
    }

    let b64_clean: Vec<u8> = raw
        .into_iter()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let decoded = match BASE64.decode(&b64_clean) {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };
    let payload: RecognPayload = match serde_json::from_slice(&decoded) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };

    let mut lines: Vec<(f64, String)> = Vec::new();
    for elem in payload.elements {
        if elem.kind != "Text" {
            continue;
        }
        let label = elem.label.trim().to_string();
        if label.is_empty() {
            continue;
        }
        let y = elem
            .words
            .iter()
            .find_map(|w| w.bounding_box.as_ref().map(|b| b.y))
            .unwrap_or(0.0);
        lines.push((y, label));
    }

    if lines.is_empty() {
        return Ok(None);
    }

    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    let joined = lines
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Some(joined))
}

/// Extracts all recognized text across pages of a `.note` file as `(page_number, text)` pairs.
pub fn extract_notebook_text(path: &Path) -> Result<Vec<(u32, String)>> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {} for text extraction", path.display()))?;
    let notebook = parse_notebook(&mut file)?;
    let mut results = Vec::new();
    for page in &notebook.pages {
        if let Some(text) = extract_page_text(&mut file, page)? {
            results.push((page.page_number, text));
        }
    }
    Ok(results)
}
