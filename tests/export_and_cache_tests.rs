use std::fs;
use std::process::Command;

use image::{DynamicImage, Rgba, RgbaImage};
use sn_tui::cache::{OcrIndex, PageCache, PageCacheKey};
use sn_tui::config::load_config;
use sn_tui::export::{ExportFormat, export_path};
use sn_tui::note::fixture::{
    DEFAULT_SIGNATURE, FixtureLayer, FixtureLayerData, FixtureNotebook, FixturePage,
    build_sample_pdf, build_supernote_bytes,
};
use sn_tui::pdf::{inspect_pdf, render_pdf_page};

fn make_two_page_note() -> Vec<u8> {
    let w = 1404u32;
    let h = 1872u32;

    let mut p1 = vec![0x62u8; (w * h) as usize];
    for y in 400..460 {
        for x in 300..1100 {
            p1[(y * w + x) as usize] = 0x61;
        }
    }

    let mut p2 = vec![0x62u8; (w * h) as usize];
    for y in 800..860 {
        for x in 300..1100 {
            p2[(y * w + x) as usize] = 0x9d;
        }
    }

    build_supernote_bytes(&FixtureNotebook {
        is_mark: false,
        equipment: "A5X".to_string(),
        signature: DEFAULT_SIGNATURE.to_string(),
        pages: vec![
            FixturePage {
                page_number: 1,
                landscape: false,
                layers: vec![FixtureLayer {
                    name: "MAINLAYER".to_string(),
                    data: FixtureLayerData::RattaRle(p1),
                    visible: true,
                }],
                recognition_lines: Some(vec![(100, "Meeting Notes Alpha".to_string())]),
            },
            FixturePage {
                page_number: 2,
                landscape: false,
                layers: vec![FixtureLayer {
                    name: "MAINLAYER".to_string(),
                    data: FixtureLayerData::RattaRle(p2),
                    visible: true,
                }],
                recognition_lines: Some(vec![(120, "Action Items Beta".to_string())]),
            },
        ],
    })
}

#[test]
fn exports_note_to_devicegray_raster_pdf_png_txt_md() {
    let dir = tempfile::tempdir().expect("tempdir");
    let note_path = dir.path().join("journal.note");
    fs::write(&note_path, make_two_page_note()).expect("write note");

    // 1. Fast Raster PDF
    let pdf_out = dir.path().join("journal.pdf");
    export_path(&note_path, &pdf_out, ExportFormat::PdfRaster, 2).expect("export raster pdf");
    let info = inspect_pdf(&pdf_out).expect("inspect generated pdf");
    assert_eq!(info.page_count, 2);

    // Verify rendered PDF page 1 actually contains the dark stroke band
    let rendered = render_pdf_page(&pdf_out, 1, 72, false)
        .expect("rasterize exported pdf")
        .to_rgba8();
    let sample_x = rendered.width() / 2;
    let sample_y = (rendered.height() as f32 * (430.0 / 1872.0)) as u32;
    assert!(
        rendered.get_pixel(sample_x, sample_y)[0] < 32,
        "expected dark stroke in exported raster PDF"
    );

    // 2. PNG sequence
    let png_out = dir.path().join("pngs").join("journal.png");
    export_path(&note_path, &png_out, ExportFormat::Png, 2).expect("export pngs");
    assert!(dir.path().join("pngs").join("journal_p001.png").is_file());
    assert!(dir.path().join("pngs").join("journal_p002.png").is_file());

    // 3. TXT & Markdown
    let txt_out = dir.path().join("journal.txt");
    export_path(&note_path, &txt_out, ExportFormat::Txt, 2).expect("export txt");
    let txt = fs::read_to_string(&txt_out).expect("read txt");
    assert!(txt.contains("Meeting Notes Alpha"));
    assert!(txt.contains("Action Items Beta"));

    let md_out = dir.path().join("journal.md");
    export_path(&note_path, &md_out, ExportFormat::Md, 2).expect("export md");
    let md = fs::read_to_string(&md_out).expect("read md");
    assert!(md.contains("# journal"));
    assert!(md.contains("## Page 1"));
    assert!(md.contains("## Page 2"));
}

