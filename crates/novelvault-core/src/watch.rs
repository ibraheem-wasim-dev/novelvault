//! Folder watching with debounce and coalescing.
//!
//! Editors do not produce one tidy "modified" event per save. Word writes a temp
//! file, then renames it over the target, so the watcher sees a burst of
//! create/rename events. [`Coalescer`] reduces a burst to a single meaningful event
//! per path, which keeps the Inbox free of noise and stops duplicate versions.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::db::Vault;
use crate::model::{EventKind, VaultEvent};
use crate::util::now_iso;
use crate::Result;

#[derive(Debug, Clone)]
struct Pending {
    kind: EventKind,
    last_at: Instant,
}

#[derive(Debug)]
pub struct Coalescer {
    quiet: Duration,
    pending: HashMap<String, Pending>,
}

impl Coalescer {
    pub fn new(quiet_ms: u64) -> Self {
        Coalescer {
            quiet: Duration::from_millis(quiet_ms),
            pending: HashMap::new(),
        }
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Fold one raw filesystem observation into the pending set.
    pub fn push(&mut self, kind: EventKind, rel_path: &str, now: Instant) {
        let merged = match self.pending.get(rel_path) {
            None => Some(kind),

            // create + remove = an editor's temp file that never existed; drop it
            Some(Pending { kind: EventKind::Created, .. }) if kind == EventKind::Removed => None,

            // remove + create = rewritten in place
            Some(Pending { kind: EventKind::Removed, .. }) if kind == EventKind::Created => {
                Some(EventKind::Modified)
            }

            // remove + modify = rewritten in place
            Some(Pending { kind: EventKind::Removed, .. }) if kind == EventKind::Modified => {
                Some(EventKind::Modified)
            }

            // a save burst stays a single create
            Some(Pending { kind: EventKind::Created, .. }) => Some(EventKind::Created),

            // anything else that ends in a removal is a removal
            Some(_) if kind == EventKind::Removed => Some(EventKind::Removed),

            Some(_) => Some(EventKind::Modified),
        };

        match merged {
            None => {
                self.pending.remove(rel_path);
            }
            Some(kind) => {
                self.pending.insert(
                    rel_path.to_string(),
                    Pending { kind, last_at: now },
                );
            }
        }
    }

    /// Events whose quiet window has elapsed.
    pub fn drain_due(&mut self, now: Instant) -> Vec<VaultEvent> {
        let quiet = self.quiet;
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now.duration_since(p.last_at) >= quiet)
            .map(|(path, _)| path.clone())
            .collect();
        self.take(due)
    }

    /// Everything still pending, regardless of the quiet window (used on shutdown).
    pub fn flush_all(&mut self) -> Vec<VaultEvent> {
        let all: Vec<String> = self.pending.keys().cloned().collect();
        self.take(all)
    }

    fn take(&mut self, paths: Vec<String>) -> Vec<VaultEvent> {
        let mut out = Vec::new();
        for path in paths {
            if let Some(pending) = self.pending.remove(&path) {
                out.push(VaultEvent {
                    kind: pending.kind,
                    rel_path: path,
                    at: now_iso(),
                });
            }
        }
        out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        out
    }
}

/// Map a `notify` event to zero or more `(kind, path)` observations.
pub fn translate(event: &notify::Event, root: &Path) -> Vec<(EventKind, String)> {
    use notify::event::{EventKind as K, RenameMode};

    let kind = match &event.kind {
        K::Create(_) => Some(EventKind::Created),
        K::Modify(notify::event::ModifyKind::Name(RenameMode::Both)) => None, // handled below
        K::Modify(notify::event::ModifyKind::Name(_)) => Some(EventKind::Created),
        K::Modify(_) => Some(EventKind::Modified),
        K::Remove(_) => Some(EventKind::Removed),
        K::Access(_) => None,
        _ => Some(EventKind::Modified),
    };

    let mut out = Vec::new();
    if matches!(event.kind, K::Modify(notify::event::ModifyKind::Name(RenameMode::Both))) {
        // [from, to] — the destination is what matters; the source simply disappeared.
        for (index, path) in event.paths.iter().enumerate() {
            let rel = crate::scan::rel_path(root, path);
            let kind = if index == 0 { EventKind::Removed } else { EventKind::Created };
            out.push((kind, rel));
        }
        return out;
    }

    if let Some(kind) = kind {
        for path in &event.paths {
            out.push((kind, crate::scan::rel_path(root, path)));
        }
    }
    out
}

/// Watch a novel folder until `stop_after` elapses (None = forever).
///
/// The callback receives each coalesced batch; the CLI uses it to apply the events to
/// the vault and print what happened. Files are only ever read.
pub fn run_watch<F>(root: &Path, quiet_ms: u64, stop_after: Option<Duration>, mut on_batch: F) -> Result<()>
where
    F: FnMut(&Vault, &[VaultEvent]) -> Result<()>,
{
    use notify::{RecursiveMode, Watcher};

    let vault = Vault::open(root)?;
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| crate::VaultError::Other(format!("watcher: {e}")))?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(|e| crate::VaultError::Other(format!("watch {root:?}: {e}")))?;

    // The app's own writes inside .novelvault/ (the WAL, extracted text, snapshots)
    // are filesystem events too. They must never re-enter the pipeline, or the vault
    // would index itself and the watcher would wake on its own updates forever.
    let globs = crate::scan::ignore_globs(&vault);

    let mut coalescer = Coalescer::new(quiet_ms);
    let deadline = stop_after.map(|d| Instant::now() + d);
    let tick = Duration::from_millis(100);

    loop {
        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                let remaining = coalescer.flush_all();
                if !remaining.is_empty() {
                    on_batch(&vault, &remaining)?;
                }
                break;
            }
        }

        match rx.recv_timeout(tick) {
            Ok(Ok(event)) => {
                for (kind, rel) in translate(&event, root) {
                    if crate::scan::is_ignored(&rel, &globs) {
                        continue;
                    }
                    coalescer.push(kind, &rel, Instant::now());
                }
            }
            Ok(Err(_)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let due = coalescer.drain_due(Instant::now());
        if !due.is_empty() {
            on_batch(&vault, &due)?;
        }
    }

    Ok(())
}
