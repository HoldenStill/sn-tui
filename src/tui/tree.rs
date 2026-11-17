//! Workspace directory tree scanner with companion `.pdf.mark` grouping, status badges,
//! and filename + OCR fuzzy search filtering.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cache::OcrIndex;
use crate::pdf::find_companion_mark;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    Note,
    Pdf,
    StandaloneMark,
}

#[derive(Debug, Clone)]
pub struct TreeRow {
    pub path: PathBuf,
    pub rel_path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub kind: EntryKind,
    pub expanded: bool,
    pub has_mark: bool,
    pub is_exported: bool,
    pub is_synced: bool,
    pub page_count: Option<usize>,
}

pub struct WorkspaceTree {
    pub root: PathBuf,
    pub export_dir: PathBuf,
    pub expanded_dirs: HashSet<PathBuf>,
    pub synced_paths: HashSet<PathBuf>,
    pub page_counts: HashMap<PathBuf, usize>,
    pub rows: Vec<TreeRow>,
    pub filter: String,
}

impl WorkspaceTree {
    pub fn new(root: PathBuf, export_dir: PathBuf) -> Self {
        let mut expanded_dirs = HashSet::new();
        expanded_dirs.insert(root.clone());
        Self {
            root,
            export_dir,
            expanded_dirs,
            synced_paths: HashSet::new(),
            page_counts: HashMap::new(),
            rows: Vec::new(),
            filter: String::new(),
        }
    }

    pub fn set_page_count(&mut self, path: &Path, count: usize) {
        self.page_counts.insert(path.to_path_buf(), count);
        for row in &mut self.rows {
            if row.path == path {
                row.page_count = Some(count);
            }
        }
    }

    pub fn all_note_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        collect_notes_recursive(&self.root, &mut out);
        out
    }

    pub fn rebuild(&mut self, ocr_index: &OcrIndex) {
        self.rows.clear();
        if !self.root.exists() {
            return;
        }
        let query = self.filter.trim().to_ascii_lowercase();
        if query.is_empty() {
            let root = self.root.clone();
            self.walk_dir(&root, 0);
        } else {
            let root = self.root.clone();
            self.walk_filtered(&root, &query, ocr_index);
        }
    }

    fn walk_dir(&mut self, dir: &Path, depth: usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                dirs.push((name, path));
            } else if path.is_file() {
                files.push((name, path));
            }
        }

        dirs.sort_by_key(|a| a.0.to_ascii_lowercase());
        files.sort_by_key(|a| a.0.to_ascii_lowercase());

        for (name, path) in dirs {
            let expanded = self.expanded_dirs.contains(&path);
            let rel_path = path.strip_prefix(&self.root).unwrap_or(&path).to_path_buf();
            self.rows.push(TreeRow {
                path: path.clone(),
                rel_path,
                name,
                depth,
                kind: EntryKind::Directory,
                expanded,
                has_mark: false,
                is_exported: false,
                is_synced: false,
                page_count: None,
            });
            if expanded {
                self.walk_dir(&path, depth + 1);
            }
        }

        for (name, path) in files {
            if let Some(row) = self.classify_file(&name, &path, depth) {
                self.rows.push(row);
            }
        }
    }

    fn walk_filtered(&mut self, dir: &Path, query: &str, ocr_index: &OcrIndex) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<_> = entries.filter_map(Result::ok).collect();
        items.sort_by_key(|e| e.file_name().to_string_lossy().to_ascii_lowercase());

        for entry in items {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                self.walk_filtered(&path, query, ocr_index);
            } else if let Some(row) = self.classify_file(&name, &path, 0) {
                let rel_match = fuzzy_contains(&row.rel_path.to_string_lossy(), query);
                let ocr_match = ocr_index
                    .cached_text(&path)
                    .is_some_and(|txt| fuzzy_contains(txt, query));
                if rel_match || ocr_match {
                    self.rows.push(row);
                }
            }
        }
    }

    fn classify_file(&self, name: &str, path: &Path, depth: usize) -> Option<TreeRow> {
        let lower = name.to_ascii_lowercase();
        let (kind, has_mark) = if lower.ends_with(".note") {
            (EntryKind::Note, false)
        } else if lower.ends_with(".pdf") {
            (EntryKind::Pdf, find_companion_mark(path).is_some())
        } else if lower.ends_with(".pdf.mark") {
            // Hide sidecar if companion PDF exists (grouped onto the PDF row).
            let companion_pdf = path.with_extension("");
            if companion_pdf.is_file() {
                return None;
            }
            (EntryKind::StandaloneMark, true)
        } else if lower.ends_with(".mark") {
            (EntryKind::StandaloneMark, true)
        } else {
            return None;
        };

        let rel_path = path.strip_prefix(&self.root).unwrap_or(path).to_path_buf();
        let is_exported = self.check_exported(&rel_path);
        let is_synced = self.synced_paths.contains(path);
        let page_count = self.page_counts.get(path).copied();

        Some(TreeRow {
            path: path.to_path_buf(),
            rel_path,
            name: name.to_string(),
            depth,
            kind,
            expanded: false,
            has_mark,
            is_exported,
            is_synced,
            page_count,
        })
    }

    fn check_exported(&self, rel_path: &Path) -> bool {
        for ext in ["pdf", "png", "txt", "md", "svg"] {
            if self.export_dir.join(rel_path).with_extension(ext).is_file() {
                return true;
            }
        }
        false
    }
}

fn collect_notes_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_notes_recursive(&path, out);
        } else if name.to_ascii_lowercase().ends_with(".note") {
            out.push(path);
        }
    }
}

/// Case-insensitive multi-token substring match (all whitespace-separated query tokens must match).
pub fn fuzzy_contains(haystack: &str, query_lower: &str) -> bool {
    let h = haystack.to_ascii_lowercase();
    query_lower
        .split_whitespace()
        .all(|tok| h.contains(tok))
}
