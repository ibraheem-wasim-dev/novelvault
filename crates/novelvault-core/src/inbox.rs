//! The Inbox: the only door into the structured novel.
//!
//! Nothing here is ever applied automatically — an item is raised by the scanner or
//! the watcher, and only an explicit author action resolves it. Every resolution is
//! written to the append-only audit log, and a resolved item is never deleted.

use rusqlite::{params, OptionalExtension};

use crate::db::Vault;
use crate::model::*;
use crate::util::{new_id, now_iso};
use crate::{Result, VaultError};

pub struct NewItem {
    pub kind: InboxKind,
    pub title: String,
    pub detail: Option<String>,
    pub rel_path: Option<String>,
    pub file_version_id: Option<String>,
    pub candidate_version_id: Option<String>,
}

/// Raise an item. Returns `None` when an identical open item already exists, so a
/// rescan does not pile up duplicates of the same question.
pub fn raise(vault: &Vault, item: NewItem) -> Result<Option<String>> {
    let id = new_id();
    let now = now_iso();
    let changed = vault.conn.execute(
        "INSERT OR IGNORE INTO inbox_items
         (id, novel_id, kind, title, detail, rel_path, file_version_id, candidate_version_id,
          status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'OPEN', ?9, ?9)",
        params![
            id,
            vault.novel_id,
            item.kind.as_str(),
            item.title,
            item.detail,
            item.rel_path,
            item.file_version_id,
            item.candidate_version_id,
            now
        ],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    vault.audit("InboxRaised", "InboxItem", Some(&id), None, Some(item.kind.label()))?;
    Ok(Some(id))
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<InboxItem> {
    let kind_raw: String = row.get("kind")?;
    let kind = InboxKind::parse(&kind_raw).unwrap_or(InboxKind::NewFile);
    Ok(InboxItem {
        id: row.get("id")?,
        kind: kind.as_str().to_string(),
        kind_label: kind.label().to_string(),
        title: row.get("title")?,
        detail: row.get("detail")?,
        rel_path: row.get("rel_path")?,
        file_version_id: row.get("file_version_id")?,
        candidate_version_id: row.get("candidate_version_id")?,
        status: row.get("status")?,
        created_at: row.get("created_at")?,
        resolved_action: row.get("resolved_action")?,
        resolved_at: row.get("resolved_at")?,
    })
}

const SELECT_COLS: &str = "id, kind, title, detail, rel_path, file_version_id, candidate_version_id,
                           status, created_at, resolved_action, resolved_at";

pub fn get(vault: &Vault, item_id: &str) -> Result<InboxItem> {
    vault
        .conn
        .query_row(
            &format!("SELECT {SELECT_COLS} FROM inbox_items WHERE id = ?1"),
            params![item_id],
            row_to_item,
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("inbox item {item_id}")))
}

pub fn list(vault: &Vault, status: Option<InboxStatus>) -> Result<Vec<InboxItem>> {
    let mut stmt = vault.conn.prepare(&format!(
        "SELECT {SELECT_COLS} FROM inbox_items
         WHERE novel_id = ?1 AND (?2 IS NULL OR status = ?2)
         ORDER BY CASE status WHEN 'OPEN' THEN 0 ELSE 1 END, created_at DESC"
    ))?;
    let status_value = status.map(|s| s.as_str().to_string());
    let rows = stmt
        .query_map(params![vault.novel_id, status_value], row_to_item)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn open_counts(vault: &Vault) -> Result<std::collections::BTreeMap<String, i64>> {
    let mut stmt = vault.conn.prepare(
        "SELECT kind, COUNT(*) FROM inbox_items
         WHERE novel_id = ?1 AND status = 'OPEN' GROUP BY kind",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().collect())
}

/// What the author asked for when resolving an item.
#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub status: Option<FileStatus>,
    pub arc: Option<String>,
    pub chapter: Option<String>,
    pub version_label: Option<String>,
}

/// Resolve an item. Every branch is explicit; nothing else in the vault changes.
pub fn resolve(vault: &Vault, item_id: &str, action: InboxAction, res: Resolution) -> Result<InboxItem> {
    let item = get(vault, item_id)?;
    if item.status != "OPEN" {
        return Err(VaultError::Other(format!(
            "inbox item {item_id} is already {} — nothing to do",
            item.status
        )));
    }
    let kind = InboxKind::parse(&item.kind).unwrap_or(InboxKind::NewFile);
    let now = now_iso();

    match action {
        InboxAction::Ignore => {}

        InboxAction::Classify => {
            let version_id = item
                .file_version_id
                .clone()
                .ok_or_else(|| VaultError::Other("this item has no file version to classify".into()))?;
            crate::store::classify(
                vault,
                &version_id,
                res.status,
                res.arc.as_deref(),
                res.chapter.as_deref(),
                res.version_label.as_deref(),
            )?;
        }

        InboxAction::KeepSeparate => {
            // "These are not the same document." Record the author's decision so the
            // scanner stops proposing it.
            if let Some(version_id) = item.file_version_id.as_deref() {
                let file_id: Option<String> = vault
                    .conn
                    .query_row(
                        "SELECT file_id FROM file_versions WHERE id = ?1",
                        params![version_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(file_id) = file_id {
                    vault.conn.execute(
                        "UPDATE files SET family_confirmed = 1, updated_at = ?1 WHERE id = ?2",
                        params![now, file_id],
                    )?;
                }
            }
        }

        InboxAction::Relink => {
            if kind != InboxKind::PossibleRename {
                return Err(VaultError::Other("only a possible rename can be relinked".into()));
            }
            let candidate = item
                .candidate_version_id
                .clone()
                .ok_or_else(|| VaultError::Other("rename item has no candidate version".into()))?;
            let new_rel = item
                .rel_path
                .clone()
                .ok_or_else(|| VaultError::Other("rename item has no path".into()))?;
            let abs = vault.abs_path(&new_rel);
            let (sha, size) = crate::util::hash_file(&abs)?;
            vault.conn.execute(
                "UPDATE file_versions
                 SET rel_path = ?1, sha256 = ?2, size_bytes = ?3, missing = 0, last_seen_at = ?4, updated_at = ?4
                 WHERE id = ?5",
                params![new_rel, sha, size as i64, now, candidate],
            )?;
            vault.audit(
                "Relink",
                "FileVersion",
                Some(&candidate),
                None,
                Some(&format!("{{\"rel_path\":\"{new_rel}\"}}")),
            )?;
        }

        InboxAction::AcceptChange => {
            let version_id = item
                .file_version_id
                .clone()
                .ok_or_else(|| VaultError::Other("this item has no candidate version".into()))?;
            crate::store::set_current(vault, &version_id)?;
        }

        InboxAction::RejectChange => {
            let version_id = item
                .file_version_id
                .clone()
                .ok_or_else(|| VaultError::Other("this item has no candidate version".into()))?;
            crate::store::classify(vault, &version_id, Some(FileStatus::Rejected), None, None, None)?;
            crate::store::note_decision(
                vault,
                &version_id,
                "rejected",
                "The author rejected this change. The file is kept and marked REJECTED; the previous version stays CURRENT.",
            )?;
        }
    }

    vault.conn.execute(
        "UPDATE inbox_items SET status = ?1, resolved_action = ?2, resolved_at = ?3, updated_at = ?3
         WHERE id = ?4",
        params![
            if action == InboxAction::Ignore { "IGNORED" } else { "RESOLVED" },
            action.as_str(),
            now,
            item_id
        ],
    )?;
    vault.audit(
        "InboxResolved",
        "InboxItem",
        Some(item_id),
        Some(&item.kind),
        Some(action.as_str()),
    )?;

    get(vault, item_id)
}

/// Merge one family into another, renumbering versions so history stays linear.
/// The emptied family row is soft-deleted, never removed (R2).
pub fn merge_family(vault: &Vault, from_file_id: &str, into_file_id: &str) -> Result<()> {
    if from_file_id == into_file_id {
        return Err(VaultError::Other("cannot merge a family into itself".into()));
    }
    let tx = vault.conn.unchecked_transaction()?;
    let next: i64 = tx.query_row(
        "SELECT COALESCE(MAX(version_number), 0) FROM file_versions WHERE file_id = ?1",
        params![into_file_id],
        |r| r.get(0),
    )?;
    let moved: Vec<(String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, rel_path FROM file_versions WHERE file_id = ?1 ORDER BY version_number",
        )?;
        let rows = stmt
            .query_map(params![from_file_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (index, (version_id, _)) in moved.iter().enumerate() {
        let number = next + index as i64 + 1;
        tx.execute(
            "UPDATE file_versions SET file_id = ?1, version_number = ?2, updated_at = ?3 WHERE id = ?4",
            params![into_file_id, number, now_iso(), version_id],
        )?;
    }
    tx.execute(
        "UPDATE files SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
        params![now_iso(), from_file_id],
    )?;
    tx.execute(
        "UPDATE files SET family_confirmed = 1, updated_at = ?1 WHERE id = ?2",
        params![now_iso(), into_file_id],
    )?;
    tx.commit()?;

    vault.audit(
        "MergeFamily",
        "File",
        Some(from_file_id),
        None,
        Some(&format!(
            "{{\"into\":\"{into_file_id}\",\"versions_moved\":{}}}",
            moved.len()
        )),
    )?;
    Ok(())
}
