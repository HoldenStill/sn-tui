//! Bounded disk LRU page cache (`~/.cache/sn-tui/pages/`), lazy OCR index (`ocr_index.json`),
//! and small in-memory `LruCache` (6 pages) for sub-5ms page turns.

use std::collections::HashMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use image::{DynamicImage, ImageFormat};
use lru::LruCache;
use serde::{Deserialize, Serialize};

use crate::note::extract_notebook_text;

const DEFAULT_MEM_CACHE_PAGES: usize = 6;
const CACHE_VERSION: &str = "v3";

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PageCacheKey {
    pub path: PathBuf,
    pub mtime_nanos: u128,
    pub file_size: u64,
    pub page_number: u32,
    pub layer_bits: u64,
    pub show_mark: bool,
}

impl PageCacheKey {
    pub fn from_file(
        path: &Path,
        page_number: u32,
        layer_bits: u64,
        show_mark: bool,
    ) -> Self {
        let (mtime_nanos, file_size) = file_fingerprint(path);
        // Also incorporate companion .mark fingerprint if present so mark edits bust cache!
        let mut combined_mtime = mtime_nanos;
        let mut combined_size = file_size;
        if show_mark && let Some(mark_path) = crate::pdf::find_companion_mark(path) {
            let (m_mt, m_sz) = file_fingerprint(&mark_path);
            combined_mtime ^= m_mt.rotate_left(13);
            combined_size ^= m_sz.rotate_left(7);
        }
        Self {
            path: path.to_path_buf(),
            mtime_nanos: combined_mtime,
            file_size: combined_size,
            page_number,
            layer_bits,
            show_mark,
        }
    }

    pub fn disk_filename(&self) -> String {
        let hash = fnv1a_64(&format!(
            "{CACHE_VERSION}|{}|{}|{}|{}|{}",
            self.path.to_string_lossy(),
            self.mtime_nanos,
            self.file_size,
            self.layer_bits,
            self.show_mark as u8
        ));
        format!("{hash:016x}_p{}.png", self.page_number)
    }

    pub fn path_prefix_hash(&self) -> String {
        let hash = fnv1a_64(&self.path.to_string_lossy());
        format!("{hash:08x}")
    }
}

pub struct PageCache {
    pages_dir: PathBuf,
    max_disk_bytes: u64,
    mem: LruCache<PageCacheKey, DynamicImage>,
}

impl PageCache {
    pub fn new(cache_root: &Path, max_disk_mb: u64) -> Result<Self> {
        let pages_dir = cache_root.join("pages");
        fs::create_dir_all(&pages_dir)?;
        let cap = NonZeroUsize::new(DEFAULT_MEM_CACHE_PAGES).expect("non-zero");
        Ok(Self {
            pages_dir,
            max_disk_bytes: max_disk_mb.max(1) * 1024 * 1024,
            mem: LruCache::new(cap),
        })
    }

    /// Creates a page cache with an explicit byte limit (handy for tests).
    pub fn with_max_bytes(cache_root: &Path, max_disk_bytes: u64) -> Result<Self> {
        let pages_dir = cache_root.join("pages");
        fs::create_dir_all(&pages_dir)?;
        let cap = NonZeroUsize::new(DEFAULT_MEM_CACHE_PAGES).expect("non-zero");
        Ok(Self {
            pages_dir,
            max_disk_bytes: max_disk_bytes.max(1024),
            mem: LruCache::new(cap),
        })
    }

    pub fn get(&mut self, key: &PageCacheKey) -> Option<DynamicImage> {
        if let Some(img) = self.mem.get(key) {
            return Some(img.clone());
        }
        let disk_path = self.pages_dir.join(key.disk_filename());
        if disk_path.is_file()
            && let Ok(img) = image::open(&disk_path)
        {
            self.mem.put(key.clone(), img.clone());
            return Some(img);
        }
        None
    }

