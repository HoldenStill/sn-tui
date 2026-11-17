//! Terminal initialization, safe RAII restoration, graphics protocol auto-detection,
//! live filesystem watching, background sync/upload dispatch, and main interactive TUI event loop.

pub mod app;
pub mod tree;
pub mod ui;
pub mod worker;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use clap::ValueEnum;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui_image::picker::Picker;

use crate::config::{AppConfig, default_cache_dir};
use crate::export::ExportFormat;
use crate::sync::watcher::WorkspaceWatcher;
use crate::sync::{pull_sync, resolve_profile, upload_file};
use app::{App, SyncTrigger};
use worker::WorkerRequest;

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

enum SyncUiEvent {
    Log(String),
    SyncedFiles(Vec<PathBuf>),
}

pub fn run_tui(config: AppConfig, startup_warnings: Vec<String>) -> Result<()> {
    // Query terminal graphics protocol BEFORE entering raw mode / event loop!
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal: Terminal<CrosstermBackend<Stdout>> = Terminal::new(backend)?;
    terminal.clear()?;

    let cache_root: PathBuf = default_cache_dir();
    let mut watcher = WorkspaceWatcher::start(&config.workspace_dir).ok();
    let mut app = App::new(config, cache_root.clone(), picker, startup_warnings);

    if watcher.is_some() {
        app.push_log("Watching workspace for live file updates");
    }

    let (sync_tx, sync_rx) = mpsc::channel::<SyncUiEvent>();

    while !app.should_quit {
        app.drain_worker_events();

        // Drain live filesystem watcher events
        if let Some(w) = watcher.as_mut() {
            for changed in w.poll_changed() {
                app.notify_fs_change(&changed);
                app.push_log(format!(
                    "Detected change: {}",
                    changed
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| changed.display().to_string())
                ));
                if let Some(fmt_str) = &app.config.auto_export_format
                    && let Ok(fmt) = ExportFormat::from_str(fmt_str, true)
                    && changed.is_file()
                    && changed
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("note"))
                {
                    let rel = changed
                        .strip_prefix(&app.config.workspace_dir)
                        .unwrap_or(&changed);
                    let out = app
                        .config
                        .export_dir
                        .join(rel)
                        .with_extension(fmt.default_extension());
                    let _ = app.worker.tx.send(WorkerRequest::Export {
                        input: changed,
                        output: out,
                        format: fmt,
                        workers: app.config.max_workers,
                    });
                }
            }
        }

        // Drain background sync/upload logs and refresh tree
        while let Ok(evt) = sync_rx.try_recv() {
            match evt {
                SyncUiEvent::Log(msg) => app.push_log(msg),
                SyncUiEvent::SyncedFiles(paths) => {
                    for p in paths {
                        app.tree.synced_paths.insert(p.clone());
                        app.notify_fs_change(&p);
                    }
                }
            }
        }

        // Dispatch sync/upload triggers onto background OS threads
        let triggers = std::mem::take(&mut app.pending_sync_triggers);
        for trigger in triggers {
            match trigger {
                SyncTrigger::PullSync { profile } => {
                    match resolve_profile(&app.config, profile.as_deref()) {
                        Ok((name, prof)) => {
                            let prof = prof.clone();
                            let local_root = app.config.workspace_dir.clone();
                            let cache_dir = cache_root.clone();
                            let tx = sync_tx.clone();
                            thread::spawn(move || {
                                let res = pull_sync(
                                    &name,
                                    &prof,
                                    &local_root,
                                    &cache_dir,
                                    false,
                                    |msg| {
                                        let _ = tx.send(SyncUiEvent::Log(msg.to_string()));
                                    },
                                );
                                match res {
                                    Ok(rep) => {
                                        let _ = tx.send(SyncUiEvent::Log(format!(
                                            "Sync `{name}` done: {} pulled, {} up-to-date, {} failed",
                                            rep.downloaded, rep.skipped, rep.failed
                                        )));
                                        if !rep.downloaded_paths.is_empty() {
                                            let _ = tx.send(SyncUiEvent::SyncedFiles(
                                                rep.downloaded_paths,
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        let _ = tx.send(SyncUiEvent::Log(format!(
                                            "Sync `{name}` error: {e:#}"
                                        )));
                                    }
                                }
                            });
                        }
                        Err(e) => app.push_log(format!("Sync config error: {e:#}")),
                    }
                }
                SyncTrigger::UploadFile {
                    local_file,
                    remote_dest,
                    profile,
                } => match resolve_profile(&app.config, profile.as_deref()) {
                    Ok((name, prof)) => {
                        let prof = prof.clone();
                        let tx = sync_tx.clone();
                        thread::spawn(move || {
                            match upload_file(&prof, &local_file, &remote_dest) {
                                Ok(()) => {
                                    let _ = tx.send(SyncUiEvent::Log(format!(
                                        "Uploaded {} -> {} ({name})",
                                        local_file.display(),
                                        remote_dest
                                    )));
                                }
                                Err(e) => {
                                    let _ = tx.send(SyncUiEvent::Log(format!(
                                        "Upload error ({name}): {e:#}"
                                    )));
                                }
                            }
                        });
                    }
                    Err(e) => app.push_log(format!("Upload config error: {e:#}")),
                },
            }
        }

        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }
    }

    Ok(())
}
