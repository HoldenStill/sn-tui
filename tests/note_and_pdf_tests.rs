use std::io::Cursor;

use image::Rgba;
use sn_tui::note::fixture::{
    DEFAULT_SIGNATURE, FixtureLayer, FixtureLayerData, FixtureNotebook, FixturePage,
    build_sample_pdf, build_supernote_bytes, make_solid_png,
};
use sn_tui::note::rle::decode_ratta_rle;
use sn_tui::note::{
    FileKind, extract_page_text, parse_notebook, render_page_gray, render_page_rgba,
};
use sn_tui::pdf::{inspect_pdf, render_pdf_page};

#[test]
fn parses_and_renders_a5x_note_with_png_bg_rle_strokes_and_layer_toggle() {
    let width = 1404u32;
    let height = 1872u32;

    // Light-gray PNG background
    let bg_png = make_solid_png(width, height, Rgba([240, 240, 240, 255]));

    // Transparent main layer with a horizontal black stroke and an X2 dark gray pixel
    let mut main_pixels = vec![0x62u8; (width * height) as usize];
    for x in 100..250 {
        main_pixels[(300 * width + x) as usize] = 0x61; // black
    }
    main_pixels[(301 * width + 100) as usize] = 0x9d; // X2 dark gray
    main_pixels[(302 * width + 100) as usize] = 0x44; // X2 anti-aliased gray

    let bytes = build_supernote_bytes(&FixtureNotebook {
        is_mark: false,
        equipment: "A5X".to_string(),
        signature: DEFAULT_SIGNATURE.to_string(),
        pages: vec![FixturePage {
            page_number: 1,
            landscape: false,
            layers: vec![
                FixtureLayer {
                    name: "BGLAYER".to_string(),
                    data: FixtureLayerData::Png(bg_png),
                    visible: true,
                },
                FixtureLayer {
                    name: "MAINLAYER".to_string(),
                    data: FixtureLayerData::RattaRle(main_pixels),
                    visible: true,
                },
            ],
            recognition_lines: Some(vec![
                (220, "Second line by y".to_string()),
                (80, "First line by y".to_string()),
            ]),
        }],
    });

    let mut cursor = Cursor::new(bytes);
    let nb = parse_notebook(&mut cursor).expect("parse A5X note");
    assert_eq!(nb.metadata.file_kind, FileKind::Note);
    assert_eq!(nb.metadata.base_width, 1404);
    assert_eq!(nb.metadata.base_height, 1872);
    assert_eq!(nb.pages.len(), 1);
    assert!(nb.metadata.is_realtime_recognition);

    // Full composite render
    let img = render_page_rgba(&mut cursor, &nb.pages[0], None, false).expect("render page");
    assert_eq!(img.dimensions(), (1404, 1872));
    assert_eq!(*img.get_pixel(10, 10), Rgba([240, 240, 240, 255])); // PNG bg
    assert_eq!(*img.get_pixel(150, 300), Rgba([0, 0, 0, 255])); // black stroke
    assert_eq!(*img.get_pixel(100, 301), Rgba([0x9d, 0x9d, 0x9d, 255])); // X2 dark gray
    assert_eq!(*img.get_pixel(100, 302), Rgba([0x44, 0x44, 0x44, 255])); // anti-alias

    // Hide MAINLAYER via layer_mask -> stroke disappears, PNG bg remains
    let hidden = render_page_rgba(&mut cursor, &nb.pages[0], Some(&[true, false]), false)
        .expect("render with hidden layer");
    assert_eq!(*hidden.get_pixel(150, 300), Rgba([240, 240, 240, 255]));

    // Grayscale render matches luminance
    let gray = render_page_gray(&mut cursor, &nb.pages[0], None).expect("render gray");
    assert_eq!(gray.get_pixel(150, 300)[0], 0);
    assert_eq!(gray.get_pixel(10, 10)[0], 240);

    // Recognition text sorted top-to-bottom by y
    let text = extract_page_text(&mut cursor, &nb.pages[0])
        .expect("extract text")
        .expect("has text");
    assert_eq!(text, "First line by y\nSecond line by y");
}

#[test]
fn parses_manta_n5_and_landscape_dimensions() {
    let width = 2560u32;
    let height = 1920u32;
    let pixels = vec![0x62u8; (width * height) as usize];

    let bytes = build_supernote_bytes(&FixtureNotebook {
        is_mark: false,
        equipment: "N5".to_string(),
        signature: DEFAULT_SIGNATURE.to_string(),
        pages: vec![FixturePage {
            page_number: 1,
            landscape: true,
            layers: vec![FixtureLayer {
                name: "MAINLAYER".to_string(),
                data: FixtureLayerData::RattaRle(pixels),
                visible: true,
            }],
            recognition_lines: None,
        }],
    });

    let mut cursor = Cursor::new(bytes);
    let nb = parse_notebook(&mut cursor).expect("parse Manta note");
    assert_eq!(nb.metadata.base_width, 1920);
    assert_eq!(nb.metadata.base_height, 2560);
    assert_eq!(nb.pages[0].width, 2560);
    assert_eq!(nb.pages[0].height, 1920);
    assert!(nb.pages[0].landscape);
}

#[test]
fn rle_handles_all_blank_special_length() {
    // 0xff with all_blank=true produces 0x400 (1024) pixels
    let decoded = decode_ratta_rle(&[0x65, 0xff], 32, 32, true).expect("decode blank");
    assert_eq!(decoded.len(), 1024);
    assert!(decoded.iter().all(|&b| b == 0x65));
}

