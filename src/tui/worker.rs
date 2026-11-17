//! Non-blocking background OS worker thread for page rendering, bounded LRU caching,
//! lazy OCR indexing, and asynchronous exports.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use image::DynamicImage;

use crate::cache::{OcrIndex, PageCache, PageCacheKey};
use crate::export::{ExportFormat, export_path};
use crate::note::{extract_page_text, parse_notebook, render_page_rgba};
use crate::pdf::render_pdf_page;

#[derive(Debug)]
pub enum WorkerRequest {
    RenderPage {
        seq: u64,
        path: PathBuf,
        page_number: u32,
        total_pages: u32,
        layer_mask: Vec<bool>,
        show_mark: bool,
        dpi: u32,
    },
    Export {
        input: PathBuf,
        output: PathBuf,
        format: ExportFormat,
        workers: usize,
    },
    IndexOcr {
        paths: Vec<PathBuf>,
    },
    InvalidatePath(PathBuf),
    Shutdown,
}

#[derive(Debug)]
pub enum WorkerEvent {
    PageRendered {
        seq: u64,
        path: PathBuf,
        page_number: u32,
        image: DynamicImage,
        ocr_text: Option<String>,
        cache_bytes: u64,
    },
    RenderError {
        seq: u64,
        path: PathBuf,
        message: String,
    },
    ExportFinished {
        message: String,
        ok: bool,
    },
    OcrIndexed {
        path: PathBuf,
        text: String,
    },
}

pub struct RenderWorker {
    pub tx: Sender<WorkerRequest>,
    pub rx: Receiver<WorkerEvent>,
    handle: Option<JoinHandle<()>>,
}

impl RenderWorker {
    pub fn spawn(cache_root: PathBuf, cache_max_mb: u64) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<WorkerRequest>();
        let (evt_tx, evt_rx) = mpsc::channel::<WorkerEvent>();

        let handle = thread::Builder::new()
            .name("sn-tui-render".to_string())
            .spawn(move || {
                worker_loop(cache_root, cache_max_mb, req_rx, evt_tx);
            })
            .expect("spawn render worker thread");

        Self {
            tx: req_tx,
            rx: evt_rx,
            handle: Some(handle),
        }
    }
}

impl Drop for RenderWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(WorkerRequest::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn worker_loop(
    cache_root: PathBuf,
    cache_max_mb: u64,
    rx: Receiver<WorkerRequest>,
    tx: Sender<WorkerEvent>,
) {
    let mut cache = PageCache::new(&cache_root, cache_max_mb).ok();
    let mut ocr_index = OcrIndex::load(&cache_root);

    while let Ok(first_req) = rx.recv() {
        // Coalesce bursty RenderPage requests so rapid scrolling only renders the latest target!
        let mut pending_render: Option<WorkerRequest> = None;
        let mut batch = vec![first_req];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }

        for req in batch {
            match req {
                WorkerRequest::Shutdown => return,
                WorkerRequest::RenderPage { .. } => {
                    pending_render = Some(req);
                }
                WorkerRequest::InvalidatePath(path) => {
                    if let Some(c) = cache.as_mut() {
                        c.invalidate_path(&path);
                    }
                    ocr_index.invalidate(&path);
                }
                WorkerRequest::Export {
                    input,
                    output,
                    format,
                    workers,
                } => {
                    let evt_tx = tx.clone();
                    thread::spawn(move || {
                        let res = export_path(&input, &output, format, workers);
                        let event = match res {
                            Ok(summary) => WorkerEvent::ExportFinished {
                                message: format!(
                                    "Exported {} file(s) to {} ({})",
                                    summary.files_exported,
                                    output.display(),
                                    format.display_name()
                                ),
                                ok: true,
                            },
                            Err(e) => WorkerEvent::ExportFinished {
                                message: format!("Export failed: {e:#}"),
                                ok: false,
                            },
                        };
                        let _ = evt_tx.send(event);
                    });
                }
                WorkerRequest::IndexOcr { paths } => {
                    let mut changed = false;
                    for path in paths {
                        if let Some(text) = ocr_index.get_or_index(&path) {
                            let text_owned = text.to_string();
                            let _ = tx.send(WorkerEvent::OcrIndexed {
                                path,
                                text: text_owned,
                            });
                            changed = true;
                        }
                    }
                    if changed {
                        let _ = ocr_index.save();
                    }
                }
            }
        }

        if let Some(WorkerRequest::RenderPage {
            seq,
            path,
            page_number,
            total_pages,
            layer_mask,
            show_mark,
            dpi,
        }) = pending_render
        {
            match render_with_cache(
                cache.as_mut(),
                &path,
                page_number,
                &layer_mask,
                show_mark,
                dpi,
            ) {
                Ok((image, ocr_text)) => {
                    let cache_bytes = cache.as_ref().map_or(0, PageCache::disk_usage_bytes);
                    let _ = tx.send(WorkerEvent::PageRendered {
                        seq,
                        path: path.clone(),
                        page_number,
                        image,
                        ocr_text,
                        cache_bytes,
                    });

                    // Prefetch adjacent page if no newer request arrived
                    if rx.try_recv().is_err() && page_number < total_pages {
                        let _ = render_with_cache(
                            cache.as_mut(),
                            &path,
                            page_number + 1,
                            &layer_mask,
                            show_mark,
                            dpi,
                        );
                    }
                }
                Err(err) => {
                    let _ = tx.send(WorkerEvent::RenderError {
                        seq,
                        path,
                        message: format!("{err:#}"),
                    });
                }
            }
        }
    }
}

