//! Interactive TUI application state, Vim keybindings, zoom/pan, layer toggles, and modals.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use image::DynamicImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::cache::OcrIndex;
use crate::config::AppConfig;
use crate::export::ExportFormat;
use crate::export::supernote_tool::SupernoteToolBackend;
use crate::note::{Notebook, parse_notebook_file};
use crate::pdf::{PdfInfo, inspect_pdf};
use crate::tui::tree::{EntryKind, TreeRow, WorkspaceTree};
use crate::tui::worker::{RenderWorker, WorkerEvent, WorkerRequest};

const MAX_LOG_LINES: usize = 60;

#[derive(Debug, Clone)]
pub enum ActiveDoc {
    None,
    Note(Notebook),
    Pdf(PdfInfo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewLayout {
    Standard3Pane,    // 26% Tree, 50% Canvas, 24% Inspector
    WideReader,       // 25% Tree, 75% Canvas (Inspector hidden)
    BottomInspector,  // 25% Tree left, Canvas 75% height top-right, Inspector 25% bottom-right
    FullFocus,        // 100% Canvas only (Tree & Inspector hidden)
}

impl ViewLayout {
    pub fn next(self) -> Self {
        match self {
            Self::Standard3Pane => Self::WideReader,
            Self::WideReader => Self::BottomInspector,
            Self::BottomInspector => Self::FullFocus,
            Self::FullFocus => Self::Standard3Pane,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Standard3Pane => "3-Pane",
            Self::WideReader => "Wide",
            Self::BottomInspector => "Bottom",
            Self::FullFocus => "Focus",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    SelectedFile,
    WorkspaceFolder,
}

#[derive(Debug, Clone)]
pub struct ExportModalState {
    pub scope: ExportScope,
    pub format_idx: usize,
    pub formats: Vec<ExportFormat>,
    pub dest_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct UploadModalState {
    pub local_file: PathBuf,
    pub remote_dest: String,
}

#[derive(Debug, Clone)]
pub enum Modal {
    None,
    Search,
    JumpPage(String),
    Export(ExportModalState),
    Upload(UploadModalState),
    Help,
}

pub enum SyncTrigger {
    PullSync { profile: Option<String> },
    UploadFile { local_file: PathBuf, remote_dest: String, profile: Option<String> },
}

pub struct App {
    pub config: AppConfig,
    pub tree: WorkspaceTree,
    pub ocr_index: OcrIndex,
    pub selected_idx: usize,
    pub active_path: Option<PathBuf>,
    pub active_doc: ActiveDoc,
    pub current_page: u32,
    pub total_pages: u32,
    pub layer_visibility: Vec<bool>,
    pub show_mark_overlay: bool,
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub raw_image: Option<DynamicImage>,
    pub image_protocol: Option<StatefulProtocol>,
    pub picker: Picker,
    pub active_ocr_text: Option<String>,
    pub render_seq: u64,
    pub loading_page: bool,
    pub modal: Modal,
    pub logs: VecDeque<String>,
    pub cache_bytes: u64,
    pub supernote_tool: SupernoteToolBackend,
    pub view_layout: ViewLayout,
    pub prev_view_layout: ViewLayout,
    pub invert_colors: bool,
    pub should_quit: bool,
    pub worker: RenderWorker,
    pub pending_sync_triggers: Vec<SyncTrigger>,
}

impl App {
    pub fn new(
        config: AppConfig,
        cache_root: PathBuf,
        picker: Picker,
        startup_warnings: Vec<String>,
    ) -> Self {
        let supernote_tool = SupernoteToolBackend::detect();
        let ocr_index = OcrIndex::load(&cache_root);
        let mut tree = WorkspaceTree::new(config.workspace_dir.clone(), config.export_dir.clone());
        tree.rebuild(&ocr_index);

        let worker = RenderWorker::spawn(cache_root, config.cache_max_mb);

        let mut logs = VecDeque::with_capacity(MAX_LOG_LINES);
        logs.push_back(format!(
            "Graphics: {:?} | supernote-tool: {}",
            picker.protocol_type(),
            supernote_tool.label()
        ));
        for w in startup_warnings {
            logs.push_back(format!("WARN: {w}"));
        }

        let note_paths = tree.all_note_paths();
        if !note_paths.is_empty() {
            let _ = worker.tx.send(WorkerRequest::IndexOcr { paths: note_paths });
        }

        let mut app = Self {
            config,
            tree,
            ocr_index,
            selected_idx: 0,
            active_path: None,
            active_doc: ActiveDoc::None,
            current_page: 1,
            total_pages: 0,
            layer_visibility: Vec::new(),
            show_mark_overlay: true,
            zoom: 1.0,
            pan_x: 0.5,
            pan_y: 0.5,
            raw_image: None,
            image_protocol: None,
            picker,
            active_ocr_text: None,
            render_seq: 0,
            loading_page: false,
            modal: Modal::None,
            logs,
            cache_bytes: 0,
            supernote_tool,
            view_layout: ViewLayout::Standard3Pane,
            prev_view_layout: ViewLayout::Standard3Pane,
            invert_colors: false,
            should_quit: false,
            worker,
            pending_sync_triggers: Vec::new(),
        };

        app.select_current_row();
        app
    }

    pub fn push_log(&mut self, msg: impl Into<String>) {
        if self.logs.len() >= MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(msg.into());
    }

    pub fn selected_row(&self) -> Option<&TreeRow> {
        self.tree.rows.get(self.selected_idx)
    }

    pub fn mode_label(&self) -> &'static str {
        match self.modal {
            Modal::None => "NORMAL",
            Modal::Search => "SEARCH",
            Modal::JumpPage(_) => "GOTO",
            Modal::Export(_) => "EXPORT",
            Modal::Upload(_) => "UPLOAD",
            Modal::Help => "HELP",
        }
    }

    pub fn drain_worker_events(&mut self) {
        while let Ok(evt) = self.worker.rx.try_recv() {
            match evt {
                WorkerEvent::PageRendered {
                    seq,
                    path,
                    page_number,
                    image,
                    ocr_text,
                    cache_bytes,
                } => {
                    self.cache_bytes = cache_bytes;
                    if seq == self.render_seq && self.active_path.as_ref() == Some(&path) {
                        self.current_page = page_number;
                        self.active_ocr_text = ocr_text;
                        self.raw_image = Some(image);
                        self.loading_page = false;
                        self.rebuild_image_protocol();
                    }
                }
                WorkerEvent::RenderError { seq, path, message } => {
                    if seq == self.render_seq {
                        self.loading_page = false;
                        self.raw_image = None;
                        self.image_protocol = None;
                        self.push_log(format!("Render error ({}): {message}", path.display()));
                    }
                }
                WorkerEvent::ExportFinished { message, ok } => {
                    self.push_log(if ok {
                        message
                    } else {
                        format!("ERR: {message}")
                    });
                    self.tree.rebuild(&self.ocr_index);
                }
                WorkerEvent::OcrIndexed { path, text } => {
                    let (mtime, size) = crate::cache::file_fingerprint(&path);
                    self.ocr_index.entries.insert(
                        path.to_string_lossy().to_string(),
                        crate::cache::OcrEntry {
                            mtime_nanos: mtime,
                            file_size: size,
                            text,
                        },
                    );
                }
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        match &mut self.modal {
            Modal::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?' | 'q' | 'Q')) {
                    self.modal = Modal::None;
                }
            }
            Modal::Search => self.handle_search_key(key),
            Modal::JumpPage(_) => self.handle_jump_key(key),
            Modal::Export(_) => self.handle_export_modal_key(key),
            Modal::Upload(_) => self.handle_upload_modal_key(key),
            Modal::None => self.handle_normal_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q' | 'Q') => self.should_quit = true,
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char('/') => {
                self.modal = Modal::Search;
            }
            KeyCode::Esc => {
                if self.view_layout == ViewLayout::FullFocus {
                    self.view_layout = self.prev_view_layout;
                    self.push_log(format!("View layout: {}", self.view_layout.label()));
                } else if !self.tree.filter.is_empty() {
                    self.tree.filter.clear();
                    self.tree.rebuild(&self.ocr_index);
                    self.clamp_selection();
                    self.select_current_row();
                }
            }
            // Strict Vim navigation for tree: always moves tree selection
            KeyCode::Char('j') => {
                if self.selected_idx + 1 < self.tree.rows.len() {
                    self.selected_idx += 1;
                    self.select_current_row();
                }
            }
            KeyCode::Char('k') => {
                if self.selected_idx > 0 {
                    self.selected_idx -= 1;
                    self.select_current_row();
                }
            }
            KeyCode::Char('G') => {
                if !self.tree.rows.is_empty() {
                    self.selected_idx = self.tree.rows.len() - 1;
                    self.select_current_row();
                }
            }
            KeyCode::Home => {
                if !self.tree.rows.is_empty() {
                    self.selected_idx = 0;
                    self.select_current_row();
                }
            }
            // Arrow keys: Pan when zoomed, navigate tree when unzoomed
            KeyCode::Up => {
                if self.zoom > 1.05 {
                    self.pan_y = (self.pan_y - 0.1).max(0.0);
                    self.rebuild_image_protocol();
                } else if self.selected_idx > 0 {
                    self.selected_idx -= 1;
                    self.select_current_row();
                }
            }
            KeyCode::Down => {
                if self.zoom > 1.05 {
                    self.pan_y = (self.pan_y + 0.1).min(1.0);
                    self.rebuild_image_protocol();
                } else if self.selected_idx + 1 < self.tree.rows.len() {
                    self.selected_idx += 1;
                    self.select_current_row();
                }
            }
            KeyCode::Left => {
                if self.zoom > 1.05 {
                    self.pan_x = (self.pan_x - 0.1).max(0.0);
                    self.rebuild_image_protocol();
                } else if let Some(row) = self.selected_row().cloned()
                    && row.kind == EntryKind::Directory
                    && row.expanded
                {
                    self.tree.expanded_dirs.remove(&row.path);
                    self.tree.rebuild(&self.ocr_index);
                    self.clamp_selection();
                }
            }
            KeyCode::Right => {
                if self.zoom > 1.05 {
                    self.pan_x = (self.pan_x + 0.1).min(1.0);
                    self.rebuild_image_protocol();
                } else if let Some(row) = self.selected_row().cloned()
                    && row.kind == EntryKind::Directory
                    && !row.expanded
                {
                    self.tree.expanded_dirs.insert(row.path);
                    self.tree.rebuild(&self.ocr_index);
                    self.clamp_selection();
                }
            }
            // 'h' / 'l' for tree folding
            KeyCode::Char('h') => {
                if let Some(row) = self.selected_row().cloned()
                    && row.kind == EntryKind::Directory
                    && row.expanded
                {
                    self.tree.expanded_dirs.remove(&row.path);
                    self.tree.rebuild(&self.ocr_index);
                    self.clamp_selection();
                }
            }
            KeyCode::Char('l') | KeyCode::Enter => {
                if let Some(row) = self.selected_row().cloned()
                    && row.kind == EntryKind::Directory
                {
                    if self.tree.expanded_dirs.contains(&row.path) {
                        self.tree.expanded_dirs.remove(&row.path);
                    } else {
                        self.tree.expanded_dirs.insert(row.path);
                    }
                    self.tree.rebuild(&self.ocr_index);
                    self.clamp_selection();
                }
            }
            // Page turning (both n/p and J/K, case-insensitive)
            KeyCode::Char('n' | 'N' | 'J') => {
                if self.total_pages > 0 && self.current_page < self.total_pages {
                    self.current_page += 1;
                    self.sync_layer_mask_for_current_page();
                    self.request_render();
                }
            }
            KeyCode::Char('p' | 'P' | 'K') => {
                if self.current_page > 1 {
                    self.current_page -= 1;
                    self.sync_layer_mask_for_current_page();
                    self.request_render();
                }
            }
            KeyCode::Char('g') => {
                if self.total_pages > 0 {
                    self.modal = Modal::JumpPage(String::new());
                }
            }
            KeyCode::Char('v') => {
                self.view_layout = self.view_layout.next();
                self.push_log(format!("View layout: {}", self.view_layout.label()));
            }
            KeyCode::Char('f' | 'F') => {
                if self.view_layout == ViewLayout::FullFocus {
                    self.view_layout = self.prev_view_layout;
                } else {
                    self.prev_view_layout = self.view_layout;
                    self.view_layout = ViewLayout::FullFocus;
                }
                self.push_log(format!("View layout: {}", self.view_layout.label()));
            }
            KeyCode::Char('i' | 'I') => {
                self.invert_colors = !self.invert_colors;
                self.push_log(format!(
                    "Dark/Invert mode: {}",
                    if self.invert_colors { "ON" } else { "OFF" }
                ));
                self.rebuild_image_protocol();
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.zoom = (self.zoom + 0.5).min(3.5);
                self.rebuild_image_protocol();
            }
            KeyCode::Char('-') => {
                self.zoom = (self.zoom - 0.5).max(1.0);
                if self.zoom <= 1.01 {
                    self.pan_x = 0.5;
                    self.pan_y = 0.5;
                }
                self.rebuild_image_protocol();
            }
            KeyCode::Char('0') => {
                self.zoom = 1.0;
                self.pan_x = 0.5;
                self.pan_y = 0.5;
                self.rebuild_image_protocol();
            }
            KeyCode::Char(c @ '1'..='5') => {
                let idx = (c as usize) - ('1' as usize);
                match &self.active_doc {
                    ActiveDoc::Pdf(_) if idx == 0 => {
                        self.show_mark_overlay = !self.show_mark_overlay;
                        self.push_log(format!(
                            "PDF .mark overlay: {}",
                            if self.show_mark_overlay { "ON" } else { "OFF" }
                        ));
                        self.request_render();
                    }
                    ActiveDoc::Note(_) if idx < self.layer_visibility.len() => {
                        self.layer_visibility[idx] = !self.layer_visibility[idx];
                        self.request_render();
                    }
                    _ => {}
                }
            }
            KeyCode::Char('e' | 'E') => self.open_export_modal(),
            KeyCode::Char('s' | 'S') => {
                self.push_log("Triggering pull-sync...");
                self.pending_sync_triggers.push(SyncTrigger::PullSync {
                    profile: self.config.default_profile.clone(),
                });
            }
            KeyCode::Char('u' | 'U') => self.open_upload_modal(),
            KeyCode::Char('r' | 'R') => {
                if let Some(path) = self.active_path.clone() {
                    let _ = self.worker.tx.send(WorkerRequest::InvalidatePath(path));
                }
                self.tree.rebuild(&self.ocr_index);
                self.clamp_selection();
                self.select_current_row();
                self.push_log("Refreshed workspace & invalidated active page cache");
            }
            _ => {}
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.tree.filter.clear();
                self.tree.rebuild(&self.ocr_index);
                self.clamp_selection();
                self.select_current_row();
                self.modal = Modal::None;
            }
            KeyCode::Enter => {
                self.modal = Modal::None;
            }
            KeyCode::Backspace => {
                self.tree.filter.pop();
                self.tree.rebuild(&self.ocr_index);
                self.clamp_selection();
                self.select_current_row();
            }
            KeyCode::Char(c) => {
                self.tree.filter.push(c);
                self.tree.rebuild(&self.ocr_index);
                self.clamp_selection();
                self.select_current_row();
            }
            _ => {}
        }
    }

    fn handle_jump_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.modal = Modal::None,
            KeyCode::Enter => {
                if let Modal::JumpPage(buf) = &self.modal
                    && let Ok(num) = buf.trim().parse::<u32>()
                    && num >= 1
                    && num <= self.total_pages
                {
                    self.current_page = num;
                    self.sync_layer_mask_for_current_page();
                    self.request_render();
                }
                self.modal = Modal::None;
            }
            KeyCode::Backspace => {
                if let Modal::JumpPage(buf) = &mut self.modal {
                    buf.pop();
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if let Modal::JumpPage(buf) = &mut self.modal
                    && buf.len() < 5
                {
                    buf.push(c);
                }
            }
            _ => {}
        }
    }

    fn open_export_modal(&mut self) {
        let formats = vec![
            ExportFormat::PdfRaster,
            ExportFormat::PdfAnnotated,
            ExportFormat::PdfVector,
            ExportFormat::Png,
            ExportFormat::Svg,
            ExportFormat::Txt,
            ExportFormat::Md,
        ];
        let default_idx = match self.active_doc {
            ActiveDoc::Pdf(_) => 1,
            _ => 0,
        };
        let scope = match self.selected_row().map(|r| r.kind) {
            Some(EntryKind::Directory) | None => ExportScope::WorkspaceFolder,
            _ => ExportScope::SelectedFile,
        };
        self.modal = Modal::Export(ExportModalState {
            scope,
            format_idx: default_idx,
            formats,
            dest_dir: self.config.export_dir.clone(),
        });
    }

    fn handle_export_modal_key(&mut self, key: KeyEvent) {
        let selected_path = self.selected_row().map(|r| r.path.clone());
        let workspace_root = self.tree.root.clone();
        let mut launch: Option<(PathBuf, PathBuf, ExportFormat)> = None;
        if let Modal::Export(state) = &mut self.modal {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                    self.modal = Modal::None;
                    return;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    state.format_idx = (state.format_idx + 1) % state.formats.len();
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    state.format_idx = if state.format_idx == 0 {
                        state.formats.len() - 1
                    } else {
                        state.format_idx - 1
                    };
                }
                KeyCode::Tab | KeyCode::Char(' ') => {
                    state.scope = match state.scope {
                        ExportScope::SelectedFile => ExportScope::WorkspaceFolder,
                        ExportScope::WorkspaceFolder => ExportScope::SelectedFile,
                    };
                }
                KeyCode::Enter => {
                    let format = state.formats[state.format_idx];
                    let input = match state.scope {
                        ExportScope::SelectedFile => {
                            selected_path.unwrap_or(workspace_root)
                        }
                        ExportScope::WorkspaceFolder => workspace_root,
                    };
                    let output = state.dest_dir.clone();
                    launch = Some((input, output, format));
                }
                _ => {}
            }
        }

        if let Some((input, output, format)) = launch {
            self.modal = Modal::None;
            self.push_log(format!(
                "Export queued: {} -> {} ({})",
                input.display(),
                output.display(),
                format.display_name()
            ));
            let _ = self.worker.tx.send(WorkerRequest::Export {
                input,
                output,
                format,
                workers: self.config.max_workers,
            });
        }
    }

