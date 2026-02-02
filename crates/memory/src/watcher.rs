/// Real-time file watching for memory sync using notify-debouncer-full.
use std::path::PathBuf;

use {
    anyhow::Result,
    notify_debouncer_full::{
        DebounceEventResult, Debouncer, RecommendedCache, new_debouncer, notify::RecursiveMode,
    },
    tokio::sync::mpsc,
    tracing::{debug, info, warn},
};

/// Events emitted by the file watcher.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
}

/// Watches directories for markdown file changes with debouncing.
pub struct MemoryFileWatcher {
    debouncer: Debouncer<notify_debouncer_full::notify::RecommendedWatcher, RecommendedCache>,
}

impl MemoryFileWatcher {
    /// Start watching the given directories. Returns the watcher and a receiver for events.
    pub fn start(dirs: Vec<PathBuf>) -> Result<(Self, mpsc::UnboundedReceiver<WatchEvent>)> {
        let (tx, rx) = mpsc::unbounded_channel();

        let debouncer = new_debouncer(
            std::time::Duration::from_millis(1500),
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        for event in events {
                            for path in &event.paths {
                                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                                if ext != "md" && ext != "markdown" {
                                    continue;
                                }

                                use notify_debouncer_full::notify::EventKind;
                                let watch_event = match event.kind {
                                    EventKind::Create(_) => WatchEvent::Created(path.clone()),
                                    EventKind::Modify(_) => WatchEvent::Modified(path.clone()),
                                    EventKind::Remove(_) => WatchEvent::Removed(path.clone()),
                                    _ => continue,
                                };

                                debug!(path = %path.display(), "file watcher event");
                                if tx.send(watch_event).is_err() {
                                    return; // receiver dropped
                                }
                            }
                        }
                    },
                    Err(errors) => {
                        for e in errors {
                            warn!(error = %e, "file watcher error");
                        }
                    },
                }
            },
        )?;

        let mut watcher = Self { debouncer };

        for dir in &dirs {
            if dir.exists() {
                watcher.debouncer.watch(dir, RecursiveMode::Recursive)?;
                info!(dir = %dir.display(), "file watcher: watching directory");
            }
        }

        Ok((watcher, rx))
    }
}
