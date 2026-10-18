//! The library: the list of novels this app knows about.
//!
//! One person can be building several novels at once. The library remembers where each
//! lives and gives a one-glance summary, so the app can switch between them. It stores
//! **only paths and titles** in the app's own data folder — never novel content — and
//! "forgetting" a novel removes the list entry only; the folder is never touched (R2).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::db::{db_path, vault_dir};
use crate::util::now_iso;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryEntry {
    pub path: String,
    pub title: String,
    pub added_at: String,
    pub last_opened_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NovelSummary {
    pub path: String,
    pub title: String,
    pub last_opened_at: String,
    /// false when the folder has moved or been deleted — the entry is kept so it can be re-found.
    pub exists: bool,
    pub has_vault: bool,
    pub files: i64,
    pub current_files: i64,
    pub arcs: i64,
    pub chapters: i64,
    pub characters: i64,
    pub canon_items: i64,
    pub open_conflicts: i64,
    pub pending_ai_suggestions: i64,
    pub last_activity: Option<String>,
}

fn file(dir: &Path) -> PathBuf {
    dir.join("library.json")
}

fn norm(path: &Path) -> String {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string()
}

pub fn load(dir: &Path) -> Result<Vec<LibraryEntry>> {
    match std::fs::read_to_string(file(dir)) {
        Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
        Err(_) => Ok(Vec::new()),
    }
}

fn save(dir: &Path, entries: &[LibraryEntry]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("library.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(entries)?)?;
    std::fs::rename(tmp, file(dir))?;
    Ok(())
}

/// Add a novel, or mark it as just opened.
pub fn register(dir: &Path, novel_root: &Path, title: &str) -> Result<()> {
    let mut entries = load(dir)?;
    let path = norm(novel_root);
    let now = now_iso();
    match entries.iter_mut().find(|e| e.path == path) {
        Some(e) => {
            e.last_opened_at = now;
            if !title.trim().is_empty() {
                e.title = title.to_string();
            }
        }
        None => entries.push(LibraryEntry {
            path,
            title: title.to_string(),
            added_at: now.clone(),
            last_opened_at: now,
        }),
    }
    save(dir, &entries)
}

/// Remove the entry from the list. The novel folder is not touched.
pub fn forget(dir: &Path, novel_path: &str) -> Result<bool> {
    let mut entries = load(dir)?;
    let before = entries.len();
    entries.retain(|e| e.path != novel_path);
    let removed = entries.len() != before;
    if removed {
        save(dir, &entries)?;
    }
    Ok(removed)
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
}

/// A read-only look at one novel. Never migrates or writes, so it is safe to call on a
/// novel that is open in the app or was written by a newer version.
pub fn summarize(entry: &LibraryEntry) -> NovelSummary {
    let root = PathBuf::from(&entry.path);
    let mut s = NovelSummary {
        path: entry.path.clone(),
        title: entry.title.clone(),
        last_opened_at: entry.last_opened_at.clone(),
        exists: root.is_dir(),
        has_vault: false,
        files: 0,
        current_files: 0,
        arcs: 0,
        chapters: 0,
        characters: 0,
        canon_items: 0,
        open_conflicts: 0,
        pending_ai_suggestions: 0,
        last_activity: None,
    };
    if !s.exists || !vault_dir(&root).is_dir() {
        return s;
    }
    let Ok(conn) = Connection::open_with_flags(db_path(&root), OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return s;
    };
    s.has_vault = true;
    s.files = count(&conn, "SELECT COUNT(*) FROM files WHERE deleted_at IS NULL");
    s.current_files = count(&conn, "SELECT COUNT(*) FROM file_versions WHERE is_current = 1 AND deleted_at IS NULL");
    s.arcs = count(&conn, "SELECT COUNT(*) FROM arcs WHERE deleted_at IS NULL");
    s.chapters = count(&conn, "SELECT COUNT(*) FROM chapters WHERE deleted_at IS NULL");
    s.characters = count(&conn, "SELECT COUNT(*) FROM characters WHERE deleted_at IS NULL AND is_reader_sentinel = 0");
    s.canon_items = count(&conn, "SELECT COUNT(*) FROM canon_items WHERE deleted_at IS NULL AND superseded_by_id IS NULL");
    s.open_conflicts = count(&conn, "SELECT COUNT(*) FROM canon_conflicts WHERE status = 'OPEN'");
    s.pending_ai_suggestions = count(&conn, "SELECT COUNT(*) FROM ai_suggestions WHERE status = 'pending'");
    s.last_activity = conn
        .query_row("SELECT MAX(at) FROM audit_log", [], |r| r.get::<_, Option<String>>(0))
        .unwrap_or(None);
    s
}

pub fn summaries(dir: &Path) -> Result<Vec<NovelSummary>> {
    let mut entries = load(dir)?;
    entries.sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at));
    Ok(entries.iter().map(summarize).collect())
}
