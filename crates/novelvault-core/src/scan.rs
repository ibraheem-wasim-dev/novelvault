//! Folder scanning and per-file import. This is the only way anything enters the
//! index, and it **never writes to the author's files** — it opens them read-only,
//! hashes them, extracts text into `.novelvault/index/`, and records what it found.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{params, OptionalExtension};

use crate::db::{self, Vault};
use crate::extract::{self, Extracted};
use crate::inbox;
use crate::model::*;
use crate::util::{hash_file, new_id, now_iso};
use crate::Result;

// ---------------------------------------------------------------------------
// Ignore rules
// ---------------------------------------------------------------------------

pub fn default_ignore_globs() -> Vec<String> {
    vec![
        "~$".to_string(),
        "*.tmp".to_string(),
        ".git".to_string(),
        ".novelvault".to_string(),
    ]
}

pub fn ignore_globs(vault: &Vault) -> Vec<String> {
    vault
        .setting_get("ignore_globs")
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_else(default_ignore_globs)
}

pub fn is_ignored(rel: &str, globs: &[String]) -> bool {
    let name = Path::new(rel)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    if rel.split('/').any(|seg| seg == ".novelvault") {
        return true;
    }

    for glob in globs {
        let g = glob.trim();
        if g.is_empty() {
            continue;
        }
        if let Some(prefix) = g.strip_suffix('*') {
            if name.starts_with(prefix) || rel.starts_with(prefix) {
                return true;
            }
        } else if let Some(suffix) = g.strip_prefix('*') {
            if name.ends_with(suffix) {
                return true;
            }
        } else if rel.split('/').any(|seg| seg == g) || name == g || name.starts_with(g) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Novel-relative path with forward slashes, so a vault stays portable between
/// Windows and POSIX hosts.
pub fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn collect_files(root: &Path, globs: &[String], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = rel_path(root, &path);
        if is_ignored(&rel, globs) {
            continue;
        }
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => collect_files(&path, globs, out),
            Ok(ft) if ft.is_file() => out.push(path),
            _ => {}
        }
    }
}

