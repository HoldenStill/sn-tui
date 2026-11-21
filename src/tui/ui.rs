//! 3-pane Yazi-style `ratatui` layout, inline `StatefulImage` canvas, inspector, and modals.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::StatefulImage;

use crate::tui::app::{ActiveDoc, App, ExportScope, Modal, ViewLayout};
use crate::tui::tree::EntryKind;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let root_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(frame.area());

    let main_area = root_chunks[0];
    let status_area = root_chunks[1];

    match app.view_layout {
        ViewLayout::Standard3Pane => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(26),
                    Constraint::Percentage(50),
                    Constraint::Percentage(24),
                ])
                .split(main_area);

            draw_left_tree(frame, app, panes[0]);
            draw_center_canvas(frame, app, panes[1]);
            draw_right_inspector(frame, app, panes[2]);
        }
        ViewLayout::WideReader => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(25),
                    Constraint::Percentage(75),
                ])
                .split(main_area);

            draw_left_tree(frame, app, panes[0]);
            draw_center_canvas(frame, app, panes[1]);
        }
        ViewLayout::BottomInspector => {
            let horiz = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(25),
                    Constraint::Percentage(75),
                ])
                .split(main_area);

            draw_left_tree(frame, app, horiz[0]);

            let right_stack = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(75),
                    Constraint::Percentage(25),
                ])
                .split(horiz[1]);

            draw_center_canvas(frame, app, right_stack[0]);
            draw_right_inspector(frame, app, right_stack[1]);
        }
        ViewLayout::FullFocus => {
            draw_center_canvas(frame, app, main_area);
        }
    }

    draw_status_bar(frame, app, status_area);

    match &app.modal {
        Modal::None => {}
        Modal::Search => draw_search_popup(frame, app),
        Modal::JumpPage(buf) => draw_jump_popup(frame, buf, app.total_pages),
        Modal::Export(state) => draw_export_popup(frame, state, app.supernote_tool.is_available()),
        Modal::Upload(state) => draw_upload_popup(frame, state),
        Modal::Help => draw_help_popup(frame),
    }
}