#[test]
fn exports_lossless_annotated_pdf_preserving_vector_text_and_mark_overlay() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pdf_path = dir.path().join("spec.pdf");
    let mark_path = dir.path().join("spec.pdf.mark");
    let out_path = dir.path().join("spec_annotated.pdf");

    fs::write(
        &pdf_path,
        build_sample_pdf(&["Page One Unannotated", "Page Two Selectable Vector Text"]),
    )
    .expect("write pdf");

    let w = 1404u32;
    let h = 1872u32;
    let mut pixels = vec![0x62u8; (w * h) as usize];
    for y in 900..960 {
        for x in 200..1200 {
            pixels[(y * w + x) as usize] = 0x61;
        }
    }
    fs::write(
        &mark_path,
        build_supernote_bytes(&FixtureNotebook {
            is_mark: true,
            equipment: "A5X".to_string(),
            signature: DEFAULT_SIGNATURE.to_string(),
            pages: vec![FixturePage {
                page_number: 2,
                landscape: false,
                layers: vec![FixtureLayer {
                    name: "MAINLAYER".to_string(),
                    data: FixtureLayerData::RattaRle(pixels),
                    visible: true,
                }],
                recognition_lines: None,
            }],
        }),
    )
    .expect("write mark");

    export_path(&pdf_path, &out_path, ExportFormat::PdfAnnotated, 2)
        .expect("export lossless annotated pdf");

    // 1. Original vector text on page 2 MUST still be selectable/extractable!
    let doc = lopdf::Document::load(&out_path).expect("reload annotated pdf");
    let extracted = doc.extract_text(&[2]).expect("extract text from page 2");
    assert!(
        extracted.contains("Page Two Selectable Vector Text"),
        "expected preserved selectable text, got: {extracted:?}"
    );

    // 2. Rasterized page 2 (WITHOUT companion mark) MUST contain the stamped black stroke!
    let rendered = render_pdf_page(&out_path, 2, 72, false)
        .expect("rasterize annotated pdf")
        .to_rgba8();
    let cx = rendered.width() / 2;
    let cy = rendered.height() / 2;
    assert!(
        rendered.get_pixel(cx, cy)[0] < 32,
        "expected stamped .mark stroke in lossless annotated PDF"
    );
}

#[test]
fn directory_export_preserves_hierarchy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("workspace");
    let sub = src.join("Work").join("2026");
    fs::create_dir_all(&sub).expect("mkdir");
    fs::write(sub.join("design.note"), make_two_page_note()).expect("write note");

    let out = dir.path().join("exports");
    let summary = export_path(&src, &out, ExportFormat::Txt, 2).expect("batch export");
    assert_eq!(summary.files_exported, 1);
    assert!(out.join("Work").join("2026").join("design.txt").is_file());
}

#[test]
fn disk_lru_cache_enforces_byte_budget_and_ocr_index_persists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let note_path = dir.path().join("indexed.note");
    fs::write(&note_path, make_two_page_note()).expect("write note");

    // Tiny 8 KB cache cap so storing multiple noisy PNGs forces LRU eviction
    let mut cache = PageCache::with_max_bytes(dir.path(), 8 * 1024).expect("init cache");
    for page in 1..=6u32 {
        let mut img = RgbaImage::new(128, 128);
        for (x, y, px) in img.enumerate_pixels_mut() {
            let v = ((x * 17 + y * 31 + page * 53) & 0xff) as u8;
            *px = Rgba([v, v, v, 255]);
        }
        let key = PageCacheKey::from_file(&note_path, page, 0xff, false);
        cache.put(key, DynamicImage::ImageRgba8(img));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert!(
        cache.disk_usage_bytes() <= 8 * 1024,
        "disk cache exceeded budget: {} bytes",
        cache.disk_usage_bytes()
    );

    let mut ocr = OcrIndex::load(dir.path());
    let text = ocr.get_or_index(&note_path).expect("ocr text").to_string();
    assert!(text.contains("Meeting Notes Alpha"));
    ocr.save().expect("save ocr index");

    let reloaded = OcrIndex::load(dir.path());
    assert_eq!(reloaded.cached_text(&note_path), Some(text.as_str()));
}

#[cfg(unix)]
#[test]
fn config_warns_on_permissive_plaintext_secret_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("config.toml");
    fs::write(
        &cfg_path,
        r#"
[profiles.private]
kind = "private_cloud"
base_url = "https://sn.local"
account = "alice@example.com"
password = "secret"
"#,
    )
    .expect("write config");

    fs::set_permissions(&cfg_path, fs::Permissions::from_mode(0o644)).expect("chmod 644");
    let loaded_warn = load_config(Some(&cfg_path)).expect("load 644 config");
    assert_eq!(loaded_warn.warnings.len(), 1);

    fs::set_permissions(&cfg_path, fs::Permissions::from_mode(0o600)).expect("chmod 600");
    let loaded_ok = load_config(Some(&cfg_path)).expect("load 600 config");
    assert!(loaded_ok.warnings.is_empty());
}

#[test]
fn cli_inspect_outputs_valid_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let note_path = dir.path().join("cli_test.note");
    fs::write(&note_path, make_two_page_note()).expect("write note");

    let bin = env!("CARGO_BIN_EXE_sn-tui");
    let out = Command::new(bin)
        .args(["inspect"])
        .arg(&note_path)
        .output()
        .expect("run sn-tui inspect");
    assert!(
        out.status.success(),
        "inspect failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("valid inspect json");
    assert_eq!(json["kind"], "supernote");
    assert_eq!(json["notebook"]["metadata"]["page_count"], 2);
}
