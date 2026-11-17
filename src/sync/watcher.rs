//! Live local workspace filesystem watcher (`notify`) with simple debouncing for
//! `.note`, `.mark`, and `.pdf` updates.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(200);

pub struct WorkspaceWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<PathBuf>,
    last_seen: HashMap<PathBuf, Instant>,
}

impl WorkspaceWatcher {
    pub fn start(root: &Path) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<Event>| {
                let Ok(event) = res else {
                    return;
                };
                if !matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    return;
                }
                for path in event.paths {
                    if is_watched_supernote_file(&path) {
                        let _ = tx.send(path);
                    }
                }
            })?;

        if root.exists() {
            watcher.watch(root, RecursiveMode::Recursive)?;
        }

        Ok(Self {
            _watcher: watcher,
            rx,
            last_seen: HashMap::new(),
        })
    }

    /// Drains debounced changed paths ready to be processed by the TUI or CLI.
    pub fn poll_changed(&mut self) -> Vec<PathBuf> {
        let now = Instant::now();
        let mut out = Vec::new();
        while let Ok(path) = self.rx.try_recv() {
            let emit = match self.last_seen.get(&path) {
                Some(prev) => now.duration_since(*prev) >= DEBOUNCE_WINDOW,
                None => true,
            };
            if emit {
                self.last_seen.insert(path.clone(), now);
                out.push(path);
            }
        }
        out
    }
}

fn is_watched_supernote_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    !lower.ends_with(".tmp")
        && !lower.ends_with(".sn_tmp")
        && (lower.ends_with(".note") || lower.ends_with(".mark") || lower.ends_with(".pdf"))
}
