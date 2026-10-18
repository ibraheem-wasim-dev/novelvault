//! Vault storage: SQLite inside `<novel>/.novelvault/vault.db`.
//!
//! Two invariants live here rather than in callers:
//!
//! * **Safe Mode** — every write helper funnels through [`write_vault_file`], which
//!   refuses any destination outside `<novel>/.novelvault/`. There is no API in this
//!   crate that can modify an author's document.
//! * **R2** — nothing is hard-deleted. Rows carry `deleted_at`; a file that vanishes
//!   from disk is marked `missing`, never removed from the index.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::Novel;
use crate::util::{new_id, now_iso};
use crate::{Result, VaultError};

/// Bump when the schema changes. A database written by a newer app version is refused
/// rather than opened and corrupted.
pub const SCHEMA_VERSION: i64 = 5;

pub fn vault_dir(root: &Path) -> PathBuf {
    root.join(".novelvault")
}
pub fn db_path(root: &Path) -> PathBuf {
    vault_dir(root).join("vault.db")
}
pub fn index_dir(root: &Path) -> PathBuf {
    vault_dir(root).join("index")
}
pub fn index_versions_dir(root: &Path) -> PathBuf {
    index_dir(root).join("versions")
}
pub fn cache_dir(root: &Path) -> PathBuf {
    vault_dir(root).join("cache")
}
pub fn logs_dir(root: &Path) -> PathBuf {
    vault_dir(root).join("logs")
}
pub fn snapshots_dir(root: &Path) -> PathBuf {
    vault_dir(root).join("snapshots")
}
pub fn settings_path(root: &Path) -> PathBuf {
    vault_dir(root).join("settings.json")
}

/// Safe Mode gate. Returns an error if `path` is not inside `<root>/.novelvault/`.
pub fn ensure_inside_vault(root: &Path, path: &Path) -> Result<()> {
    let vault = canonicalize_lossy(&vault_dir(root));
    let target = canonicalize_lossy(path);
    if target.starts_with(&vault) {
        Ok(())
    } else {
        Err(VaultError::SafeMode(target.display().to_string()))
    }
}

/// Canonicalise as far as the path exists; the rest is appended verbatim. This keeps
/// the check usable for paths whose leaf file has not been created yet.
fn canonicalize_lossy(path: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            canonicalize_lossy(parent).join(name)
        }
        _ => path.to_path_buf(),
    }
}

/// The single write path of the application. Everything the app persists during a
/// scan, classification, archive or snapshot goes through here, so Safe Mode cannot
/// be bypassed by a new call site.
pub fn write_vault_file(root: &Path, rel: &str, bytes: &[u8]) -> Result<PathBuf> {
    let target = vault_dir(root).join(rel);
    ensure_inside_vault(root, &target)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, bytes)?;
    Ok(target)
}

pub struct Vault {
    pub root: PathBuf,
    pub novel_id: String,
    pub title: String,
    pub conn: Connection,
}