    pub fn put(&mut self, key: PageCacheKey, image: DynamicImage) {
        let disk_path = self.pages_dir.join(key.disk_filename());
        let tmp_path = self.pages_dir.join(format!("{}.tmp", key.disk_filename()));
        if image.save_with_format(&tmp_path, ImageFormat::Png).is_ok() {
            let _ = fs::rename(&tmp_path, &disk_path);
            let _ = self.enforce_disk_budget();
        } else {
            let _ = fs::remove_file(&tmp_path);
        }
        self.mem.put(key, image);
    }

    pub fn invalidate_path(&mut self, target: &Path) {
        let keys_to_remove: Vec<PageCacheKey> = self
            .mem
            .iter()
            .filter(|(k, _)| k.path == target)
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys_to_remove {
            let disk = self.pages_dir.join(k.disk_filename());
            let _ = fs::remove_file(disk);
            self.mem.pop(&k);
        }
    }

    pub fn disk_usage_bytes(&self) -> u64 {
        let Ok(entries) = fs::read_dir(&self.pages_dir) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum()
    }

    /// Evicts oldest cached PNGs by mtime until total disk cache usage is `<= max_disk_bytes`.
    pub fn enforce_disk_budget(&self) -> Result<usize> {
        let Ok(entries) = fs::read_dir(&self.pages_dir) else {
            return Ok(0);
        };
        let mut files: Vec<(PathBuf, u64, u128)> = Vec::new();
        let mut total = 0u64;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("png") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let len = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                total += len;
                files.push((path, len, mtime));
            }
        }
        if total <= self.max_disk_bytes {
            return Ok(0);
        }

        // Oldest first
        files.sort_by_key(|(_, _, mtime)| *mtime);
        let mut evicted = 0usize;
        for (path, len, _) in files {
            if total <= self.max_disk_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                evicted += 1;
            }
        }
        Ok(evicted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrEntry {
    pub mtime_nanos: u128,
    pub file_size: u64,
    pub text: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OcrIndex {
    #[serde(skip)]
    index_path: PathBuf,
    pub entries: HashMap<String, OcrEntry>,
}

impl OcrIndex {
    pub fn load(cache_root: &Path) -> Self {
        let index_path = cache_root.join("ocr_index.json");
        if let Ok(raw) = fs::read_to_string(&index_path)
            && let Ok(mut idx) = serde_json::from_str::<OcrIndex>(&raw)
        {
            idx.index_path = index_path;
            return idx;
        }
        Self {
            index_path,
            entries: HashMap::new(),
        }
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.index_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(&self)?;
        let tmp = self.index_path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.index_path)?;
        Ok(())
    }

    /// Lazily indexes a `.note` file if missing or modified on disk, returning its cached text.
    pub fn get_or_index(&mut self, path: &Path) -> Option<&str> {
        let key = path.to_string_lossy().to_string();
        let (mtime_nanos, file_size) = file_fingerprint(path);

        let needs_update = match self.entries.get(&key) {
            Some(existing) => existing.mtime_nanos != mtime_nanos || existing.file_size != file_size,
            None => true,
        };

        if needs_update {
            let text = match extract_notebook_text(path) {
                Ok(pages) => pages
                    .into_iter()
                    .map(|(_, t)| t)
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(_) => String::new(),
            };
            self.entries.insert(
                key.clone(),
                OcrEntry {
                    mtime_nanos,
                    file_size,
                    text,
                },
            );
        }

        self.entries
            .get(&key)
            .map(|e| e.text.as_str())
            .filter(|s| !s.is_empty())
    }

    pub fn cached_text(&self, path: &Path) -> Option<&str> {
        let key = path.to_string_lossy();
        self.entries
            .get(key.as_ref())
            .map(|e| e.text.as_str())
            .filter(|s| !s.is_empty())
    }

    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path.to_string_lossy().as_ref());
    }
}

pub fn file_fingerprint(path: &Path) -> (u128, u64) {
    match fs::metadata(path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            (mtime, meta.len())
        }
        Err(_) => (0, 0),
    }
}

fn fnv1a_64(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in input.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