fn draw_left_tree(frame: &mut Frame, app: &App, area: Rect) {
    let title = if app.tree.filter.is_empty() {
        format!(" Library ({}) ", app.tree.root.display())
    } else {
        format!(" Filter: /{}/ ({}) ", app.tree.filter, app.tree.rows.len())
    };

    let items: Vec<ListItem> = if app.tree.rows.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "  (No .note/.pdf files found)",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        app.tree
            .rows
            .iter()
            .map(|row| {
                let indent = "  ".repeat(row.depth);
                let icon = match row.kind {
                    EntryKind::Directory => {
                        if row.expanded {
                            "▾ "
                        } else {
                            "▸ "
                        }
                    }
                    EntryKind::Note => "N ",
                    EntryKind::Pdf => "P ",
                    EntryKind::StandaloneMark => "M ",
                };
                let mut spans = vec![
                    Span::raw(indent),
                    Span::styled(
                        icon,
                        Style::default().fg(match row.kind {
                            EntryKind::Directory => Color::Cyan,
                            EntryKind::Note => Color::LightBlue,
                            EntryKind::Pdf => Color::Yellow,
                            EntryKind::StandaloneMark => Color::Magenta,
                        }),
                    ),
                    Span::raw(&row.name),
                ];
                if row.has_mark && row.kind == EntryKind::Pdf {
                    spans.push(Span::styled(" [+Mark]", Style::default().fg(Color::Green)));
                }
                if row.is_exported {
                    spans.push(Span::styled(" [Exp]", Style::default().fg(Color::Cyan)));
                }
                if row.is_synced {
                    spans.push(Span::styled(" [Sync]", Style::default().fg(Color::Blue)));
                }
                if let Some(p) = row.page_count {
                    spans.push(Span::styled(
                        format!(" ({p}p)"),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect()
    };

    let mut state = ListState::default();
    if !app.tree.rows.is_empty() {
        state.select(Some(app.selected_idx));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_center_canvas(frame: &mut Frame, app: &mut App, area: Rect) {
    let filename = app
        .active_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Preview".to_string());

    let page_badge = if app.total_pages > 0 {
        format!(" Page {}/{}", app.current_page, app.total_pages)
    } else {
        String::new()
    };
    let zoom_badge = if app.zoom > 1.05 {
        format!(
            " [Zoom {:.1}x | Pan X:{:.0}% Y:{:.0}% | ↑↓←→ Pan, 0 Reset]",
            app.zoom,
            app.pan_x * 100.0,
            app.pan_y * 100.0
        )
    } else {
        String::new()
    };
    let dark_badge = if app.invert_colors { " [Dark]" } else { "" };
    let layout_badge = if app.view_layout != ViewLayout::Standard3Pane {
        format!(" [{}]", app.view_layout.label())
    } else {
        String::new()
    };
    let mark_badge = if matches!(app.active_doc, ActiveDoc::Pdf(ref info) if info.companion_mark.is_some())
    {
        if app.show_mark_overlay {
            " [.mark ON]"
        } else {
            " [.mark OFF]"
        }
    } else {
        ""
    };

    let block = Block::default()
        .title(format!(" {filename}{page_badge}{zoom_badge}{mark_badge}{dark_badge}{layout_badge} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(proto) = app.image_protocol.as_mut() {
        frame.render_stateful_widget(StatefulImage::default(), inner, proto);
    } else {
        let msg = if app.loading_page {
            "Rendering page..."
        } else if matches!(app.active_doc, ActiveDoc::None) {
            "Select a .note or .pdf file to preview.\n\nPress ? for keybindings, / to search, s to sync."
        } else {
            "No renderable page available."
        };
        let placeholder = Paragraph::new(msg)
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true });
        frame.render_widget(placeholder, inner);
    }
}

fn draw_right_inspector(frame: &mut Frame, app: &App, area: Rect) {
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(area);

    // Top: Metadata + Layers + OCR preview
    let mut lines: Vec<Line> = Vec::new();
    match &app.active_doc {
        ActiveDoc::Note(nb) => {
            let m = &nb.metadata;
            lines.push(kv_line("Type", format!("{:?}", m.file_kind)));
            lines.push(kv_line("Device", m.device_label.clone()));
            lines.push(kv_line("Format", m.signature.clone()));
            lines.push(kv_line("Canvas", format!("{}×{}", m.base_width, m.base_height)));
            lines.push(kv_line("Pages", m.page_count.to_string()));
            lines.push(kv_line(
                "OCR",
                if m.is_realtime_recognition { "Yes" } else { "No" }.to_string(),
            ));

            let page_idx = (app.current_page as usize).saturating_sub(1);
            if let Some(page) = nb.pages.get(page_idx) {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Layers (keys 1-5):",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));
                for (i, layer) in page.layers.iter().enumerate() {
                    let vis = app.layer_visibility.get(i).copied().unwrap_or(layer.visible);
                    let mark = if vis { "[x]" } else { "[ ]" };
                    lines.push(Line::from(format!(
                        " {} {} {} ({})",
                        i + 1,
                        mark,
                        layer.key,
                        layer.protocol
                    )));
                }
            }

            if let Some(ocr) = &app.active_ocr_text {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Recognized Text:",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )));
                for l in ocr.lines().take(12) {
                    lines.push(Line::from(l.to_string()));
                }
            }
        }
        ActiveDoc::Pdf(info) => {
            lines.push(kv_line("Type", "PDF Document".to_string()));
            lines.push(kv_line("Pages", info.page_count.to_string()));
            lines.push(kv_line(
                ".mark",
                if info.companion_mark.is_some() {
                    format!("Yes ({} annotated)", info.annotated_pages.len())
                } else {
                    "None".to_string()
                },
            ));
            if !info.annotated_pages.is_empty() {
                let list: Vec<String> = info
                    .annotated_pages
                    .iter()
                    .take(12)
                    .map(u32::to_string)
                    .collect();
                lines.push(kv_line("Marked", list.join(", ")));
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "1: [{}] .mark Overlay",
                    if app.show_mark_overlay { "x" } else { " " }
                )));
            }
        }
        ActiveDoc::None => {
            lines.push(Line::from(Span::styled(
                "No file selected",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(kv_line(
                "Export Dir",
                app.config.export_dir.display().to_string(),
            ));
            lines.push(kv_line(
                "Profiles",
                if app.config.profiles.is_empty() {
                    "none".to_string()
                } else {
                    app.config
                        .profiles
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            ));
        }
    }

    let inspector = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Inspector ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(inspector, right_chunks[0]);

    // Bottom: Activity / Job / Sync Log
    let log_lines: Vec<Line> = app
        .logs
        .iter()
        .rev()
        .take(right_chunks[1].height.saturating_sub(2) as usize)
        .rev()
        .map(|s| Line::from(s.as_str()))
        .collect();
    let logs_widget = Paragraph::new(log_lines)
        .block(
            Block::default()
                .title(" Activity Log ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(logs_widget, right_chunks[1]);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let profile = app
        .config
        .default_profile
        .as_deref()
        .unwrap_or("no-profile");
    let cache_mb = (app.cache_bytes as f64) / (1024.0 * 1024.0);
    let layout_name = app.view_layout.label();
    let dark_str = if app.invert_colors { " | dark:ON" } else { "" };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.mode_label()),
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " profile:{profile} | view:{layout_name}{dark_str} | cache:{cache_mb:.1}/{}MB | ",
            app.config.cache_max_mb
        )),
        Span::styled(
            "v:view  f:focus  i:dark  j/k:nav  ↑↓←→:pan  n/p:page  +/-/0:zoom  e:export  s:sync  u:upload  ?:help  q:quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_search_popup(frame: &mut Frame, app: &App) {
    let area = centered_rect(50, 3, frame.area());
    frame.render_widget(Clear, area);
    let p = Paragraph::new(format!("/{}", app.tree.filter)).block(
        Block::default()
            .title(" Fuzzy Search (Filename & OCR Text) — Enter:apply Esc:clear ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(p, area);
}

fn draw_jump_popup(frame: &mut Frame, buf: &str, total: u32) {
    let area = centered_rect(36, 3, frame.area());
    frame.render_widget(Clear, area);
    let p = Paragraph::new(format!("Page (1..{total}): {buf}")).block(
        Block::default()
            .title(" Jump to Page ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(p, area);
}

fn draw_export_popup(
    frame: &mut Frame,
    state: &crate::tui::app::ExportModalState,
    has_sntool: bool,
) {
    let area = centered_rect(64, 15, frame.area());
    frame.render_widget(Clear, area);

    let scope_str = match state.scope {
        ExportScope::SelectedFile => "Selected File",
        ExportScope::WorkspaceFolder => "Entire Workspace Folder",
    };

    let mut items = Vec::with_capacity(state.formats.len());
    for (i, fmt) in state.formats.iter().enumerate() {
        let marker = if i == state.format_idx { "▶ " } else { "  " };
        let avail = if fmt.requires_supernote_tool() && !has_sntool {
            " [requires supernote-tool]"
        } else {
            ""
        };
        let style = if i == state.format_idx {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if !avail.is_empty() {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        items.push(Line::from(Span::styled(
            format!("{marker}{}{avail}", fmt.display_name()),
            style,
        )));
    }

    let mut all = vec![
        Line::from(format!("Scope [Tab]: {scope_str}")),
        Line::from(format!("Output Dir:  {}", state.dest_dir.display())),
        Line::from(""),
    ];
    all.extend(items);
    all.push(Line::from(""));
    all.push(Line::from(Span::styled(
        "j/k: format  Tab: scope  Enter: export  Esc: cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let p = Paragraph::new(all).block(
        Block::default()
            .title(" Export ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green)),
    );
    frame.render_widget(p, area);
}

fn draw_upload_popup(frame: &mut Frame, state: &crate::tui::app::UploadModalState) {
    let area = centered_rect(60, 7, frame.area());
    frame.render_widget(Clear, area);
    let lines = vec![
        Line::from(format!("Local File:  {}", state.local_file.display())),
        Line::from(format!("Remote Dest: {}", state.remote_dest)),
        Line::from(""),
        Line::from(Span::styled(
            "Type remote folder path (e.g. /Document or /MyStyle), Enter to upload, Esc to cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .title(" Explicit Upload to Cloud / Private Cloud ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
    );
    frame.render_widget(p, area);
}

fn draw_help_popup(frame: &mut Frame) {
    let area = centered_rect(66, 21, frame.area());
    frame.render_widget(Clear, area);
    let lines = vec![
        Line::from("j / k          Move selection in library tree (G: end, Home: start)"),
        Line::from("↑ / ↓ / ← / →  Pan zoomed canvas (or navigate tree/folders when 1.0x)"),
        Line::from("h / l, Enter   Collapse / expand directory"),
        Line::from("n / p, J / K   Next / previous page"),
        Line::from("g              Jump to page number"),
        Line::from("v              Cycle layout (3-Pane -> Wide -> Bottom -> Focus)"),
        Line::from("f              Toggle Full Focus / Fullscreen canvas"),
        Line::from("i              Toggle Dark / Invert color mode"),
        Line::from("1 – 5          Toggle .note layers (1 toggles PDF .mark overlay)"),
        Line::from("+ / - / 0      Zoom in / zoom out / reset zoom"),
        Line::from("/              Fuzzy search filenames & OCR text"),
        Line::from("e              Open Export modal"),
        Line::from("s              Pull-sync active Cloud / Private Cloud / WebDAV profile"),
        Line::from("u              Upload selected local file to remote folder"),
        Line::from("r              Refresh tree & invalidate active cache"),
        Line::from("?              Toggle this help overlay"),
        Line::from("q / Ctrl+C     Quit cleanly"),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .title(" sn-tui Keybindings (Esc or ? to close) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(p, area);
}

fn kv_line(key: &str, val: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key}: "), Style::default().fg(Color::DarkGray)),
        Span::raw(val),
    ])
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let w = (area.width * percent_x / 100).max(24).min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}