    fn open_upload_modal(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            self.push_log("Select a local file to upload first.");
            return;
        };
        if row.kind == EntryKind::Directory {
            self.push_log("Upload expects a file (e.g. PDF or template), not a directory.");
            return;
        }
        self.modal = Modal::Upload(UploadModalState {
            local_file: row.path,
            remote_dest: "/Document".to_string(),
        });
    }

    fn handle_upload_modal_key(&mut self, key: KeyEvent) {
        let mut submit: Option<(PathBuf, String)> = None;
        if let Modal::Upload(state) = &mut self.modal {
            match key.code {
                KeyCode::Esc => {
                    self.modal = Modal::None;
                    return;
                }
                KeyCode::Backspace => {
                    state.remote_dest.pop();
                }
                KeyCode::Char(c) => {
                    state.remote_dest.push(c);
                }
                KeyCode::Enter => {
                    let dest = if state.remote_dest.trim().is_empty() {
                        "/Document".to_string()
                    } else {
                        state.remote_dest.trim().to_string()
                    };
                    submit = Some((state.local_file.clone(), dest));
                }
                _ => {}
            }
        }
        if let Some((local_file, remote_dest)) = submit {
            self.modal = Modal::None;
            self.push_log(format!(
                "Uploading {} -> {remote_dest}...",
                local_file.display()
            ));
            self.pending_sync_triggers.push(SyncTrigger::UploadFile {
                local_file,
                remote_dest,
                profile: self.config.default_profile.clone(),
            });
        }
    }

    pub fn notify_fs_change(&mut self, changed_path: &Path) {
        let _ = self
            .worker
            .tx
            .send(WorkerRequest::InvalidatePath(changed_path.to_path_buf()));
        self.tree.rebuild(&self.ocr_index);
        self.clamp_selection();
        if self.active_path.as_deref() == Some(changed_path) {
            self.select_current_row();
        }
    }

    fn clamp_selection(&mut self) {
        if self.tree.rows.is_empty() {
            self.selected_idx = 0;
        } else if self.selected_idx >= self.tree.rows.len() {
            self.selected_idx = self.tree.rows.len() - 1;
        }
    }

    pub fn select_current_row(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            self.active_path = None;
            self.active_doc = ActiveDoc::None;
            self.raw_image = None;
            self.image_protocol = None;
            self.total_pages = 0;
            return;
        };

        if row.kind == EntryKind::Directory {
            self.active_path = Some(row.path);
            self.active_doc = ActiveDoc::None;
            self.raw_image = None;
            self.image_protocol = None;
            self.active_ocr_text = None;
            self.total_pages = 0;
            self.loading_page = false;
            return;
        }

        self.active_path = Some(row.path.clone());
        self.zoom = 1.0;
        self.pan_x = 0.5;
        self.pan_y = 0.5;
        self.current_page = 1;

        match row.kind {
            EntryKind::Note | EntryKind::StandaloneMark => match parse_notebook_file(&row.path) {
                Ok(nb) => {
                    self.total_pages = nb.pages.len() as u32;
                    self.tree.set_page_count(&row.path, nb.pages.len());
                    self.active_doc = ActiveDoc::Note(nb);
                    self.sync_layer_mask_for_current_page();
                    self.request_render();
                }
                Err(e) => {
                    self.active_doc = ActiveDoc::None;
                    self.total_pages = 0;
                    self.raw_image = None;
                    self.image_protocol = None;
                    self.push_log(format!("Parse error ({}): {e:#}", row.name));
                }
            },
            EntryKind::Pdf => match inspect_pdf(&row.path) {
                Ok(info) => {
                    self.total_pages = info.page_count;
                    self.tree.set_page_count(&row.path, info.page_count as usize);
                    self.active_doc = ActiveDoc::Pdf(info);
                    self.layer_visibility.clear();
                    self.request_render();
                }
                Err(e) => {
                    self.active_doc = ActiveDoc::None;
                    self.total_pages = 0;
                    self.raw_image = None;
                    self.image_protocol = None;
                    self.push_log(format!("PDF inspect error ({}): {e:#}", row.name));
                }
            },
            EntryKind::Directory => {}
        }
    }

    fn sync_layer_mask_for_current_page(&mut self) {
        if let ActiveDoc::Note(nb) = &self.active_doc {
            let idx = (self.current_page as usize).saturating_sub(1);
            if let Some(page) = nb.pages.get(idx) {
                self.layer_visibility = page.layers.iter().map(|l| l.visible).collect();
            }
        }
    }

    fn request_render(&mut self) {
        let Some(path) = self.active_path.clone() else {
            return;
        };
        if self.total_pages == 0 {
            return;
        }
        self.render_seq += 1;
        self.loading_page = true;
        let _ = self.worker.tx.send(WorkerRequest::RenderPage {
            seq: self.render_seq,
            path,
            page_number: self.current_page,
            total_pages: self.total_pages,
            layer_mask: self.layer_visibility.clone(),
            show_mark: self.show_mark_overlay,
            dpi: self.config.pdf_render_dpi,
        });
    }

    fn rebuild_image_protocol(&mut self) {
        let Some(img) = &self.raw_image else {
            self.image_protocol = None;
            return;
        };
        let mut display_img = if self.zoom > 1.05 {
            crop_for_zoom(img, self.zoom, self.pan_x, self.pan_y)
        } else {
            img.clone()
        };
        if self.invert_colors {
            let mut rgba = display_img.to_rgba8();
            crate::note::invert_image(&mut rgba);
            display_img = DynamicImage::ImageRgba8(rgba);
        }
        self.image_protocol = Some(self.picker.new_resize_protocol(display_img));
    }
}

fn crop_for_zoom(img: &DynamicImage, zoom: f32, pan_x: f32, pan_y: f32) -> DynamicImage {
    let w = img.width();
    let h = img.height();
    let crop_w = ((w as f32) / zoom).clamp(16.0, w as f32) as u32;
    let crop_h = ((h as f32) / zoom).clamp(16.0, h as f32) as u32;
    let max_x = w.saturating_sub(crop_w);
    let max_y = h.saturating_sub(crop_h);
    let x = ((max_x as f32) * pan_x.clamp(0.0, 1.0)) as u32;
    let y = ((max_y as f32) * pan_y.clamp(0.0, 1.0)) as u32;
    img.crop_imm(x, y, crop_w, crop_h)
}
