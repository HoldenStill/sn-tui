use std::fs;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui_image::picker::Picker;
use sn_tui::config::AppConfig;
use sn_tui::note::fixture::{
    DEFAULT_SIGNATURE, FixtureLayer, FixtureLayerData, FixtureNotebook, FixturePage,
    build_sample_pdf, build_supernote_bytes,
};
use sn_tui::tui::app::{App, Modal};
use sn_tui::tui::tree::EntryKind;
use sn_tui::tui::ui;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn tui_renders_three_panes_groups_pdf_mark_and_handles_navigation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("Supernote");
    let cache = dir.path().join("cache");
    fs::create_dir_all(&ws).expect("mkdir ws");

    // 1. Add a 2-page .note file with OCR text
    let w = 1404u32;
    let h = 1872u32;
    let pixels = vec![0x62u8; (w * h) as usize];
    let note_bytes = build_supernote_bytes(&FixtureNotebook {
        is_mark: false,
        equipment: "A5X".to_string(),
        signature: DEFAULT_SIGNATURE.to_string(),
        pages: vec![
            FixturePage {
                page_number: 1,
                landscape: false,
                layers: vec![FixtureLayer {
                    name: "MAINLAYER".to_string(),
                    data: FixtureLayerData::RattaRle(pixels.clone()),
                    visible: true,
                }],
                recognition_lines: Some(vec![(50, "Quantum Architecture".to_string())]),
            },
            FixturePage {
                page_number: 2,
                landscape: false,
                layers: vec![FixtureLayer {
                    name: "MAINLAYER".to_string(),
                    data: FixtureLayerData::RattaRle(pixels.clone()),
                    visible: true,
                }],
                recognition_lines: None,
            },
        ],
    });
    fs::write(ws.join("alpha.note"), note_bytes).expect("write note");

    // 2. Add a PDF + companion .pdf.mark (sidecar should group onto the PDF row)
    fs::write(ws.join("beta.pdf"), build_sample_pdf(&["PDF Page 1"])).expect("write pdf");
    let mark_bytes = build_supernote_bytes(&FixtureNotebook {
        is_mark: true,
        equipment: "A5X".to_string(),
        signature: DEFAULT_SIGNATURE.to_string(),
        pages: vec![FixturePage {
            page_number: 1,
            landscape: false,
            layers: vec![FixtureLayer {
                name: "MAINLAYER".to_string(),
                data: FixtureLayerData::RattaRle(pixels),
                visible: true,
            }],
            recognition_lines: None,
        }],
    });
    fs::write(ws.join("beta.pdf.mark"), mark_bytes).expect("write mark");

    let cfg = AppConfig {
        workspace_dir: ws.clone(),
        export_dir: dir.path().join("Exports"),
        ..AppConfig::default()
    };

    let mut app = App::new(cfg, cache, Picker::halfblocks(), vec![]);

    // Only 2 rows: alpha.note and beta.pdf (beta.pdf.mark grouped onto beta.pdf!)
    assert_eq!(app.tree.rows.len(), 2);
    assert_eq!(app.tree.rows[0].kind, EntryKind::Note);
    assert_eq!(app.tree.rows[1].kind, EntryKind::Pdf);
    assert!(app.tree.rows[1].has_mark);

    // Wait briefly for background worker to render page 1 of alpha.note
    for _ in 0..40 {
        app.drain_worker_events();
        if app.raw_image.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(app.raw_image.is_some(), "expected background render of alpha.note");
    assert_eq!(app.total_pages, 2);

    // Page turn next/prev
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.current_page, 2);
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.current_page, 1);

    // Render full TUI frame into an in-memory TestBackend (normal + modals)
    let backend = TestBackend::new(120, 36);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|f| ui::draw(f, &mut app)).expect("draw normal");

    // Open & render Export modal
    app.handle_key(key(KeyCode::Char('e')));
    assert!(matches!(app.modal, Modal::Export(_)));
    terminal.draw(|f| ui::draw(f, &mut app)).expect("draw export modal");
    app.handle_key(key(KeyCode::Esc));
    assert!(matches!(app.modal, Modal::None));

    // Open & render Help overlay
    app.handle_key(key(KeyCode::Char('?')));
    assert!(matches!(app.modal, Modal::Help));
    terminal.draw(|f| ui::draw(f, &mut app)).expect("draw help modal");
    app.handle_key(key(KeyCode::Esc));

    // Fuzzy search by OCR text ("quantum" matches alpha.note)
    app.handle_key(key(KeyCode::Char('/')));
    for ch in "quantum".chars() {
        app.handle_key(key(KeyCode::Char(ch)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.tree.rows.len(), 1);
    assert_eq!(app.tree.rows[0].name, "alpha.note");

    // Quit cleanly
    app.handle_key(key(KeyCode::Char('q')));
    assert!(app.should_quit);
}
