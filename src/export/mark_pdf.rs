//! Pure-Rust lossless PDF + `.pdf.mark` annotation overlay exporter via `lopdf`.
//!
//! Preserves 100% of original PDF vector text, fonts, and hyperlinks by stamping each
//! annotated `.mark` page as a transparent `/DeviceGray` + `/SMask` Image XObject on top.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use image::imageops;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use crate::note::{parse_notebook, render_page_rgba};

/// Overlays annotations from `mark_path` onto `pdf_path` losslessly and writes to `output_path`.
pub fn export_annotated_pdf(pdf_path: &Path, mark_path: &Path, output_path: &Path) -> Result<()> {
    let mut doc = Document::load(pdf_path)
        .with_context(|| format!("failed to load base PDF {}", pdf_path.display()))?;
    let mut mark_file = File::open(mark_path)
        .with_context(|| format!("failed to open mark file {}", mark_path.display()))?;
    let mark_nb = parse_notebook(&mut mark_file)?;

    let pdf_pages = doc.get_pages();

    for mark_page in &mark_nb.pages {
        let Some(&page_id) = pdf_pages.get(&mark_page.page_number) else {
            continue;
        };

        let (x0, y0, box_w, box_h) = page_media_box(&doc, page_id).unwrap_or((0.0, 0.0, 595.28, 841.89));

        let rgba = render_page_rgba(&mut mark_file, mark_page, None, true)?;

        // Supernote letterboxes non-native aspect ratio PDFs on screen.
        // Crop the .mark canvas to the subregion corresponding to the actual PDF page
        // so annotations align 1:1 with vector text on exported slides/documents.
        let canvas_w = mark_page.width as f64;
        let canvas_h = mark_page.height as f64;
        let scale = (canvas_w / box_w).min(canvas_h / box_h);
        let target_w = (box_w * scale).round() as u32;
        let target_h = (box_h * scale).round() as u32;
        let offset_x = (mark_page.width.saturating_sub(target_w)) / 2;
        let offset_y = (mark_page.height.saturating_sub(target_h)) / 2;

        let cropped = if offset_x > 0 || offset_y > 0 || target_w != mark_page.width || target_h != mark_page.height {
            imageops::crop_imm(&rgba, offset_x, offset_y, target_w, target_h).to_image()
        } else {
            rgba
        };
        let (w, h) = cropped.dimensions();

        // Split RGBA into compact 8-bit grayscale + 8-bit alpha SMask.
        let pixel_count = (w as usize) * (h as usize);
        let mut gray_raw = Vec::with_capacity(pixel_count);
        let mut alpha_raw = Vec::with_capacity(pixel_count);
        let mut has_ink = false;
        for px in cropped.as_raw().as_chunks::<4>().0 {
            let luma = if px[0] == px[1] && px[1] == px[2] {
                px[0]
            } else {
                ((px[0] as u32 * 77 + px[1] as u32 * 150 + px[2] as u32 * 29) >> 8) as u8
            };
            gray_raw.push(luma);
            alpha_raw.push(px[3]);
            if px[3] > 0 {
                has_ink = true;
            }
        }
        if !has_ink {
            continue;
        }

        let gray_zlib = zlib_compress(&gray_raw)?;
        let alpha_zlib = zlib_compress(&alpha_raw)?;

        let smask_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => w as i64,
                "Height" => h as i64,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
                "Filter" => "FlateDecode",
            },
            alpha_zlib,
        ));

        let image_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => w as i64,
                "Height" => h as i64,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
                "Filter" => "FlateDecode",
                "SMask" => smask_id,
            },
            gray_zlib,
        ));

        let xobj_name = format!("SnMarkP{}", mark_page.page_number);
        attach_xobject_to_page(&mut doc, page_id, &xobj_name, image_id)?;

        // Wrap existing page contents in q ... Q so our overlay always draws in clean page space.
        let save_id = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
        let overlay_ops = format!("Q\nq\n{box_w:.3} 0 0 {box_h:.3} {x0:.3} {y0:.3} cm\n/{xobj_name} Do\nQ\n");
        let overlay_id = doc.add_object(Stream::new(Dictionary::new(), overlay_ops.into_bytes()));

        prepend_and_append_contents(&mut doc, page_id, save_id, overlay_id)?;
    }

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    doc.save(output_path)
        .with_context(|| format!("failed to save annotated PDF {}", output_path.display()))?;
    Ok(())
}

fn zlib_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

fn object_as_f64(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(f) => Some(*f as f64),
        _ => None,
    }
}

