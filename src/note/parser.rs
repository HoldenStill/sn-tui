//! Pure-Rust Supernote `.note` and `.mark` binary metadata parser (zero regex).

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;

pub const A5X_WIDTH: u32 = 1404;
pub const A5X_HEIGHT: u32 = 1872;
pub const A5X2_WIDTH: u32 = 1920;
pub const A5X2_HEIGHT: u32 = 2560;

/// Maximum metadata block size we will allocate (guard against corrupt length prefixes).
const MAX_METADATA_BLOCK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FileKind {
    Note,
    Mark,
}

#[derive(Debug, Clone, Serialize)]
pub struct Layer {
    pub key: String,
    pub name: String,
    pub protocol: String,
    pub bitmap_address: u64,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    /// 1-based page number from footer key `PAGE<N>` (can be sparse in `.mark` files).
    pub page_number: u32,
    pub addr: u64,
    pub width: u32,
    pub height: u32,
    pub landscape: bool,
    pub style: String,
    pub recogn_status: u8,
    pub recogn_text_addr: u64,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotebookMetadata {
    pub file_kind: FileKind,
    pub signature: String,
    pub equipment: String,
    pub device_label: String,
    pub file_id: String,
    pub is_realtime_recognition: bool,
    pub base_width: u32,
    pub base_height: u32,
    pub page_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Notebook {
    pub metadata: NotebookMetadata,
    pub pages: Vec<Page>,
}

impl Notebook {
    /// Look up a page by its 1-based Supernote page number (useful for sparse `.mark` files).
    pub fn find_page_by_number(&self, page_number: u32) -> Option<(usize, &Page)> {
        self.pages
            .iter()
            .enumerate()
            .find(|(_, p)| p.page_number == page_number)
    }
}

/// Linear zero-regex parser for `<KEY:VALUE>` Supernote metadata blocks.
pub fn parse_kv_block(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let start = i + 1;
            if let Some(rel_end) = content[start..].find('>') {
                let end = start + rel_end;
                let pair = &content[start..end];
                if let Some(colon) = pair.find(':') {
                    let key = &pair[..colon];
                    let val = &pair[colon + 1..];
                    if !key.is_empty() {
                        map.insert(key.to_string(), val.to_string());
                    }
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    map
}

/// Reads a length-prefixed binary block `[u32_le len][bytes]` at `address`.
pub fn read_block<R: Read + Seek>(reader: &mut R, address: u64, max_bytes: usize) -> Result<Vec<u8>> {
    if address == 0 {
        return Ok(Vec::new());
    }
    reader
        .seek(SeekFrom::Start(address))
        .with_context(|| format!("failed to seek to block at offset {address}"))?;
    let mut len_bytes = [0u8; 4];
    reader
        .read_exact(&mut len_bytes)
        .with_context(|| format!("failed to read block length at offset {address}"))?;
    let block_len = u32::from_le_bytes(len_bytes) as usize;
    if block_len > max_bytes {
        bail!("block length {block_len} at offset {address} exceeds safety cap {max_bytes}");
    }
    let mut buf = vec![0u8; block_len];
    reader
        .read_exact(&mut buf)
        .with_context(|| format!("failed to read {block_len} bytes at offset {address}"))?;
    Ok(buf)
}

fn read_metadata_block<R: Read + Seek>(reader: &mut R, address: u64) -> Result<HashMap<String, String>> {
    if address == 0 {
        return Ok(HashMap::new());
    }
    let bytes = read_block(reader, address, MAX_METADATA_BLOCK_BYTES)?;
    let content = String::from_utf8_lossy(&bytes);
    Ok(parse_kv_block(&content))
}

fn equipment_info(equipment: &str) -> (&'static str, u32, u32) {
    match equipment {
        "N5" => ("A5 X2 (Manta)", A5X2_WIDTH, A5X2_HEIGHT),
        "N6" | "A6X2" => ("A6 X2 (Nomad)", A5X_WIDTH, A5X_HEIGHT),
        "A5X" => ("A5 X", A5X_WIDTH, A5X_HEIGHT),
        "A6X" => ("A6 X", A5X_WIDTH, A5X_HEIGHT),
        _ => ("Supernote X/X2", A5X_WIDTH, A5X_HEIGHT),
    }
}

pub fn parse_notebook_file(path: &Path) -> Result<Notebook> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open Supernote file {}", path.display()))?;
    parse_notebook(&mut file)
}

pub fn parse_notebook<R: Read + Seek>(reader: &mut R) -> Result<Notebook> {
    let file_len = reader.seek(SeekFrom::End(0))?;
    if file_len < 28 {
        bail!("file too small ({file_len} bytes) to be a valid Supernote .note/.mark file");
    }

    reader.seek(SeekFrom::Start(0))?;
    let mut magic_and_sig = [0u8; 24];
    reader.read_exact(&mut magic_and_sig)?;

    let magic = std::str::from_utf8(&magic_and_sig[0..4]).unwrap_or("");
    let signature = String::from_utf8_lossy(&magic_and_sig[4..24]).to_string();

    if !signature.starts_with("SN_FILE_VER_") && !signature.starts_with("SN_FILE_ASA_") {
        bail!("unrecognized Supernote signature: {signature:?}");
    }

    // Footer pointer lives in the last 4 bytes (u32 LE)
    reader.seek(SeekFrom::End(-4))?;
    let mut footer_addr_bytes = [0u8; 4];
    reader.read_exact(&mut footer_addr_bytes)?;
    let footer_addr = u32::from_le_bytes(footer_addr_bytes) as u64;
    if footer_addr == 0 || footer_addr >= file_len.saturating_sub(4) {
        bail!("invalid footer offset {footer_addr} for file length {file_len}");
    }

    let footer_map = read_metadata_block(reader, footer_addr)?;

    let header_map = if let Some(addr_str) = footer_map.get("FILE_FEATURE") {
        if let Ok(addr) = addr_str.parse::<u64>() {
            read_metadata_block(reader, addr)?
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };

    let file_kind = match header_map.get("FILE_TYPE").map(String::as_str) {
        Some("MARK") => FileKind::Mark,
        Some("NOTE") => FileKind::Note,
        _ if magic == "mark" => FileKind::Mark,
        _ => FileKind::Note,
    };

    let equipment = header_map
        .get("APPLY_EQUIPMENT")
        .cloned()
        .unwrap_or_else(|| "A5X".to_string());
    let (device_label, base_width, base_height) = equipment_info(&equipment);
    let file_id = header_map.get("FILE_ID").cloned().unwrap_or_default();
    let is_realtime_recognition = header_map
        .get("FILE_RECOGN_TYPE")
        .is_some_and(|v| v == "1");

    // Collect and numerically sort all PAGE<N> footer keys
    let mut page_entries: Vec<(u32, u64)> = footer_map
        .iter()
        .filter_map(|(k, v)| {
            let num_str = k.strip_prefix("PAGE")?;
            let page_num = num_str.parse::<u32>().ok()?;
            let addr = v.parse::<u64>().ok()?;
            Some((page_num, addr))
        })
        .collect();
    page_entries.sort_by_key(|(num, _)| *num);

    let mut pages = Vec::with_capacity(page_entries.len());
    for (page_number, addr) in page_entries {
        let page_map = read_metadata_block(reader, addr)?;
        let landscape = page_map
            .get("ORIENTATION")
            .is_some_and(|o| o == "1090");
        let (width, height) = if landscape {
            (base_height, base_width)
        } else {
            (base_width, base_height)
        };
        let style = page_map.get("PAGESTYLE").cloned().unwrap_or_default();
        let recogn_status = page_map
            .get("RECOGNSTATUS")
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(0);
        let recogn_text_addr = page_map
            .get("RECOGNTEXT")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let layer_order: Vec<String> = page_map
            .get("LAYERSEQ")
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                s.split(',')
                    .map(|part| part.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![
                    "BGLAYER".to_string(),
                    "MAINLAYER".to_string(),
                    "LAYER1".to_string(),
                    "LAYER2".to_string(),
                    "LAYER3".to_string(),
                ]
            });

        // Supernote stores LAYERSEQ in top-to-bottom or bottom-to-top depending on firmware;
        // BGLAYER should always be composited first, followed by MAINLAYER and LAYER1..3.
        let mut layers = Vec::new();
        for layer_key in &layer_order {
            if let Some(addr_str) = page_map.get(layer_key.as_str()) {
                let layer_addr = addr_str.parse::<u64>().unwrap_or(0);
                if layer_addr == 0 {
                    continue;
                }
                let layer_map = read_metadata_block(reader, layer_addr)?;
                let visible = layer_map
                    .get("LAYERVISIBLE")
                    .map(|v| v != "N" && v != "0" && v != "false")
                    .unwrap_or(true);
                layers.push(Layer {
                    key: layer_key.clone(),
                    name: layer_map
                        .get("LAYERNAME")
                        .cloned()
                        .unwrap_or_else(|| layer_key.clone()),
                    protocol: layer_map
                        .get("LAYERPROTOCOL")
                        .cloned()
                        .unwrap_or_default(),
                    bitmap_address: layer_map
                        .get("LAYERBITMAP")
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0),
                    visible,
                });
            }
        }

        // Ensure BGLAYER is composited first if present
        if let Some(bg_pos) = layers.iter().position(|l| l.key == "BGLAYER")
            && bg_pos != 0
        {
            let bg = layers.remove(bg_pos);
            layers.insert(0, bg);
        }

        pages.push(Page {
            page_number,
            addr,
            width,
            height,
            landscape,
            style,
            recogn_status,
            recogn_text_addr,
            layers,
        });
    }

    let metadata = NotebookMetadata {
        file_kind,
        signature,
        equipment,
        device_label: device_label.to_string(),
        file_id,
        is_realtime_recognition,
        base_width,
        base_height,
        page_count: pages.len(),
    };

    Ok(Notebook { metadata, pages })
}
