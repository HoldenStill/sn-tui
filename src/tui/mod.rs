//! Terminal initialization, safe RAII restoration, graphics protocol auto-detection,
//! and main interactive TUI event loop.

pub mod app;
pub mod tree;
pub mod ui;
pub mod worker;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui_image::picker::Picker;

use crate::config::{AppConfig, default_cache_dir};
use app::{App, SyncTrigger};

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

pub fn run_tui(config: AppConfig, startup_warnings: Vec<String>) -> Result<()> {
    // Query terminal graphics protocol BEFORE entering raw mode / event loop!
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal: Terminal<CrosstermBackend<Stdout>> = Terminal::new(backend)?;
    terminal.clear()?;

    let cache_root: PathBuf = default_cache_dir();
    let mut app = App::new(config, cache_root, picker, startup_warnings);

    while !app.should_quit {
        app.drain_worker_events();

        // Sync triggers will be dispatched to the native sync engine in Milestone 5
        let triggers = std::mem::take(&mut app.pending_sync_triggers);
        for trigger in triggers {
            match trigger {
                SyncTrigger::PullSync { .. } => {
                    app.push_log("Sync backend will be wired in Milestone 5.");
                }
                SyncTrigger::UploadFile { .. } => {
                    app.push_log("Upload backend will be wired in Milestone 5.");
                }
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