fn page_media_box(doc: &Document, mut obj_id: ObjectId) -> Option<(f64, f64, f64, f64)> {
    for _ in 0..16 {
        let dict = doc.get_object(obj_id).ok()?.as_dict().ok()?;
        if let Ok(mb) = dict.get(b"MediaBox") {
            let resolved = match mb {
                Object::Reference(r) => doc.get_object(*r).ok()?,
                other => other,
            };
            if let Ok(arr) = resolved.as_array()
                && arr.len() == 4
            {
                let x0 = object_as_f64(&arr[0])?;
                let y0 = object_as_f64(&arr[1])?;
                let x1 = object_as_f64(&arr[2])?;
                let y1 = object_as_f64(&arr[3])?;
                return Some((x0, y0, (x1 - x0).abs(), (y1 - y0).abs()));
            }
        }
        obj_id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn attach_xobject_to_page(
    doc: &mut Document,
    page_id: ObjectId,
    xobj_name: &str,
    image_id: ObjectId,
) -> Result<()> {
    // Resolve /Resources (may be inline Dictionary, indirect Reference, or missing)
    let res_ref = {
        let page_dict = doc
            .get_object(page_id)
            .map_err(|e| anyhow!("missing page object: {e}"))?
            .as_dict()
            .map_err(|e| anyhow!("page object is not a dict: {e}"))?;
        match page_dict.get(b"Resources") {
            Ok(Object::Reference(r)) => Some(*r),
            _ => None,
        }
    };

    if let Some(res_id) = res_ref {
        let xobj_ref = {
            let res_dict = doc.get_object(res_id)?.as_dict()?;
            match res_dict.get(b"XObject") {
                Ok(Object::Reference(r)) => Some(*r),
                _ => None,
            }
        };
        if let Some(xobj_id) = xobj_ref {
            let xobj_dict = doc.get_object_mut(xobj_id)?.as_dict_mut()?;
            xobj_dict.set(xobj_name.as_bytes(), Object::Reference(image_id));
        } else {
            let res_dict = doc.get_object_mut(res_id)?.as_dict_mut()?;
            if let Ok(Object::Dictionary(xobj_dict)) = res_dict.get_mut(b"XObject") {
                xobj_dict.set(xobj_name.as_bytes(), Object::Reference(image_id));
            } else {
                let mut xobj_dict = Dictionary::new();
                xobj_dict.set(xobj_name.as_bytes(), Object::Reference(image_id));
                res_dict.set("XObject", Object::Dictionary(xobj_dict));
            }
        }
        return Ok(());
    }

    // Inline /Resources on page dictionary
    let page_dict = doc.get_object_mut(page_id)?.as_dict_mut()?;
    if !page_dict.has(b"Resources") {
        page_dict.set("Resources", Object::Dictionary(Dictionary::new()));
    }
    let res_dict = page_dict.get_mut(b"Resources")?.as_dict_mut()?;
    if let Ok(Object::Reference(xobj_id)) = res_dict.get(b"XObject").cloned() {
        let xobj_dict = doc.get_object_mut(xobj_id)?.as_dict_mut()?;
        xobj_dict.set(xobj_name.as_bytes(), Object::Reference(image_id));
    } else if let Ok(Object::Dictionary(xobj_dict)) = res_dict.get_mut(b"XObject") {
        xobj_dict.set(xobj_name.as_bytes(), Object::Reference(image_id));
    } else {
        let mut xobj_dict = Dictionary::new();
        xobj_dict.set(xobj_name.as_bytes(), Object::Reference(image_id));
        res_dict.set("XObject", Object::Dictionary(xobj_dict));
    }

    Ok(())
}

fn prepend_and_append_contents(
    doc: &mut Document,
    page_id: ObjectId,
    save_id: ObjectId,
    overlay_id: ObjectId,
) -> Result<()> {
    let existing = doc
        .get_object(page_id)?
        .as_dict()?
        .get(b"Contents")
        .ok()
        .cloned();

    let new_contents = match existing {
        Some(Object::Reference(orig_ref)) => Object::Array(vec![
            Object::Reference(save_id),
            Object::Reference(orig_ref),
            Object::Reference(overlay_id),
        ]),
        Some(Object::Array(mut arr)) => {
            arr.insert(0, Object::Reference(save_id));
            arr.push(Object::Reference(overlay_id));
            Object::Array(arr)
        }
        _ => Object::Array(vec![Object::Reference(save_id), Object::Reference(overlay_id)]),
    };

    let page_dict = doc.get_object_mut(page_id)?.as_dict_mut()?;
    page_dict.set("Contents", new_contents);
    Ok(())
}
