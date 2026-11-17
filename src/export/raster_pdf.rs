//! Direct minimal PDF 1.7 writer using 8-bit `/DeviceGray` + `/FlateDecode` zlib streams.
//!
//! Adapted from `RohanGautam/supernote_pdf` (MIT) and optimized to use `/DeviceGray`
//! (1/3 the memory and ~3x smaller output than RGB for monochrome e-ink notebooks).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use rayon::prelude::*;

use crate::note::{parse_notebook_file, render_page_gray};

const A4_SHORT_PT: f32 = 595.28;
const A4_LONG_PT: f32 = 841.89;

struct PdfPageChunk {
    page_obj: Vec<u8>,
    contents_obj: Vec<u8>,
    image_obj: Vec<u8>,
}

/// Converts a Supernote `.note` file directly into a compact grayscale raster PDF.
pub fn export_note_raster_pdf(input_path: &Path, output_path: &Path, max_threads: usize) -> Result<()> {
    let notebook = parse_notebook_file(input_path)?;
    if notebook.pages.is_empty() {
        bail!("cannot export {}: notebook has 0 pages", input_path.display());
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(max_threads.clamp(1, 4))
        .build()
        .context("failed to build bounded rayon pool")?;

    let page_chunks: Vec<PdfPageChunk> = pool.install(|| {
        notebook
            .pages
            .par_iter()
            .enumerate()
            .map(|(i, page)| {
                let mut file = File::open(input_path).with_context(|| {
                    format!("failed to open {} for page {}", input_path.display(), page.page_number)
                })?;
                let gray = render_page_gray(&mut file, page, None)?;
                let (w, h) = gray.dimensions();

                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(gray.as_raw())?;
                let compressed = encoder.finish()?;

                let (box_w, box_h) = if page.landscape || w > h {
                    (A4_LONG_PT, A4_SHORT_PT)
                } else {
                    (A4_SHORT_PT, A4_LONG_PT)
                };

                let page_id = i * 3 + 3;
                let contents_id = page_id + 1;
                let image_id = page_id + 2;

                let page_obj = format!(
                    "{page_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {box_w:.2} {box_h:.2}] \
                     /Contents {contents_id} 0 R /Resources << /XObject << /Im1 {image_id} 0 R >> >> >>\nendobj\n"
                )
                .into_bytes();

                let contents = format!("q\n{box_w:.2} 0 0 {box_h:.2} 0 0 cm\n/Im1 Do\nQ\n");
                let contents_obj = format!(
                    "{contents_id} 0 obj\n<< /Length {} >>\nstream\n{contents}endstream\nendobj\n",
                    contents.len()
                )
                .into_bytes();

                let mut image_obj = format!(
                    "{image_id} 0 obj\n<< /Type /XObject /Subtype /Image /Width {w} /Height {h} \
                     /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
                    compressed.len()
                )
                .into_bytes();
                image_obj.extend_from_slice(&compressed);
                image_obj.extend_from_slice(b"\nendstream\nendobj\n");

                Ok(PdfPageChunk {
                    page_obj,
                    contents_obj,
                    image_obj,
                })
            })
            .collect::<Result<Vec<_>>>()
    })?;

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let out_file = File::create(output_path)
        .with_context(|| format!("failed to create {}", output_path.display()))?;
    let mut writer = BufWriter::new(out_file);

    let total_pages = page_chunks.len();
    let mut byte_offset = 0u64;
    let mut xref_offsets = vec![0u64; total_pages * 3 + 2];

    let header = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n";
    writer.write_all(header)?;
    byte_offset += header.len() as u64;

    // 1: Catalog
    xref_offsets[0] = byte_offset;
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    writer.write_all(catalog)?;
    byte_offset += catalog.len() as u64;

    // 2: Pages root
    xref_offsets[1] = byte_offset;
    let kids: Vec<String> = (0..total_pages)
        .map(|i| format!("{} 0 R", i * 3 + 3))
        .collect();
    let pages_root = format!(
        "2 0 obj\n<< /Type /Pages /Kids [ {} ] /Count {total_pages} >>\nendobj\n",
        kids.join(" ")
    )
    .into_bytes();
    writer.write_all(&pages_root)?;
    byte_offset += pages_root.len() as u64;

    for (i, chunk) in page_chunks.iter().enumerate() {
        let idx = i * 3 + 2;
        xref_offsets[idx] = byte_offset;
        writer.write_all(&chunk.page_obj)?;
        byte_offset += chunk.page_obj.len() as u64;

        xref_offsets[idx + 1] = byte_offset;
        writer.write_all(&chunk.contents_obj)?;
        byte_offset += chunk.contents_obj.len() as u64;

        xref_offsets[idx + 2] = byte_offset;
        writer.write_all(&chunk.image_obj)?;
        byte_offset += chunk.image_obj.len() as u64;
    }

    let xref_start = byte_offset;
    writer.write_all(format!("xref\n0 {}\n0000000000 65535 f \n", xref_offsets.len() + 1).as_bytes())?;
    for off in &xref_offsets {
        writer.write_all(format!("{off:010} 00000 n \n").as_bytes())?;
    }
    writer.write_all(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_start}\n%%EOF\n",
            xref_offsets.len() + 1
        )
        .as_bytes(),
    )?;
    writer.flush()?;

    Ok(())
}