impl Vault {
    /// Create (or reopen) a vault for a novel folder. Never writes outside the folder.
    pub fn create(root: &Path, title: &str) -> Result<Vault> {
        std::fs::create_dir_all(root)?;
        std::fs::create_dir_all(vault_dir(root))?;
        std::fs::create_dir_all(index_versions_dir(root))?;
        std::fs::create_dir_all(cache_dir(root))?;
        std::fs::create_dir_all(logs_dir(root))?;
        std::fs::create_dir_all(snapshots_dir(root))?;

        let path = db_path(root);
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let vault = Vault {
            root: root.to_path_buf(),
            novel_id: String::new(),
            title: title.to_string(),
            conn,
        };
        vault.migrate()?;

        let existing = vault
            .conn
            .query_row("SELECT id, title FROM novels LIMIT 1", [], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .optional()?;

        if let Some((id, db_title)) = existing {
            let mut vault = vault;
            vault.novel_id = id;
            vault.title = db_title;
            return Ok(vault);
        }

        let id = new_id();
        let now = now_iso();
        vault.conn.execute(
            "INSERT INTO novels (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, title, now],
        )?;
        if vault.setting_get("ignore_globs").is_none() {
            vault.setting_set(
                "ignore_globs",
                "[\"~$\",\"*.tmp\",\".git\",\".novelvault\"]",
            )?;
        }
        vault.audit(
            "CreateNovel",
            "Novel",
            Some(&id),
            None,
            Some(&format!("{{\"title\":\"{}\"}}", title)),
        )?;

        let mut vault = vault;
        vault.novel_id = id;
        Ok(vault)
    }

    /// Reopen an existing vault. The filesystem is the source of truth, so the root
    /// path may have changed entirely since the last open (portability).
    pub fn open(root: &Path) -> Result<Vault> {
        let path = db_path(root);
        if !path.exists() {
            return Err(VaultError::NotFound(format!(
                "no vault at {} — use `init` first",
                path.display()
            )));
        }
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let vault = Vault {
            root: root.to_path_buf(),
            novel_id: String::new(),
            title: String::new(),
            conn,
        };
        vault.migrate()?;

        let found = vault
            .conn
            .query_row(
                "SELECT key, value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((_, value)) = found {
            let on_disk: i64 = value.parse().unwrap_or(0);
            if on_disk > SCHEMA_VERSION {
                return Err(VaultError::Other(format!(
                    "this vault was written by a newer NovelVault (schema {on_disk} > {SCHEMA_VERSION}); refusing to open it"
                )));
            }
        }

        let (id, title) = vault
            .conn
            .query_row("SELECT id, title FROM novels LIMIT 1", [], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .optional()?
            .ok_or_else(|| VaultError::NotFound("vault has no novel row".into()))?;

        let mut vault = vault;
        vault.novel_id = id;
        vault.title = title;
        Ok(vault)
    }

    pub fn open_or_create(root: &Path, title: &str) -> Result<Vault> {
        if db_path(root).exists() {
            Vault::open(root)
        } else {
            Vault::create(root, title)
        }
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        self.conn.execute(
            "INSERT INTO schema_meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    pub fn novel(&self) -> Novel {
        Novel {
            id: self.novel_id.clone(),
            title: self.title.clone(),
            created_at: String::new(),
        }
    }

    pub fn abs_path(&self, rel_path: &str) -> PathBuf {
        self.root.join(rel_path)
    }

    pub fn setting_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn setting_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO schema_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Append-only audit trail. Every mutation the app performs lands here.
    pub fn audit(
        &self,
        action: &str,
        entity_type: &str,
        entity_id: Option<&str>,
        before: Option<&str>,
        after: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit_log (id, novel_id, action, entity_type, entity_id, before_json, after_json, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                new_id(),
                self.novel_id,
                action,
                entity_type,
                entity_id,
                before,
                after,
                now_iso()
            ],
        )?;
        Ok(())
    }

    pub fn audit_tail(&self, limit: i64) -> Result<Vec<(String, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT at, action, entity_type, COALESCE(entity_id, '') FROM audit_log
             WHERE novel_id = ?1 ORDER BY at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![self.novel_id, limit], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS novels (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- A "file" row is a document family (Arc_2_Revised.docx, _v2, old Arc 2.pdf).
CREATE TABLE IF NOT EXISTS files (
    id               TEXT PRIMARY KEY,
    novel_id         TEXT NOT NULL REFERENCES novels(id),
    family_name      TEXT NOT NULL,
    family_confirmed INTEGER NOT NULL DEFAULT 0,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    deleted_at       TEXT,
    UNIQUE (novel_id, family_name)
);

CREATE TABLE IF NOT EXISTS file_versions (
    id               TEXT PRIMARY KEY,
    file_id          TEXT NOT NULL REFERENCES files(id),
    novel_id         TEXT NOT NULL,
    version_number   INTEGER NOT NULL,
    rel_path         TEXT NOT NULL,
    sha256           TEXT NOT NULL,
    size_bytes       INTEGER NOT NULL,
    mtime            TEXT,
    extraction_state TEXT NOT NULL DEFAULT 'pending',
    extraction_note  TEXT,
    extracted_chars  INTEGER NOT NULL DEFAULT 0,
    file_status      TEXT NOT NULL DEFAULT 'UNKNOWN',
    status_source    TEXT NOT NULL DEFAULT 'import',
    arc_label        TEXT,
    chapter_label    TEXT,
    version_label    TEXT,
    is_current       INTEGER NOT NULL DEFAULT 0,
    missing          INTEGER NOT NULL DEFAULT 0,
    last_seen_at     TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    deleted_at       TEXT,
    UNIQUE (file_id, version_number)
);

-- One CURRENT per family, enforced by the database rather than by the UI.
CREATE UNIQUE INDEX IF NOT EXISTS ux_current_per_family
    ON file_versions (file_id) WHERE is_current = 1 AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS ix_versions_path ON file_versions (rel_path);
CREATE INDEX IF NOT EXISTS ix_versions_sha  ON file_versions (sha256);
CREATE INDEX IF NOT EXISTS ix_versions_status ON file_versions (novel_id, file_status);

CREATE TABLE IF NOT EXISTS file_events (
    id       TEXT PRIMARY KEY,
    novel_id TEXT NOT NULL,
    kind     TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    detail   TEXT,
    at       TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS inbox_items (
    id                  TEXT PRIMARY KEY,
    novel_id            TEXT NOT NULL,
    kind                TEXT NOT NULL,
    title               TEXT NOT NULL,
    detail              TEXT,
    rel_path            TEXT,
    file_version_id     TEXT,
    candidate_version_id TEXT,
    status              TEXT NOT NULL DEFAULT 'OPEN',
    resolved_action     TEXT,
    resolved_at         TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

-- Do not queue the same kind of question about the same path twice.
CREATE UNIQUE INDEX IF NOT EXISTS ux_inbox_open_item
    ON inbox_items (novel_id, kind, rel_path) WHERE status = 'OPEN';

CREATE TABLE IF NOT EXISTS audit_log (
    id           TEXT PRIMARY KEY,
    novel_id     TEXT,
    action       TEXT NOT NULL,
    entity_type  TEXT NOT NULL,
    entity_id    TEXT,
    before_json  TEXT,
    after_json   TEXT,
    at           TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshots (
    id            TEXT PRIMARY KEY,
    novel_id      TEXT NOT NULL,
    name          TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    rel_path      TEXT NOT NULL,
    includes_files INTEGER NOT NULL DEFAULT 0,
    note          TEXT
);

CREATE INDEX IF NOT EXISTS ix_inbox_status ON inbox_items (novel_id, status, kind);

-- =====================================================================
-- V2 (Phase 2) — story records
-- =====================================================================

CREATE TABLE IF NOT EXISTS arcs (
    id         TEXT PRIMARY KEY,
    novel_id   TEXT NOT NULL REFERENCES novels(id),
    number     INTEGER NOT NULL,
    title      TEXT NOT NULL,
    purpose    TEXT,
    summary    TEXT,
    status     TEXT NOT NULL DEFAULT 'Planned',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);

-- Arc numbers are unique per novel, enforced by the database, soft-deleted rows
-- excluded so a deleted arc frees its number again.
CREATE UNIQUE INDEX IF NOT EXISTS ux_arc_number
    ON arcs (novel_id, number) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS chapters (
    id                 TEXT PRIMARY KEY,
    arc_id             TEXT NOT NULL REFERENCES arcs(id),
    number             INTEGER NOT NULL,
    title              TEXT NOT NULL,
    purpose            TEXT,
    summary            TEXT,
    emotional_movement TEXT,
    status             TEXT NOT NULL DEFAULT 'Planned',
    word_count         INTEGER NOT NULL DEFAULT 0,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    deleted_at         TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_chapter_number
    ON chapters (arc_id, number) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS scenes (
    id               TEXT PRIMARY KEY,
    chapter_id       TEXT NOT NULL REFERENCES chapters(id),
    number           INTEGER NOT NULL,
    title            TEXT NOT NULL,
    purpose          TEXT,
    pov_character_id TEXT,
    location         TEXT,
    summary          TEXT,
    word_count       INTEGER NOT NULL DEFAULT 0,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    deleted_at       TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_scene_number
    ON scenes (chapter_id, number) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS characters (
    id                 TEXT PRIMARY KEY,
    novel_id           TEXT NOT NULL REFERENCES novels(id),
    name               TEXT NOT NULL,
    role               TEXT,
    description        TEXT,
    is_reader_sentinel INTEGER NOT NULL DEFAULT 0,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    deleted_at         TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_character_name
    ON characters (novel_id, name) WHERE deleted_at IS NULL;

-- R4: exactly one "what the reader knows" row per novel.
CREATE UNIQUE INDEX IF NOT EXISTS ux_reader_sentinel
    ON characters (novel_id) WHERE is_reader_sentinel = 1 AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS locations (
    id          TEXT PRIMARY KEY,
    novel_id    TEXT NOT NULL REFERENCES novels(id),
    name        TEXT NOT NULL,
    description TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT
);

CREATE TABLE IF NOT EXISTS story_objects (
    id          TEXT PRIMARY KEY,
    novel_id    TEXT NOT NULL REFERENCES novels(id),
    name        TEXT NOT NULL,
    description TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT
);

CREATE TABLE IF NOT EXISTS notes (
    id          TEXT PRIMARY KEY,
    novel_id    TEXT NOT NULL REFERENCES novels(id),
    category    TEXT NOT NULL,
    body        TEXT NOT NULL,
    is_archived INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT
);

CREATE TABLE IF NOT EXISTS note_links (
    id          TEXT PRIMARY KEY,
    note_id     TEXT NOT NULL REFERENCES notes(id),
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    link_type   TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_note_link
    ON note_links (note_id, entity_type, entity_id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS ideas (
    id              TEXT PRIMARY KEY,
    novel_id        TEXT NOT NULL REFERENCES novels(id),
    text            TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'UNDECIDED',
    decision_reason TEXT,
    decided_at      TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    deleted_at      TEXT
);

-- =====================================================================
-- V3 (Phase 3) — the Canon Engine
-- =====================================================================

CREATE TABLE IF NOT EXISTS canon_items (
    id               TEXT PRIMARY KEY,
    novel_id         TEXT NOT NULL REFERENCES novels(id),
    subject          TEXT NOT NULL,
    value            TEXT NOT NULL,
    category         TEXT,
    status           TEXT NOT NULL DEFAULT 'PROPOSED',
    entity_type      TEXT,
    entity_id        TEXT,
    reason           TEXT,
    superseded_by_id TEXT,
    superseded_at    TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    deleted_at       TEXT
);

CREATE INDEX IF NOT EXISTS ix_canon_subject ON canon_items (novel_id, subject);
CREATE INDEX IF NOT EXISTS ix_canon_status  ON canon_items (novel_id, status);

-- R3: the evidence behind a canon fact.
CREATE TABLE IF NOT EXISTS canon_sources (
    id           TEXT PRIMARY KEY,
    canon_item_id TEXT NOT NULL REFERENCES canon_items(id),
    source_type  TEXT NOT NULL,
    source_id    TEXT NOT NULL,
    source_label TEXT,
    location     TEXT,
    created_at   TEXT NOT NULL,
    deleted_at   TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_canon_source
    ON canon_sources (canon_item_id, source_type, source_id) WHERE deleted_at IS NULL;

-- Append-only: rows here are never updated or deleted.
CREATE TABLE IF NOT EXISTS canon_history (
    id            TEXT PRIMARY KEY,
    canon_item_id TEXT NOT NULL,
    novel_id      TEXT NOT NULL,
    action        TEXT NOT NULL,
    reason        TEXT,
    before_json   TEXT,
    after_json    TEXT,
    at            TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS ix_canon_history ON canon_history (canon_item_id, at);

CREATE TABLE IF NOT EXISTS canon_conflicts (
    id                  TEXT PRIMARY KEY,
    novel_id            TEXT NOT NULL,
    subject             TEXT NOT NULL,
    current_value       TEXT,
    conflicting_value   TEXT,
    current_source      TEXT,
    conflicting_source  TEXT,
    detector            TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'OPEN',
    resolution          TEXT,
    resolved_at         TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS ix_conflicts_open ON canon_conflicts (novel_id, status);

-- R4: what each character knows, separately from what the reader knows.
CREATE TABLE IF NOT EXISTS character_knowledge (
    id            TEXT PRIMARY KEY,
    novel_id      TEXT NOT NULL,
    character_id  TEXT NOT NULL REFERENCES characters(id),
    canon_item_id TEXT NOT NULL REFERENCES canon_items(id),
    state         TEXT NOT NULL DEFAULT 'UNKNOWN',
    note          TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    deleted_at    TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_character_knowledge
    ON character_knowledge (character_id, canon_item_id) WHERE deleted_at IS NULL;

-- Phase 4: the local AI layer. Everything here is DERIVED and rebuildable from the
-- novel folder; none of it is canon, and none of it ever edits a file (R1, R2).
CREATE TABLE IF NOT EXISTS ai_indexed (
    version_id  TEXT PRIMARY KEY,
    sha256      TEXT NOT NULL,
    chunk_count INTEGER NOT NULL,
    indexed_at  TEXT NOT NULL
);

-- Full-text index over passages of every extracted file version (BM25 ranking).
CREATE VIRTUAL TABLE IF NOT EXISTS ai_chunks_fts USING fts5(
    body,
    version_id UNINDEXED,
    ordinal UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS ai_messages (
    id           TEXT PRIMARY KEY,
    novel_id     TEXT NOT NULL,
    role         TEXT NOT NULL,
    content      TEXT NOT NULL,
    sources_json TEXT,
    mode         TEXT,
    created_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ai_digests (
    version_id TEXT PRIMARY KEY,
    digest     TEXT NOT NULL,
    method     TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- Things the assistant NOTICED. They wait here until a person decides; accepting one is
-- the person's act, never the assistant's.
CREATE TABLE IF NOT EXISTS ai_suggestions (
    id            TEXT PRIMARY KEY,
    novel_id      TEXT NOT NULL,
    kind          TEXT NOT NULL,
    title         TEXT NOT NULL,
    detail        TEXT NOT NULL,
    evidence_json TEXT,
    status        TEXT NOT NULL DEFAULT 'pending',
    decision_note TEXT,
    created_at    TEXT NOT NULL,
    decided_at    TEXT,
    UNIQUE (novel_id, kind, title)
);
CREATE INDEX IF NOT EXISTS ix_ai_suggestions_status ON ai_suggestions (novel_id, status);

-- Phase 5: the Reader. A crawler that reads every file version in story order and keeps what
-- it learned. All DERIVED from the novel folder: deleting these tables loses nothing the
-- files cannot rebuild, and nothing here is canon (R1). Rows are keyed by file version, so
-- when a draft is archived or made CURRENT its facts become history or live without re-reading.
CREATE TABLE IF NOT EXISTS reader_files (
    version_id TEXT PRIMARY KEY,
    sha256     TEXT NOT NULL,
    sentences  INTEGER NOT NULL DEFAULT 0,
    method     TEXT NOT NULL DEFAULT 'heuristic',
    summary    TEXT,
    read_at    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS reader_names (
    version_id TEXT NOT NULL,
    name       TEXT NOT NULL,
    mid        INTEGER NOT NULL DEFAULT 0,
    total      INTEGER NOT NULL DEFAULT 0,
    person_cue INTEGER NOT NULL DEFAULT 0,
    place_cue  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (version_id, name)
);
CREATE TABLE IF NOT EXISTS reader_facts (
    id         TEXT PRIMARY KEY,
    version_id TEXT NOT NULL,
    ordinal    INTEGER NOT NULL,
    subject    TEXT NOT NULL,
    predicate  TEXT NOT NULL,
    object     TEXT NOT NULL,
    excerpt    TEXT NOT NULL,
    source     TEXT NOT NULL DEFAULT 'heuristic',
    inferred   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ix_reader_facts_version ON reader_facts (version_id);
CREATE INDEX IF NOT EXISTS ix_reader_facts_subject ON reader_facts (subject);
CREATE TABLE IF NOT EXISTS reader_events (
    id            TEXT PRIMARY KEY,
    version_id    TEXT NOT NULL,
    ordinal       INTEGER NOT NULL,
    seq           INTEGER NOT NULL,
    summary       TEXT NOT NULL,
    entities_json TEXT,
    salience      INTEGER NOT NULL DEFAULT 0,
    source        TEXT NOT NULL DEFAULT 'heuristic'
);
CREATE INDEX IF NOT EXISTS ix_reader_events_version ON reader_events (version_id, seq);
CREATE TABLE IF NOT EXISTS reader_threads (
    id            TEXT PRIMARY KEY,
    version_id    TEXT NOT NULL,
    ordinal       INTEGER NOT NULL,
    excerpt       TEXT NOT NULL,
    cue           TEXT NOT NULL,
    entities_json TEXT
);
CREATE INDEX IF NOT EXISTS ix_reader_threads_version ON reader_threads (version_id);
CREATE TABLE IF NOT EXISTS reader_relations (
    version_id TEXT NOT NULL,
    a          TEXT NOT NULL,
    b          TEXT NOT NULL,
    n          INTEGER NOT NULL,
    PRIMARY KEY (version_id, a, b)
);
"#;
