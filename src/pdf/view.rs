//! Lightweight PDF inspection (`pdfinfo` / `lopdf` fallback), single-page rasterization
//! (`pdftoppm`), and companion `.pdf.mark` RGBA alpha compositing.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, imageops};
use serde::Serialize;

use crate::note::{parse_notebook, render_page_rgba};

#[derive(Debug, Clone, Serialize)]
pub struct PdfInfo {
    pub page_count: u32,
    pub companion_mark: Option<PathBuf>,
    pub annotated_pages: Vec<u32>,
}

/// Checks for a Supernote companion annotation file `<file.pdf>.mark`.
pub fn find_companion_mark(pdf_path: &Path) -> Option<PathBuf> {
    let mut os = pdf_path.as_os_str().to_os_string();
    os.push(".mark");
    let candidate = PathBuf::from(os);
    candidate.is_file().then_some(candidate)
}

/// Returns total page count and optional `.pdf.mark` annotation page list.
pub fn inspect_pdf(pdf_path: &Path) -> Result<PdfInfo> {
    let page_count = pdf_page_count(pdf_path)?;
    let companion_mark = find_companion_mark(pdf_path);
    let annotated_pages = if let Some(mark_path) = &companion_mark {
        match File::open(mark_path).and_then(|mut f| {
            parse_notebook(&mut f).map_err(|e| std::io::Error::other(e.to_string()))
        }) {
            Ok(nb) => nb.pages.iter().map(|p| p.page_number).collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    Ok(PdfInfo {
        page_count,
        companion_mark,
        annotated_pages,
    })
}

/// Gets PDF page count via `pdfinfo`, falling back to pure-Rust `lopdf` if `pdfinfo` is absent.
pub fn pdf_page_count(pdf_path: &Path) -> Result<u32> {
    if let Ok(output) = Command::new("pdfinfo").arg(pdf_path).output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(rest) = line.strip_prefix("Pages:")
                && let Ok(n) = rest.trim().parse::<u32>()
            {
                return Ok(n);
            }
        }
    }

    // Pure-Rust fallback via lopdf
    let doc = lopdf::Document::load(pdf_path)
        .with_context(|| format!("failed to inspect PDF {}", pdf_path.display()))?;
    Ok(doc.get_pages().len() as u32)
}

/// Rasterizes a single 1-based page of `pdf_path` via `pdftoppm` and optionally composites
/// the matching page from a companion `<file.pdf>.mark` annotation file on top.
pub fn render_pdf_page(
    pdf_path: &Path,
    page_number: u32,
    dpi: u32,
    show_mark_overlay: bool,
) -> Result<DynamicImage> {
    let page_str = page_number.to_string();
    let dpi_str = dpi.to_string();

    // Passing no output prefix causes poppler's pdftoppm to stream PNG bytes directly to stdout.
    let output = Command::new("pdftoppm")
        .args([
            "-png",
            "-r",
            &dpi_str,
            "-f",
            &page_str,
            "-l",
            &page_str,
            "-singlefile",
        ])
        .arg(pdf_path)
        .output()
        .context("failed to run `pdftoppm` (is `poppler` installed?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "pdftoppm failed on {} page {page_number}: {}",
            pdf_path.display(),
            stderr.trim()
        );
    }
    if output.stdout.is_empty() {
        bail!(
            "pdftoppm produced empty output for {} page {page_number}",
            pdf_path.display()
        );
    }

    let mut base = image::load_from_memory(&output.stdout)
        .context("failed to decode PNG stream from pdftoppm")?
        .to_rgba8();

    if show_mark_overlay
        && let Some(mark_path) = find_companion_mark(pdf_path)
    {
        let mut mark_file = File::open(&mark_path)
            .with_context(|| format!("failed to open companion mark {}", mark_path.display()))?;
        let mark_nb = parse_notebook(&mut mark_file)?;
        if let Some((_, mark_page)) = mark_nb.find_page_by_number(page_number) {
            let mut overlay = render_page_rgba(&mut mark_file, mark_page, None, true)?;
            if overlay.width() != base.width() || overlay.height() != base.height() {
                overlay = imageops::resize(
                    &overlay,
                    base.width(),
                    base.height(),
                    imageops::FilterType::Triangle,
                );
            }
            imageops::overlay(&mut base, &overlay, 0, 0);
        }
    }

    Ok(DynamicImage::ImageRgba8(base))
}
