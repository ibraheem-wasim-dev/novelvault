//! Query and command surface used by both the CLI and the desktop UI.

use std::path::Path;
use std::time::Duration;

use rusqlite::{params, OptionalExtension};

use crate::db::{self, Vault};
use crate::model::*;
use crate::util::{new_id, now_iso, short_hash};
use crate::{Result, VaultError};

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

fn version_from_row(row: &rusqlite::Row<'_>, root: &Path) -> rusqlite::Result<FileVersion> {
    let rel_path: String = row.get("rel_path")?;
    let sha: String = row.get("sha256")?;
    let status_raw: String = row.get("file_status")?;
    let status = FileStatus::parse(&status_raw).unwrap_or(FileStatus::Unknown);
    Ok(FileVersion {
        id: row.get("id")?,
        file_id: row.get("file_id")?,
        family_name: row.get("family_name")?,
        family_confirmed: row.get::<_, i64>("family_confirmed")? == 1,
        version_number: row.get("version_number")?,
        sha256_short: short_hash(&sha),
        sha256: sha,
        rel_path: rel_path.clone(),
        size_bytes: row.get("size_bytes")?,
        mtime: row.get("mtime")?,
        extraction_state: row.get("extraction_state")?,
        extraction_note: row.get("extraction_note")?,
        extracted_chars: row.get("extracted_chars")?,
        file_status: status.as_str().to_string(),
        status_marker: status.marker().to_string(),
        arc_label: row.get("arc_label")?,
        chapter_label: row.get("chapter_label")?,
        version_label: row.get("version_label")?,
        is_current: row.get::<_, i64>("is_current")? == 1,
        missing: row.get::<_, i64>("missing")? == 1,
        last_seen_at: row.get("last_seen_at")?,
        updated_at: row.get("updated_at")?,
        abs_path: root.join(rel_path).display().to_string(),
    })
}

const VERSION_COLS: &str = "v.id, v.file_id, v.version_number, v.rel_path, v.sha256, v.size_bytes,
    v.mtime, v.extraction_state, v.extraction_note, v.extracted_chars, v.file_status,
    v.arc_label, v.chapter_label, v.version_label, v.is_current, v.missing, v.last_seen_at,
    v.updated_at, f.family_name, f.family_confirmed";