#[test]
fn composites_sparse_pdf_mark_overlay_via_pdftoppm() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pdf_path = dir.path().join("paper.pdf");
    let mark_path = dir.path().join("paper.pdf.mark");

    let pdf_bytes = build_sample_pdf(&["Page 1 Clean", "Page 2 Annotated"]);
    std::fs::write(&pdf_path, pdf_bytes).expect("write pdf");

    // Create a `.mark` file annotating ONLY page 2 with a thick black band across the middle
    let w = 1404u32;
    let h = 1872u32;
    let mut pixels = vec![0x62u8; (w * h) as usize];
    for y in 900..960 {
        for x in 200..1200 {
            pixels[(y * w + x) as usize] = 0x61;
        }
    }
    let mark_bytes = build_supernote_bytes(&FixtureNotebook {
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
    });
    std::fs::write(&mark_path, mark_bytes).expect("write mark");

    let info = inspect_pdf(&pdf_path).expect("inspect pdf");
    assert_eq!(info.page_count, 2);
    assert_eq!(info.annotated_pages, vec![2]);

    // Render page 2 WITH mark overlay vs WITHOUT mark overlay (if pdftoppm available)
    if std::process::Command::new("pdftoppm").arg("-v").output().is_ok() {
        let with_mark = render_pdf_page(&pdf_path, 2, 72, true)
            .expect("render page 2 with mark")
            .to_rgba8();
        let without_mark = render_pdf_page(&pdf_path, 2, 72, false)
            .expect("render page 2 without mark")
            .to_rgba8();

        let cx = with_mark.width() / 2;
        let cy = with_mark.height() / 2;
        let cx_without = without_mark.width() / 2;
        let cy_without = without_mark.height() / 2;
        // Middle of page 2 should be black with mark, white without mark
        assert_eq!(with_mark.get_pixel(cx, cy)[0], 0);
        assert_eq!(without_mark.get_pixel(cx_without, cy_without)[0], 255);
    }
}

#[test]
fn test_render_section_10_6() {
    let pdf = std::path::Path::new("/home/holden/Documents/supernote-sync/Note/Calculus III/Section 10.6 - Alternating Series and Conditional Convergence.pdf");
    if pdf.exists() {
        let img = sn_tui::pdf::render_pdf_page(pdf, 4, 150, true).unwrap();
        img.save("/tmp/direct_p4.png").unwrap();

        // Also render raw mark page 4
        let mark_path = sn_tui::pdf::find_companion_mark(pdf).unwrap();
        let mut f = std::fs::File::open(&mark_path).unwrap();
        let nb = sn_tui::note::parse_notebook(&mut f).unwrap();
        let (_, p) = nb.find_page_by_number(4).unwrap();
        let mark_img = sn_tui::note::render_page_rgba(&mut f, p, None, true).unwrap();
        mark_img.save("/tmp/raw_mark_p4.png").unwrap();
        println!("Saved /tmp/raw_mark_p4.png");

        assert_eq!(img.width(), 2560);
        assert_eq!(img.height(), 1920);

        let out_pdf = std::path::Path::new("/tmp/sec10_6_annotated.pdf");
        sn_tui::export::export_annotated_pdf(pdf, &mark_path, out_pdf).unwrap();
        assert!(out_pdf.exists());
    }
}

#[test]
fn rejects_corrupt_files_without_panicking() {
    let mut short = Cursor::new(b"not a note".to_vec());
    assert!(parse_notebook(&mut short).is_err());

    let mut bad_sig = Cursor::new(vec![0u8; 64]);
    assert!(parse_notebook(&mut bad_sig).is_err());
}

#[test]
fn marker_codes_are_semitransparent_and_invert_works() {
    use image::{Rgba, RgbaImage};
    use sn_tui::note::rle::color_code_to_rgba;
    use sn_tui::note::invert_image;

    // Solid pens
    assert_eq!(color_code_to_rgba(0x61)[3], 255);
    assert_eq!(color_code_to_rgba(0x63)[3], 255);
    assert_eq!(color_code_to_rgba(0x9d)[3], 255);
    assert_eq!(color_code_to_rgba(0x64)[3], 255);
    assert_eq!(color_code_to_rgba(0xc9)[3], 255);

    // Semitransparent markers
    assert_eq!(color_code_to_rgba(0x66)[3], 110);
    assert_eq!(color_code_to_rgba(0x67)[3], 110);
    assert_eq!(color_code_to_rgba(0x9e)[3], 110);
    assert_eq!(color_code_to_rgba(0x68)[3], 110);
    assert_eq!(color_code_to_rgba(0xca)[3], 110);

    // Invert image test
    let mut img = RgbaImage::new(2, 2);
    img.put_pixel(0, 0, Rgba([255, 255, 255, 255])); // white
    img.put_pixel(1, 0, Rgba([0, 0, 0, 255]));       // black
    img.put_pixel(0, 1, Rgba([0, 0, 0, 0]));         // transparent
    img.put_pixel(1, 1, Rgba([100, 150, 200, 110])); // marker

    invert_image(&mut img);

    assert_eq!(img.get_pixel(0, 0), &Rgba([0, 0, 0, 255]));
    assert_eq!(img.get_pixel(1, 0), &Rgba([255, 255, 255, 255]));
    assert_eq!(img.get_pixel(0, 1), &Rgba([0, 0, 0, 0])); // transparent unchanged
    assert_eq!(img.get_pixel(1, 1), &Rgba([155, 105, 55, 110])); // alpha preserved
}