fn layer_bits(mask: &[bool]) -> u64 {
    let mut bits = 0u64;
    for (i, &vis) in mask.iter().take(64).enumerate() {
        if vis {
            bits |= 1u64 << i;
        }
    }
    bits
}

fn render_with_cache(
    cache: Option<&mut PageCache>,
    path: &Path,
    page_number: u32,
    layer_mask: &[bool],
    show_mark: bool,
    dpi: u32,
) -> anyhow::Result<(DynamicImage, Option<String>)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    let bits = layer_bits(layer_mask);
    let key = PageCacheKey::from_file(path, page_number, bits, show_mark);

    let ocr_text = if ext == "note" {
        extract_single_page_ocr(path, page_number)
    } else {
        None
    };

    if let Some(c) = cache {
        if let Some(hit) = c.get(&key) {
            return Ok((hit, ocr_text));
        }
        let img = decode_page(path, &ext, page_number, layer_mask, show_mark, dpi)?;
        c.put(key, img.clone());
        Ok((img, ocr_text))
    } else {
        let img = decode_page(path, &ext, page_number, layer_mask, show_mark, dpi)?;
        Ok((img, ocr_text))
    }
}

fn decode_page(
    path: &Path,
    ext: &str,
    page_number: u32,
    layer_mask: &[bool],
    show_mark: bool,
    dpi: u32,
) -> anyhow::Result<DynamicImage> {
    match ext {
        "note" | "mark" => {
            let mut file = File::open(path)?;
            let nb = parse_notebook(&mut file)?;
            let idx = (page_number as usize)
                .saturating_sub(1)
                .min(nb.pages.len().saturating_sub(1));
            let page = nb
                .pages
                .get(idx)
                .ok_or_else(|| anyhow::anyhow!("notebook has 0 pages"))?;
            let mask_opt = if layer_mask.is_empty() {
                None
            } else {
                Some(layer_mask)
            };
            let rgba = render_page_rgba(&mut file, page, mask_opt, false)?;
            Ok(DynamicImage::ImageRgba8(rgba))
        }
        "pdf" => render_pdf_page(path, page_number, dpi, show_mark),
        _ => anyhow::bail!("unsupported file type: {}", path.display()),
    }
}

fn extract_single_page_ocr(path: &Path, page_number: u32) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let nb = parse_notebook(&mut file).ok()?;
    let idx = (page_number as usize).saturating_sub(1);
    let page = nb.pages.get(idx)?;
    extract_page_text(&mut file, page).ok().flatten()
}