pub fn list_versions(vault: &Vault, include_missing: bool) -> Result<Vec<FileVersion>> {
    let mut stmt = vault.conn.prepare(&format!(
        "SELECT {VERSION_COLS} FROM file_versions v
         JOIN files f ON f.id = v.file_id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL AND (?2 = 1 OR v.missing = 0)
         ORDER BY f.family_name, v.version_number"
    ))?;
    let rows = stmt
        .query_map(params![vault.novel_id, include_missing as i64], |r| {
            version_from_row(r, vault.root.as_path())
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_version(vault: &Vault, version_id: &str) -> Result<FileVersion> {
    vault
        .conn
        .query_row(
            &format!(
                "SELECT {VERSION_COLS} FROM file_versions v
                 JOIN files f ON f.id = v.file_id WHERE v.id = ?1"
            ),
            params![version_id],
            |r| version_from_row(r, vault.root.as_path()),
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("file version {version_id}")))
}

/// Set classification fields. Selecting `CURRENT` goes through `set_current`, which
/// clears the previous CURRENT inside one transaction.
pub fn classify(
    vault: &Vault,
    version_id: &str,
    status: Option<FileStatus>,
    arc: Option<&str>,
    chapter: Option<&str>,
    version_label: Option<&str>,
) -> Result<FileVersion> {
    if let Some(FileStatus::Current) = status {
        set_current(vault, version_id)?;
    }
    let now = now_iso();
    if let Some(status) = status {
        if status != FileStatus::Current {
            vault.conn.execute(
                "UPDATE file_versions SET file_status = ?1, status_source = 'author', updated_at = ?2 WHERE id = ?3",
                params![status.as_str(), now, version_id],
            )?;
        }
    }
    if let Some(arc) = arc {
        vault.conn.execute(
            "UPDATE file_versions SET arc_label = ?1, updated_at = ?2 WHERE id = ?3",
            params![arc, now, version_id],
        )?;
    }
    if let Some(chapter) = chapter {
        vault.conn.execute(
            "UPDATE file_versions SET chapter_label = ?1, updated_at = ?2 WHERE id = ?3",
            params![chapter, now, version_id],
        )?;
    }
    if let Some(label) = version_label {
        vault.conn.execute(
            "UPDATE file_versions SET version_label = ?1, updated_at = ?2 WHERE id = ?3",
            params![label, now, version_id],
        )?;
    }
    vault.audit(
        "Classify",
        "FileVersion",
        Some(version_id),
        None,
        Some(&format!(
            "{{\"status\":{:?},\"arc\":{:?},\"chapter\":{:?},\"version_label\":{:?}}}",
            status.map(|s| s.as_str()),
            arc,
            chapter,
            version_label
        )),
    )?;
    get_version(vault, version_id)
}

/// Marking a version CURRENT is always an explicit author action (R1). Exactly one
/// version per family can hold it; the database enforces that too.
pub fn set_current(vault: &Vault, version_id: &str) -> Result<FileVersion> {
    let (file_id, previous_id): (String, Option<String>) = {
        let file_id: String = vault
            .conn
            .query_row(
                "SELECT file_id FROM file_versions WHERE id = ?1",
                params![version_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| VaultError::NotFound(format!("file version {version_id}")))?;
        // The version being superseded is whatever held the family before: normally the
        // row flagged CURRENT, and otherwise the newest other version in the family.
        let previous: Option<String> = vault
            .conn
            .query_row(
                "SELECT id FROM file_versions
                 WHERE file_id = ?1 AND id <> ?2 AND deleted_at IS NULL
                 ORDER BY is_current DESC, version_number DESC LIMIT 1",
                params![file_id, version_id],
                |r| r.get(0),
            )
            .optional()?;
        (file_id, previous)
    };

    let tx = vault.conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE file_versions SET is_current = 0 WHERE file_id = ?1",
        params![file_id],
    )?;
    tx.execute(
        "UPDATE file_versions
         SET is_current = 1, file_status = 'CURRENT', status_source = 'author', updated_at = ?1
         WHERE id = ?2",
        params![now_iso(), version_id],
    )?;
    if let Some(previous) = previous_id.as_deref() {
        // The superseded version is retained and visibly marked, never removed.
        tx.execute(
            // Superseded versions keep their bytes, their hash, their extracted text and
            // their record — only the label changes, so the archive view can flag them.
            "UPDATE file_versions
             SET file_status = 'HISTORICAL',
                 status_source = 'superseded', updated_at = ?1
             WHERE id = ?2",
            params![now_iso(), previous],
        )?;
    }
    tx.execute(
        "UPDATE files SET family_confirmed = 1, updated_at = ?1 WHERE id = ?2",
        params![now_iso(), file_id],
    )?;
    tx.commit()?;

    vault.audit(
        "SetCurrent",
        "FileVersion",
        Some(version_id),
        previous_id.as_deref(),
        Some(&format!("{{\"file_id\":\"{file_id}\"}}")),
    )?;
    get_version(vault, version_id)
}

/// Archive: keeps the text searchable, marks the file ⚠️ OUTDATED, deletes nothing.
pub fn archive(vault: &Vault, version_id: &str) -> Result<FileVersion> {
    let now = now_iso();
    vault.conn.execute(
        "UPDATE file_versions
         SET file_status = 'HISTORICAL', is_current = 0, status_source = 'author', updated_at = ?1
         WHERE id = ?2",
        params![now, version_id],
    )?;
    vault.audit(
        "Archive",
        "FileVersion",
        Some(version_id),
        None,
        Some("{\"file_status\":\"HISTORICAL\"}"),
    )?;
    get_version(vault, version_id)
}

pub fn note_decision(vault: &Vault, version_id: &str, decision: &str, reason: &str) -> Result<()> {
    vault.audit(
        "Decision",
        "FileVersion",
        Some(version_id),
        None,
        Some(&format!(
            "{{\"decision\":{},\"reason\":{}}}",
            serde_json::to_string(decision)?,
            serde_json::to_string(reason)?
        )),
    )
}

// ---------------------------------------------------------------------------
// Search — filename and extracted text, current and archived alike
// ---------------------------------------------------------------------------

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let hay_chars: Vec<char> = hay.to_lowercase().chars().collect();
    let needle_chars: Vec<char> = needle.to_lowercase().chars().collect();
    if hay_chars.len() < needle_chars.len() {
        return None;
    }
    (0..=(hay_chars.len() - needle_chars.len()))
        .find(|&i| hay_chars[i..i + needle_chars.len()] == needle_chars[..])
}

fn snippet(text: &str, at_char: usize, len: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = at_char.saturating_sub(90);
    let end = (at_char + len + 90).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn search(vault: &Vault, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    let versions = list_versions(vault, true)?;
    let mut hits: Vec<SearchHit> = Vec::new();

    for version in versions {
        let mut matched_in = None;
        let mut text = String::new();

        if find_ci(&version.rel_path, query).is_some() {
            matched_in = Some("filename".to_string());
        }

        if version.extracted_chars > 0 {
            let index_file = db::index_versions_dir(vault.root.as_path()).join(format!("{}.txt", version.id));
            if let Ok(contents) = std::fs::read_to_string(&index_file) {
                if let Some(at) = find_ci(&contents, query) {
                    let matched_in = matched_in.take().unwrap_or_else(|| "content".to_string());
                    let text_owned = snippet(&contents, at, query.chars().count());
                    return_push(&mut hits, &version, matched_in, text_owned);
                    let _ = text;
                    continue;
                }
                text = contents;
            }
        }

        if let Some(matched_in) = matched_in {
            let fallback = if version.missing {
                "record kept — the file is no longer on disk".to_string()
            } else if version.extraction_state == "failed" {
                format!(
                    "no text available ({})",
                    version.extraction_note.clone().unwrap_or_else(|| "extraction failed".into())
                )
            } else if version.extraction_state == "unsupported" {
                "format not extractable".to_string()
            } else {
                String::new()
            };
            let _ = &text;
            return_push(&mut hits, &version, matched_in, fallback);
        }
    }

    hits.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current)
            .then(b.version_number.cmp(&a.version_number))
            .then(a.family_name.cmp(&b.family_name))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn return_push(hits: &mut Vec<SearchHit>, version: &FileVersion, matched_in: String, snippet: String) {
    hits.push(SearchHit {
        file_version_id: version.id.clone(),
        family_name: version.family_name.clone(),
        rel_path: version.rel_path.clone(),
        version_number: version.version_number,
        file_status: version.file_status.clone(),
        status_marker: if version.missing {
            "⛔".to_string()
        } else if version.file_status == "HISTORICAL" {
            "⚠️".to_string()
        } else {
            version.status_marker.clone()
        },
        is_current: version.is_current,
        matched_in,
        snippet,
    });
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------

pub fn dashboard(vault: &Vault) -> Result<Dashboard> {
    let count = |sql: &str| -> Result<i64> {
        let value: i64 = vault
            .conn
            .query_row(sql, params![vault.novel_id], |r| r.get(0))?;
        Ok(value)
    };

    let files = count("SELECT COUNT(*) FROM files WHERE novel_id = ?1 AND deleted_at IS NULL")?;
    let versions = count("SELECT COUNT(*) FROM file_versions WHERE novel_id = ?1 AND deleted_at IS NULL")?;
    let current_versions = count(
        "SELECT COUNT(*) FROM file_versions WHERE novel_id = ?1 AND is_current = 1 AND deleted_at IS NULL",
    )?;
    let unclassified = count(
        "SELECT COUNT(*) FROM file_versions WHERE novel_id = ?1 AND file_status = 'UNKNOWN' AND deleted_at IS NULL",
    )?;
    let missing = count("SELECT COUNT(*) FROM file_versions WHERE novel_id = ?1 AND missing = 1")?;
    let extraction_failed = count(
        "SELECT COUNT(*) FROM file_versions WHERE novel_id = ?1 AND extraction_state IN ('failed','unsupported')",
    )?;
    let snapshots = count("SELECT COUNT(*) FROM snapshots WHERE novel_id = ?1")?;

    let inbox_by_kind = crate::inbox::open_counts(vault)?;
    let inbox_open = inbox_by_kind.values().sum();

    Ok(Dashboard {
        novel: Novel {
            id: vault.novel_id.clone(),
            title: vault.title.clone(),
            created_at: String::new(),
        },
        files,
        versions,
        current_versions,
        unclassified,
        missing,
        extraction_failed,
        inbox_open,
        inbox_by_kind,
        snapshots,
        conflicts: 0,
        ideas: 0,
        counts_note: "conflicts and ideas read tables that arrive in V3/V2; they report 0 honestly rather than guessing."
            .to_string(),
    })
}

// ---------------------------------------------------------------------------
// Portability check
// ---------------------------------------------------------------------------

pub fn open_report(vault: &Vault) -> Result<OpenReport> {
    let versions = list_versions(vault, true)?;
    let mut resolved = 0i64;
    let mut missing = 0i64;
    let mut mismatches = Vec::new();

    for version in &versions {
        let abs = vault.abs_path(&version.rel_path);
        if !abs.is_file() {
            missing += 1;
            continue;
        }
        resolved += 1;
        match crate::util::hash_file(&abs) {
            Ok((sha, _)) => {
                if sha != version.sha256 {
                    mismatches.push(format!("{} (contents changed since import)", version.rel_path));
                }
            }
            Err(e) => mismatches.push(format!("{} ({e})", version.rel_path)),
        }
    }

    Ok(OpenReport {
        novel: Novel {
            id: vault.novel_id.clone(),
            title: vault.title.clone(),
            created_at: String::new(),
        },
        root: vault.root.display().to_string(),
        versions: versions.len() as i64,
        resolved,
        missing,
        hash_mismatches: mismatches,
        portable: true,
    })
}

// ---------------------------------------------------------------------------
// Snapshots — the only irreplaceable layer is the vault itself
// ---------------------------------------------------------------------------

pub fn create_snapshot(
    vault: &Vault,
    name: &str,
    includes_files: bool,
    note: Option<&str>,
) -> Result<SnapshotRecord> {
    let id = new_id();
    let stamp = chrono::Utc::now().format("%Y-%m-%d_%H%M%S").to_string();
    let rel = format!("snapshots/{stamp}_{}.db", crate::util::slugify(name));
    // Safe Mode: the snapshot destination is inside .novelvault/ by construction.
    db::ensure_inside_vault(vault.root.as_path(), &db::vault_dir(vault.root.as_path()).join(&rel))?;

    let dest = db::vault_dir(vault.root.as_path()).join(&rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dest_str = dest.display().to_string();
    vault
        .conn
        .execute("VACUUM INTO ?1", params![dest_str])?;

    let manifest = serde_json::json!({
        "snapshot_id": id,
        "name": name,
        "novel": vault.title,
        "created_at": now_iso(),
        "includes_files": includes_files,
        "note": note,
        "schema_version": db::SCHEMA_VERSION,
    });
    let manifest_rel = format!("snapshots/{stamp}_{}.json", crate::util::slugify(name));
    db::write_vault_file(vault.root.as_path(), &manifest_rel, manifest.to_string().as_bytes())?;

    vault.conn.execute(
        "INSERT INTO snapshots (id, novel_id, name, created_at, rel_path, includes_files, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, vault.novel_id, name, now_iso(), rel, includes_files as i64, note],
    )?;
    vault.audit("Snapshot", "Novel", Some(&vault.novel_id), None, Some(name))?;

    Ok(SnapshotRecord {
        id,
        name: name.to_string(),
        created_at: now_iso(),
        rel_path: rel,
        includes_files,
        note: note.map(|n| n.to_string()),
    })
}

pub fn list_snapshots(vault: &Vault) -> Result<Vec<SnapshotRecord>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, name, created_at, rel_path, includes_files, note FROM snapshots
         WHERE novel_id = ?1 ORDER BY created_at DESC",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| {
            Ok(SnapshotRecord {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                rel_path: r.get(3)?,
                includes_files: r.get::<_, i64>(4)? == 1,
                note: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Inbox shortcut used by the CLI's resolve command
// ---------------------------------------------------------------------------

pub fn resolve_inbox(
    vault: &Vault,
    item_id: &str,
    action: crate::model::InboxAction,
    res: crate::inbox::Resolution,
) -> Result<InboxItem> {
    crate::inbox::resolve(vault, item_id, action, res)
}

pub fn audit_tail(vault: &Vault, limit: i64) -> Result<Vec<(String, String, String, String)>> {
    vault.audit_tail(limit)
}

/// Convenience for callers that only have a vault path.
pub fn scan_path(root: &Path) -> Result<ScanSummary> {
    let vault = Vault::open(root)?;
    crate::scan::scan(&vault)
}

pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}