fn mtime_iso(meta: &std::fs::Metadata) -> Option<String> {
    meta.modified().ok().and_then(|t| {
        let dt: chrono::DateTime<chrono::Utc> = t.into();
        Some(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    })
}

// ---------------------------------------------------------------------------
// Import outcome
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportKind {
    Unchanged,
    NewFile,
    NewVersion,
    RenameCandidate,
    Duplicate,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct ImportOutcome {
    pub kind: ImportKind,
    pub rel_path: String,
    pub version_id: Option<String>,
    pub item_id: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
struct ExistingVersion {
    id: String,
    file_id: String,
    sha256: String,
    version_number: i64,
    missing: i64,
}

fn latest_version_for_path(vault: &Vault, rel: &str) -> Result<Option<ExistingVersion>> {
    let row = vault
        .conn
        .query_row(
            "SELECT id, file_id, sha256, version_number, missing FROM file_versions
             WHERE novel_id = ?1 AND rel_path = ?2 AND deleted_at IS NULL
             ORDER BY version_number DESC LIMIT 1",
            params![vault.novel_id, rel],
            |r| {
                Ok(ExistingVersion {
                    id: r.get(0)?,
                    file_id: r.get(1)?,
                    sha256: r.get(2)?,
                    version_number: r.get(3)?,
                    missing: r.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// A version whose contents match but whose recorded path is no longer on disk: the
/// file moved or was renamed. Checked against the filesystem, not just the flag, so a
/// rename is recognised even before the end-of-scan sweep marks the old path missing.
fn find_relocated_version(vault: &Vault, sha: &str) -> Result<Option<(ExistingVersion, String)>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, file_id, sha256, version_number, missing, rel_path FROM file_versions
         WHERE novel_id = ?1 AND sha256 = ?2 AND deleted_at IS NULL
         ORDER BY version_number DESC",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id, sha], |r| {
            Ok((
                ExistingVersion {
                    id: r.get(0)?,
                    file_id: r.get(1)?,
                    sha256: r.get(2)?,
                    version_number: r.get(3)?,
                    missing: r.get(4)?,
                },
                r.get::<_, String>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (version, rel) in rows {
        if version.missing == 0 && vault.abs_path(&rel).is_file() {
            continue;
        }
        return Ok(Some((version, rel)));
    }
    Ok(None)
}

fn find_version_by_hash(vault: &Vault, sha: &str, only_missing: bool) -> Result<Option<ExistingVersion>> {
    let sql = if only_missing {
        "SELECT id, file_id, sha256, version_number, missing FROM file_versions
         WHERE novel_id = ?1 AND sha256 = ?2 AND missing = 1 AND deleted_at IS NULL
         ORDER BY version_number DESC LIMIT 1"
    } else {
        "SELECT id, file_id, sha256, version_number, missing FROM file_versions
         WHERE novel_id = ?1 AND sha256 = ?2 AND missing = 0 AND deleted_at IS NULL
         ORDER BY version_number DESC LIMIT 1"
    };
    let row = vault
        .conn
        .query_row(sql, params![vault.novel_id, sha], |r| {
            Ok(ExistingVersion {
                id: r.get(0)?,
                file_id: r.get(1)?,
                sha256: r.get(2)?,
                version_number: r.get(3)?,
                missing: r.get(4)?,
            })
        })
        .optional()?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Family naming (proposals only — nothing is merged without the author)
// ---------------------------------------------------------------------------

/// Reduce a filename to a family key: `Arc_2_Revised_v2.docx` → `arc-2`.
fn family_key(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let mut value = stem.replace(['_', '.'], " ");
    // Parenthetical counters: "(1)", "(2)".
    while let Some(open) = value.rfind('(') {
        if value[open..].starts_with('(') && value.ends_with(')') {
            value.truncate(open);
            value = value.trim_end().to_string();
        } else {
            break;
        }
    }

    let is_marker = |token: &str| -> bool {
        matches!(
            token,
            "final" | "revised" | "revision" | "revisions" | "draft" | "drafts" | "edit"
                | "edited" | "updated" | "copy" | "new" | "old"
        ) || (token.len() > 1
            && token.starts_with('v')
            && token[1..].chars().all(|c| c.is_ascii_digit()))
    };

    let mut tokens: Vec<String> = Vec::new();
    for raw in value.split_whitespace() {
        if is_marker(raw) {
            continue;
        }
        let cleaned: String = raw.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if !cleaned.is_empty() {
            tokens.push(cleaned);
        }
    }

    // A trailing number is a copy counter ("Arc 3 Draft 2") — but never drop the only
    // number in the name, because that number is usually the arc ("Arc 3").
    let numeric_count = tokens.iter().filter(|t| t.chars().all(|c| c.is_ascii_digit())).count();
    if numeric_count >= 2 {
        if tokens.last().is_some_and(|t| t.chars().all(|c| c.is_ascii_digit())) {
            tokens.pop();
        }
    }

    tokens.join("-")
}

fn other_family_with_same_key(vault: &Vault, key: &str, own_file_id: &str) -> Result<Option<(String, String)>> {
    let mut stmt = vault.conn.prepare(
        "SELECT f.id, f.family_name FROM files f
         WHERE f.novel_id = ?1 AND f.deleted_at IS NULL AND f.id <> ?2",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id, own_file_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, name) in rows {
        if family_key(&name) == key {
            return Ok(Some((id, name)));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

fn extract_into_index(vault: &Vault, version_id: &str, rel: &str) -> Result<(String, Option<String>, i64)> {
    let abs = vault.abs_path(rel);
    let outcome = if extract::is_text_extractable(rel) {
        extract::extract_file(&abs)
    } else {
        Extracted::Unsupported
    };

    match outcome {
        Extracted::Text(text) => {
            let chars = text.chars().count() as i64;
            // Written through the Safe Mode gate, which cannot reach outside .novelvault/.
            db::write_vault_file(vault.root.as_path(), &format!("index/versions/{version_id}.txt"), text.as_bytes())?;
            Ok((ExtractionState::Ok.as_str().to_string(), None, chars))
        }
        Extracted::Unsupported => Ok((
            ExtractionState::Unsupported.as_str().to_string(),
            Some(format!("no text extractor for .{} files", extract::extension(rel))),
            0,
        )),
        Extracted::Failed(note) => Ok((ExtractionState::Failed.as_str().to_string(), Some(note), 0)),
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Import or re-import one file. Nothing here modifies the file itself.
pub fn import_file(vault: &Vault, rel: &str) -> Result<ImportOutcome> {
    let abs = vault.abs_path(rel);
    let meta = match std::fs::metadata(&abs) {
        Ok(m) if m.is_file() => m,
        _ => {
            return Ok(ImportOutcome {
                kind: ImportKind::Skipped,
                rel_path: rel.to_string(),
                version_id: None,
                item_id: None,
                detail: Some("not a readable file".into()),
            })
        }
    };

    let (sha, size) = hash_file(&abs)?;
    let mtime = mtime_iso(&meta);
    let now = now_iso();

    // --- already known at this path? -----------------------------------------
    if let Some(existing) = latest_version_for_path(vault, rel)? {
        if existing.sha256 == sha {
            vault.conn.execute(
                "UPDATE file_versions SET last_seen_at = ?1, missing = 0, updated_at = ?1 WHERE id = ?2",
                params![now, existing.id],
            )?;
            return Ok(ImportOutcome {
                kind: ImportKind::Unchanged,
                rel_path: rel.to_string(),
                version_id: Some(existing.id),
                item_id: None,
                detail: None,
            });
        }

        // Same path, different bytes → a new version in the same family. The old row
        // and its extracted text stay exactly where they are (R2).
        let next: i64 = vault.conn.query_row(
            "SELECT COALESCE(MAX(version_number), 0) + 1 FROM file_versions WHERE file_id = ?1",
            params![existing.file_id],
            |r| r.get(0),
        )?;
        let new_version_id = new_id();
        vault.conn.execute(
            "INSERT INTO file_versions
             (id, file_id, novel_id, version_number, rel_path, sha256, size_bytes, mtime,
              extraction_state, file_status, status_source, is_current, missing, last_seen_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 'UNKNOWN', 'import', 0, 0, ?9, ?9, ?9)",
            params![
                new_version_id,
                existing.file_id,
                vault.novel_id,
                next,
                rel,
                sha,
                size as i64,
                mtime,
                now
            ],
        )?;
        let (state, note, chars) = extract_into_index(vault, &new_version_id, rel)?;
        vault.conn.execute(
            "UPDATE file_versions SET extraction_state = ?1, extraction_note = ?2, extracted_chars = ?3, updated_at = ?4 WHERE id = ?5",
            params![state, note, chars, now_iso(), new_version_id],
        )?;

        let family: String = vault.conn.query_row(
            "SELECT family_name FROM files WHERE id = ?1",
            params![existing.file_id],
            |r| r.get(0),
        )?;
        let item = inbox::raise(
            vault,
            inbox::NewItem {
                kind: InboxKind::NewVersion,
                title: format!("New version detected — {rel}"),
                detail: Some(format!(
                    "{family} now has Version {next}. The previous version is still stored and still searchable."
                )),
                rel_path: Some(rel.to_string()),
                file_version_id: Some(new_version_id.clone()),
                candidate_version_id: Some(existing.id.clone()),
            },
        )?;

        vault.audit(
            "ImportVersion",
            "FileVersion",
            Some(&new_version_id),
            None,
            Some(&format!("{{\"rel_path\":\"{rel}\",\"version\":{next}}}")),
        )?;

        if state == ExtractionState::Failed.as_str() {
            inbox::raise(
                vault,
                inbox::NewItem {
                    kind: InboxKind::ExtractionFailed,
                    title: format!("Text extraction failed — {rel}"),
                    detail: note.clone(),
                    rel_path: Some(rel.to_string()),
                    file_version_id: Some(new_version_id.clone()),
                    candidate_version_id: None,
                },
            )?;
        }

        return Ok(ImportOutcome {
            kind: ImportKind::NewVersion,
            rel_path: rel.to_string(),
            version_id: Some(new_version_id),
            item_id: item,
            detail: Some(format!("version {next} of {family}")),
        });
    }

    // --- path unknown: is this a file that moved? ----------------------------
    if let Some((moved, old_path)) = find_relocated_version(vault, &sha)? {
        let item = inbox::raise(
            vault,
            inbox::NewItem {
                kind: InboxKind::PossibleRename,
                title: format!("Possible rename — {rel}"),
                detail: Some(format!(
                    "Identical contents to {old_path}, which is no longer on disk. Relink, or keep them as separate records?"
                )),
                rel_path: Some(rel.to_string()),
                file_version_id: None,
                candidate_version_id: Some(moved.id.clone()),
            },
        )?;
        return Ok(ImportOutcome {
            kind: ImportKind::RenameCandidate,
            rel_path: rel.to_string(),
            version_id: None,
            item_id: item,
            detail: Some(format!("matches {old_path}")),
        });
    }

    // --- brand new path ------------------------------------------------------
    let file_name = Path::new(rel)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| rel.to_string());
    let family_name = file_name.clone();
    let key = family_key(&file_name);

    let file_id = new_id();
    vault.conn.execute(
        "INSERT INTO files (id, novel_id, family_name, family_confirmed, created_at, updated_at)
         VALUES (?1, ?2, ?3, 0, ?4, ?4)",
        params![file_id, vault.novel_id, family_name, now],
    )?;

    let version_id = new_id();
    vault.conn.execute(
        "INSERT INTO file_versions
         (id, file_id, novel_id, version_number, rel_path, sha256, size_bytes, mtime,
          extraction_state, file_status, status_source, is_current, missing, last_seen_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, 'pending', 'UNKNOWN', 'import', 0, 0, ?8, ?8, ?8)",
        params![version_id, file_id, vault.novel_id, rel, sha, size as i64, mtime, now],
    )?;
    let (state, note, chars) = extract_into_index(vault, &version_id, rel)?;
    vault.conn.execute(
        "UPDATE file_versions SET extraction_state = ?1, extraction_note = ?2, extracted_chars = ?3, updated_at = ?4 WHERE id = ?5",
        params![state, note, chars, now_iso(), version_id],
    )?;

    // A twin of an existing live file: record it, but ask before deciding anything.
    let twin = find_version_by_hash(vault, &sha, false)?;
    if let Some(twin) = twin {
        if twin.id != version_id {
            let twin_path: String = vault.conn.query_row(
                "SELECT rel_path FROM file_versions WHERE id = ?1",
                params![twin.id],
                |r| r.get(0),
            )?;
            let item = inbox::raise(
                vault,
                inbox::NewItem {
                    kind: InboxKind::PossibleDuplicate,
                    title: format!("Possible duplicate — {rel}"),
                    detail: Some(format!(
                        "Byte-identical to {twin_path}. Keep as an independent file, or mark it a version of that family?"
                    )),
                    rel_path: Some(rel.to_string()),
                    file_version_id: Some(version_id.clone()),
                    candidate_version_id: Some(twin.id.clone()),
                },
            )?;
            return Ok(ImportOutcome {
                kind: ImportKind::Duplicate,
                rel_path: rel.to_string(),
                version_id: Some(version_id),
                item_id: item,
                detail: Some(format!("identical to {twin_path}")),
            });
        }
    }

    let item = inbox::raise(
        vault,
        inbox::NewItem {
            kind: InboxKind::NewFile,
            title: format!("New file detected — {rel}"),
            detail: Some(format!(
                "{} · imported as UNKNOWN. Nothing becomes current or canon until you say so.",
                state
            )),
            rel_path: Some(rel.to_string()),
            file_version_id: Some(version_id.clone()),
            candidate_version_id: None,
        },
    )?;

    // Naming suggests this belongs to a family already in the vault.
    if let Some((other_id, other_name)) = other_family_with_same_key(vault, &key, &file_id)? {
        // The candidate is that family's latest version, so the UI can show what the
        // merge would sit alongside.
        let other_version: Option<String> = vault
            .conn
            .query_row(
                "SELECT id FROM file_versions WHERE file_id = ?1 AND deleted_at IS NULL
                 ORDER BY version_number DESC LIMIT 1",
                params![other_id],
                |r| r.get(0),
            )
            .optional()?;
        inbox::raise(
            vault,
            inbox::NewItem {
                kind: InboxKind::FamilyProposal,
                title: format!("Version family proposed — {file_name}"),
                detail: Some(format!(
                    "The name resembles {other_name} (same family key '{key}'). Merge into one family with version history, or keep separate?"
                )),
                rel_path: Some(rel.to_string()),
                file_version_id: Some(version_id.clone()),
                candidate_version_id: other_version,
            },
        )?;
    }

    vault.audit(
        "ImportFile",
        "FileVersion",
        Some(&version_id),
        None,
        Some(&format!("{{\"rel_path\":\"{rel}\",\"sha256_prefix\":\"{}\"}}", &sha[..12])),
    )?;

    if state == ExtractionState::Failed.as_str() {
        inbox::raise(
            vault,
            inbox::NewItem {
                kind: InboxKind::ExtractionFailed,
                title: format!("Text extraction failed — {rel}"),
                detail: note.clone(),
                rel_path: Some(rel.to_string()),
                file_version_id: Some(version_id.clone()),
                candidate_version_id: None,
            },
        )?;
    }

    Ok(ImportOutcome {
        kind: ImportKind::NewFile,
        rel_path: rel.to_string(),
        version_id: Some(version_id),
        item_id: item,
        detail: None,
    })
}

// ---------------------------------------------------------------------------
// Full scan
// ---------------------------------------------------------------------------

fn kind_label(category: &str) -> String {
    match category {
        "image" => "image".to_string(),
        "no-extension" => "no-extension".to_string(),
        other => other.to_string(),
    }
}

fn headline(by_kind: &BTreeMap<String, usize>, total: usize) -> String {
    if total == 0 {
        return "No files found in this folder yet.".to_string();
    }
    let order = ["DOCX", "PDF", "Markdown", "TXT", "image", "other", "no-extension"];
    let mut parts: Vec<String> = Vec::new();
    for key in order {
        if let Some(count) = by_kind.get(key) {
            if *count > 0 {
                parts.push(format!("{count} {}", kind_label(key)));
            }
        }
    }
    for (key, count) in by_kind {
        if !order.contains(&key.as_str()) && *count > 0 {
            parts.push(format!("{count} {key}"));
        }
    }
    format!("{total} files found — {}.", parts.join(", "))
}

pub fn scan(vault: &Vault) -> Result<ScanSummary> {
    let started = Instant::now();
    let root = vault.root.clone();
    let globs = ignore_globs(vault);

    let mut paths: Vec<PathBuf> = Vec::new();
    collect_files(&root, &globs, &mut paths);

    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen_paths: Vec<String> = Vec::new();
    let mut summary = ScanSummary {
        root: root.display().to_string(),
        files_seen: 0,
        by_kind: BTreeMap::new(),
        imported: 0,
        unchanged: 0,
        new_versions: 0,
        rename_candidates: 0,
        duplicates: 0,
        marked_missing: 0,
        extraction_failed: 0,
        extraction_unsupported: 0,
        duration_ms: 0,
        headline: String::new(),
    };

    for path in paths {
        let rel = rel_path(&root, &path);
        seen_paths.push(rel.clone());
        *by_kind.entry(extract::category(&rel).to_string()).or_insert(0) += 1;

        let outcome = import_file(vault, &rel)?;
        match outcome.kind {
            ImportKind::Unchanged => summary.unchanged += 1,
            ImportKind::NewFile => summary.imported += 1,
            ImportKind::NewVersion => summary.new_versions += 1,
            ImportKind::RenameCandidate => summary.rename_candidates += 1,
            ImportKind::Duplicate => summary.duplicates += 1,
            ImportKind::Skipped => {}
        }
        if let Some(version_id) = outcome.version_id {
            let state: Option<String> = vault
                .conn
                .query_row(
                    "SELECT extraction_state FROM file_versions WHERE id = ?1",
                    params![version_id],
                    |r| r.get(0),
                )
                .optional()?;
            match state.as_deref() {
                Some("failed") => summary.extraction_failed += 1,
                Some("unsupported") => summary.extraction_unsupported += 1,
                _ => {}
            }
        }
    }

    // Anything indexed but no longer on disk is marked missing — never deleted (R2).
    let indexed: Vec<(String, String)> = {
        let mut stmt = vault.conn.prepare(
            "SELECT id, rel_path FROM file_versions
             WHERE novel_id = ?1 AND missing = 0 AND deleted_at IS NULL",
        )?;
        let rows = stmt
            .query_map(params![vault.novel_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let seen: std::collections::HashSet<&String> = seen_paths.iter().collect();
    for (version_id, rel) in indexed {
        if seen.contains(&rel) {
            continue;
        }
        vault.conn.execute(
            "UPDATE file_versions SET missing = 1, updated_at = ?1 WHERE id = ?2",
            params![now_iso(), version_id],
        )?;
        summary.marked_missing += 1;
        vault.audit("MarkMissing", "FileVersion", Some(&version_id), None, Some(&format!("{{\"rel_path\":\"{rel}\"}}")))?;
        inbox::raise(
            vault,
            inbox::NewItem {
                kind: InboxKind::MissingFile,
                title: format!("File no longer on disk — {rel}"),
                detail: Some(
                    "The record, its extracted text and its links are kept. Nothing was deleted.".to_string(),
                ),
                rel_path: Some(rel),
                file_version_id: Some(version_id),
                candidate_version_id: None,
            },
        )?;
    }

    summary.files_seen = seen_paths.len();
    summary.by_kind = by_kind.clone();
    summary.headline = headline(&by_kind, seen_paths.len());
    summary.duration_ms = started.elapsed().as_millis();

    vault.audit(
        "Scan",
        "Novel",
        Some(&vault.novel_id),
        None,
        Some(&format!(
            "{{\"files_seen\":{},\"imported\":{},\"new_versions\":{},\"marked_missing\":{}}}",
            summary.files_seen, summary.imported, summary.new_versions, summary.marked_missing
        )),
    )?;

    Ok(summary)
}

/// Shared with the watcher: turn a set of coalesced events into index changes.
pub fn apply_events(vault: &Vault, events: &[VaultEvent]) -> Result<Vec<String>> {
    let globs = ignore_globs(vault);
    let mut item_ids = Vec::new();
    for event in events {
        if is_ignored(&event.rel_path, &globs) {
            continue;
        }
        let abs = vault.abs_path(&event.rel_path);
        let exists = abs.is_file();

        if !exists {
            if let Some(existing) = latest_version_for_path(vault, &event.rel_path)? {
                vault.conn.execute(
                    "UPDATE file_versions SET missing = 1, updated_at = ?1 WHERE id = ?2",
                    params![now_iso(), existing.id],
                )?;
                vault.audit(
                    "MarkMissing",
                    "FileVersion",
                    Some(&existing.id),
                    None,
                    Some(&format!("{{\"rel_path\":\"{}\"}}", event.rel_path)),
                )?;
                let item = inbox::raise(
                    vault,
                    inbox::NewItem {
                        kind: InboxKind::MissingFile,
                        title: format!("File no longer on disk — {}", event.rel_path),
                        detail: Some("Watched while the app was running. Nothing was deleted.".to_string()),
                        rel_path: Some(event.rel_path.clone()),
                        file_version_id: Some(existing.id),
                        candidate_version_id: None,
                    },
                )?;
                item_ids.extend(item);
            }
            continue;
        }

        let outcome = import_file(vault, &event.rel_path)?;
        if let Some(item) = outcome.item_id {
            item_ids.push(item);
        }
        vault.conn.execute(
            "INSERT INTO file_events (id, novel_id, kind, rel_path, detail, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                new_id(),
                vault.novel_id,
                event.kind.as_str(),
                event.rel_path,
                outcome.detail,
                event.at
            ],
        )?;
    }
    Ok(item_ids)
}

/// Coarse map of file category → display grouping used by the CLI report.
pub fn category_counts(vault: &Vault) -> Result<HashMap<String, usize>> {
    let mut out = HashMap::new();
    let mut stmt = vault
        .conn
        .prepare("SELECT rel_path FROM file_versions WHERE novel_id = ?1 AND deleted_at IS NULL")?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for rel in rows {
        *out.entry(extract::category(&rel).to_string()).or_insert(0) += 1;
    }
    Ok(out)
}
