//! Unified single-file and directory-hierarchy export pipeline.

pub mod mark_pdf;
pub mod raster_pdf;
pub mod supernote_tool;

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use image::ImageFormat;
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::note::{extract_notebook_text, parse_notebook_file, render_page_gray};
use crate::pdf::find_companion_mark;
pub use mark_pdf::export_annotated_pdf;
use raster_pdf::export_note_raster_pdf;
use supernote_tool::SupernoteToolBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    /// Fast native Rust /DeviceGray raster PDF (for `.note`) or lossless annotated PDF (for `.pdf` + `.mark`)
    PdfRaster,
    /// Lossless `.pdf` + `.pdf.mark` annotation overlay via pure-Rust `lopdf`
    PdfAnnotated,
    /// Vector PDF via optional `supernote-tool`
    PdfVector,
    /// Native Rust PNG sequence (one PNG per page)
    Png,
    /// Vector SVG pages via optional `supernote-tool`
    Svg,
    /// Native Rust extracted `RECOGNTEXT` plain text
    Txt,
    /// Native Rust extracted `RECOGNTEXT` Markdown with page headings
    Md,
}

impl ExportFormat {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::PdfRaster => "PDF (Fast Raster — Native Rust)",
            Self::PdfAnnotated => "PDF (Lossless .mark Overlay — Native Rust)",
            Self::PdfVector => "PDF (Vector — supernote-tool)",
            Self::Png => "PNG Sequence (Native Rust)",
            Self::Svg => "SVG Vector Pages (supernote-tool)",
            Self::Txt => "Recognition Text .txt (Native Rust)",
            Self::Md => "Recognition Markdown .md (Native Rust)",
        }
    }

    pub fn requires_supernote_tool(self) -> bool {
        matches!(self, Self::PdfVector | Self::Svg)
    }

    pub fn default_extension(self) -> &'static str {
        match self {
            Self::PdfRaster | Self::PdfAnnotated | Self::PdfVector => "pdf",
            Self::Png => "png",
            Self::Svg => "svg",
            Self::Txt => "txt",
            Self::Md => "md",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportSummary {
    pub files_exported: usize,
    pub outputs: Vec<PathBuf>,
    pub skipped: usize,
}

/// Exports a single `.note` or `.pdf` file, or recursively mirrors an entire directory tree.
pub fn export_path(
    input: &Path,
    output: &Path,
    format: ExportFormat,
    max_threads: usize,
) -> Result<ExportSummary> {
    if !input.exists() {
        bail!("input path does not exist: {}", input.display());
    }

    if input.is_file() {
        let out_file = resolve_single_output_path(input, output, format);
        export_single_file(input, &out_file, format, max_threads)?;
        return Ok(ExportSummary {
            files_exported: 1,
            outputs: vec![out_file],
            skipped: 0,
        });
    }

    // Recursive directory export preserving exact folder hierarchy
    fs::create_dir_all(output)
        .with_context(|| format!("failed to create output directory {}", output.display()))?;

    let mut targets: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in WalkDir::new(input).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();

        let is_exportable = match ext.as_str() {
            "note" => true,
            "pdf" => find_companion_mark(path).is_some(),
            _ => false,
        };
        if !is_exportable {
            continue;
        }

        let rel = path.strip_prefix(input).unwrap_or(path);
        let dest = output.join(rel).with_extension(format.default_extension());
        targets.push((path.to_path_buf(), dest));
    }

    let mut outputs = Vec::with_capacity(targets.len());
    let mut skipped = 0usize;
    for (src, dst) in targets {
        match export_single_file(&src, &dst, format, max_threads) {
            Ok(()) => outputs.push(dst),
            Err(_) => skipped += 1,
        }
    }

    Ok(ExportSummary {
        files_exported: outputs.len(),
        outputs,
        skipped,
    })
}

fn resolve_single_output_path(input: &Path, output: &Path, format: ExportFormat) -> PathBuf {
    if output.is_dir() {
        let stem = input.file_stem().unwrap_or_default();
        let mut p = output.join(stem);
        p.set_extension(format.default_extension());
        p
    } else {
        output.to_path_buf()
    }
}

pub fn export_single_file(
    input: &Path,
    output: &Path,
    format: ExportFormat,
    max_threads: usize,
) -> Result<()> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    if ext == "pdf" {
        return match format {
            ExportFormat::PdfRaster | ExportFormat::PdfAnnotated => {
                if let Some(mark_path) = find_companion_mark(input) {
                    export_annotated_pdf(input, &mark_path, output)
                } else {
                    if let Some(parent) = output.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(input, output)?;
                    Ok(())
                }
            }
            _ => bail!(
                "format {:?} is for .note files; use `pdf-annotated` or `pdf-raster` for PDF + .mark files",
                format
            ),
        };
    }

    if ext != "note" {
        bail!("unsupported input file extension for export: {}", input.display());
    }

    match format {
        ExportFormat::PdfRaster | ExportFormat::PdfAnnotated => {
            export_note_raster_pdf(input, output, max_threads)
        }
        ExportFormat::PdfVector => {
            let backend = SupernoteToolBackend::detect();
            backend.export_vector_pdf(input, output)
        }
        ExportFormat::Svg => {
            let backend = SupernoteToolBackend::detect();
            backend.export_svg(input, output)
        }
        ExportFormat::Png => export_note_png_sequence(input, output, max_threads),
        ExportFormat::Txt => export_note_text(input, output, false),
        ExportFormat::Md => export_note_text(input, output, true),
    }
}

fn export_note_png_sequence(input: &Path, output: &Path, max_threads: usize) -> Result<()> {
    let nb = parse_notebook_file(input)?;
    if nb.pages.is_empty() {
        bail!("cannot export PNGs: notebook {} has 0 pages", input.display());
    }

    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(max_threads.clamp(1, 4))
        .build()?;

    let multi = nb.pages.len() > 1;
    pool.install(|| {
        nb.pages.par_iter().try_for_each(|page| -> Result<()> {
            let mut file = File::open(input)?;
            let gray = render_page_gray(&mut file, page, None)?;
            let out_path = if multi {
                parent.join(format!("{stem}_p{:03}.png", page.page_number))
            } else {
                output.to_path_buf()
            };
            gray.save_with_format(&out_path, ImageFormat::Png)
                .with_context(|| format!("failed to write PNG {}", out_path.display()))?;
            Ok(())
        })
    })
}

fn export_note_text(input: &Path, output: &Path, markdown: bool) -> Result<()> {
    let pages = extract_notebook_text(input)?;
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let title = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Notebook");

    let mut content = String::new();
    if markdown {
        content.push_str(&format!("# {title}\n\n"));
    }

    if pages.is_empty() {
        content.push_str(if markdown {
            "_No real-time recognition text embedded in this notebook._\n"
        } else {
            ""
        });
    } else {
        for (idx, (page_num, text)) in pages.iter().enumerate() {
            if idx > 0 {
                content.push_str("\n\n");
            }
            if markdown {
                content.push_str(&format!("## Page {page_num}\n\n{text}\n"));
            } else {
                if pages.len() > 1 {
                    content.push_str(&format!("--- Page {page_num} ---\n"));
                }
                content.push_str(text);
                content.push('\n');
            }
        }
    }

    fs::write(output, content)
        .with_context(|| format!("failed to write text export {}", output.display()))?;
    Ok(())
}
