//! V3 — the Canon Engine.
//!
//! Three rules are enforced here, in code, not in the UI:
//!
//! * **R1** — nothing becomes canon automatically. Canon items are created as
//!   `PROPOSED`; a canon-level status must be granted by an explicit call with a
//!   reason.
//! * **R2** — nothing is overwritten. Changing a canon fact writes a *new* row, marks
//!   the old one `OUTDATED`, links it with `superseded_by`, and appends history.
//! * **R3** — a canon fact cannot be promoted without at least one source. The check
//!   is in `promote`, so the CLI and the UI both pass through it.
//!
//! The conflict detector never decides anything: it writes a `canon_conflicts` row
//! for a human to resolve.

use rusqlite::{params, OptionalExtension};

use crate::db::Vault;
use crate::model::*;
use crate::util::{new_id, now_iso};
use crate::{store, Result, VaultError};

// ---------------------------------------------------------------------------
// Creating and reading canon items
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn propose(
    vault: &Vault,
    subject: &str,
    value: &str,
    category: Option<&str>,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    note: Option<&str>,
) -> Result<CanonItem> {
    if subject.trim().is_empty() || value.trim().is_empty() {
        return Err(VaultError::Other("a canon item needs a subject and a value".into()));
    }
    let kind = match entity_type {
        None => None,
        Some(raw) => {
            let kind = entity_kinds::normalize(raw)
                .ok_or_else(|| VaultError::Other(format!("unknown entity type '{raw}'")))?;
            if let Some(id) = entity_id {
                crate::story::exists_in_novel(vault, kind, id)?;
            }
            Some(kind)
        }
    };

    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT INTO canon_items
         (id, novel_id, subject, value, category, status, entity_type, entity_id, reason, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'PROPOSED', ?6, ?7, ?8, ?9, ?9)",
        params![
            id,
            vault.novel_id,
            subject.trim(),
            value.trim(),
            category,
            kind,
            entity_id,
            note,
            now
        ],
    )?;
    history(vault, &id, "Proposed", note, None, Some(&format!(
        "{{\"subject\":{},\"value\":{}}}",
        serde_json::to_string(subject)?,
        serde_json::to_string(value)?
    )))?;
    vault.audit("ProposeCanon", "CanonItem", Some(&id), None, Some(subject))?;
    get(vault, &id)
}

fn canon_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CanonItem> {
    let status_raw: String = row.get("status")?;
    let status = CanonStatus::parse(&status_raw).unwrap_or(CanonStatus::Unknown);
    Ok(CanonItem {
        id: row.get("id")?,
        subject: row.get("subject")?,
        value: row.get("value")?,
        category: row.get("category")?,
        status: status.as_str().to_string(),
        status_label: status.label().to_string(),
        entity_type: row.get("entity_type")?,
        entity_id: row.get("entity_id")?,
        reason: row.get("reason")?,
        superseded_by_id: row.get("superseded_by_id")?,
        sources: Vec::new(),
        is_superseded: row.get::<_, Option<String>>("superseded_by_id")?.is_some(),
    })
}

const CANON_COLS: &str = "id, subject, value, category, status, entity_type, entity_id, reason, superseded_by_id";

pub fn get(vault: &Vault, canon_id: &str) -> Result<CanonItem> {
    let mut item = vault
        .conn
        .query_row(
            &format!("SELECT {CANON_COLS} FROM canon_items WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL"),
            params![canon_id, vault.novel_id],
            canon_from_row,
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("canon item {canon_id}")))?;
    item.sources = sources(vault, canon_id)?;
    Ok(item)
}

pub fn list(vault: &Vault, status: Option<CanonStatus>, include_superseded: bool) -> Result<Vec<CanonItem>> {
    let status_value = status.map(|s| s.as_str().to_string());
    let mut stmt = vault.conn.prepare(&format!(
        "SELECT {CANON_COLS} FROM canon_items
         WHERE novel_id = ?1 AND deleted_at IS NULL
           AND (?2 IS NULL OR status = ?2)
           AND (?3 = 1 OR superseded_by_id IS NULL)
         ORDER BY subject, created_at"
    ))?;
    let mut items = stmt
        .query_map(
            params![vault.novel_id, status_value, include_superseded as i64],
            canon_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for item in &mut items {
        item.sources = sources(vault, &item.id)?;
    }
    Ok(items)
}

/// The canon item that currently speaks for a subject, if any: the one that is not
/// superseded and holds the highest canon tier.
pub fn current_for_subject(vault: &Vault, subject: &str) -> Result<Option<CanonItem>> {
    let candidates = list(vault, None, false)?;
    let mut best: Option<CanonItem> = None;
    for item in candidates {
        if item.subject.eq_ignore_ascii_case(subject.trim()) {
            let better = match &best {
                None => true,
                Some(current) => rank(&item.status) > rank(&current.status),
            };
            if better {
                best = Some(item);
            }
        }
    }
    Ok(best)
}

fn rank(status: &str) -> i32 {
    match CanonStatus::parse(status).unwrap_or(CanonStatus::Unknown) {
        CanonStatus::LockedCanon => 6,
        CanonStatus::Canon => 5,
        CanonStatus::WorkingCanon => 4,
        CanonStatus::Draft => 3,
        CanonStatus::Proposed => 2,
        CanonStatus::Unknown => 1,
        CanonStatus::Outdated => 0,
        CanonStatus::Rejected => -1,
    }
}

// ---------------------------------------------------------------------------
// Sources (R3)
// ---------------------------------------------------------------------------

pub fn add_source(
    vault: &Vault,
    canon_id: &str,
    source_type: &str,
    source_id: &str,
    source_label: Option<&str>,
    location: Option<&str>,
) -> Result<CanonSource> {
    let kind = entity_kinds::normalize(source_type)
        .ok_or_else(|| VaultError::Other(format!("unknown source type '{source_type}'")))?;
    if kind == entity_kinds::CANON_ITEM {
        return Err(VaultError::Other(
            "a canon item cannot be its own source".into(),
        ));
    }
    crate::story::exists_in_novel(vault, kind, source_id)?;
    // A source must belong to the same novel; exists_in_novel guarantees that, except
    // for Novel itself which is checked below.
    if kind == entity_kinds::NOVEL && source_id != vault.novel_id {
        return Err(VaultError::NotFound("that novel is not the open one".into()));
    }

    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT OR IGNORE INTO canon_sources
         (id, canon_item_id, source_type, source_id, source_label, location, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, canon_id, kind, source_id, source_label, location, now],
    )?;
    vault.audit("AddCanonSource", "CanonItem", Some(canon_id), None, Some(&format!("{kind}:{source_id}")))?;
    sources(vault, canon_id)?
        .into_iter()
        .find(|s| s.source_type == kind && s.source_id == source_id)
        .ok_or_else(|| VaultError::NotFound("newly added source".into()))
}

pub fn sources(vault: &Vault, canon_id: &str) -> Result<Vec<CanonSource>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, source_type, source_id, source_label, location FROM canon_sources
         WHERE canon_item_id = ?1 AND deleted_at IS NULL ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map(params![canon_id], |r| {
            Ok(CanonSource {
                id: r.get(0)?,
                source_type: r.get(1)?,
                source_id: r.get(2)?,
                source_label: r.get(3)?,
                location: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let _ = vault;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Promotion and change — the only ways a status or value moves
// ---------------------------------------------------------------------------

pub fn promote(
    vault: &Vault,
    canon_id: &str,
    target: CanonStatus,
    reason: &str,
) -> Result<CanonItem> {
    if reason.trim().is_empty() {
        return Err(VaultError::Other(
            "promoting a canon fact requires a reason — it is recorded in the history".into(),
        ));
    }
    let before = get(vault, canon_id)?;

    if target.requires_source() && before.sources.is_empty() {
        return Err(VaultError::Other(format!(
            "R3: '{}' cannot become {} without at least one source. Attach a file version, chapter or note first.",
            before.subject,
            target.label()
        )));
    }

    let now = now_iso();
    vault.conn.execute(
        "UPDATE canon_items SET status = ?1, reason = ?2, updated_at = ?3 WHERE id = ?4 AND novel_id = ?5",
        params![target.as_str(), reason.trim(), now, canon_id, vault.novel_id],
    )?;
    history(
        vault,
        canon_id,
        "Promoted",
        Some(reason.trim()),
        Some(&format!("{{\"status\":\"{}\"}}", before.status)),
        Some(&format!("{{\"status\":\"{}\"}}", target.as_str())),
    )?;
    vault.audit("PromoteCanon", "CanonItem", Some(canon_id), Some(&before.status), Some(target.as_str()))?;
    get(vault, canon_id)
}

/// Change the value of a canon fact. The old row is never overwritten: a new row takes
/// over, the old one is marked OUTDATED and linked forward.
pub fn change_value(
    vault: &Vault,
    canon_id: &str,
    new_value: &str,
    reason: &str,
    keep_status: Option<CanonStatus>,
) -> Result<CanonItem> {
    if reason.trim().is_empty() {
        return Err(VaultError::Other(
            "changing canon requires a reason — it is recorded in the history".into(),
        ));
    }
    if new_value.trim().is_empty() {
        return Err(VaultError::Other("the new value cannot be empty".into()));
    }
    let before = get(vault, canon_id)?;
    if before.value.trim().eq_ignore_ascii_case(new_value.trim()) {
        return Err(VaultError::Other("that is already the current value".into()));
    }

    let target_status = keep_status.unwrap_or(
        CanonStatus::parse(&before.status).unwrap_or(CanonStatus::Proposed),
    );

    let new_id_value = new_id();
    let now = now_iso();
    let tx = vault.conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO canon_items
         (id, novel_id, subject, value, category, status, entity_type, entity_id, reason, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![
            new_id_value,
            vault.novel_id,
            before.subject,
            new_value.trim(),
            before.category,
            target_status.as_str(),
            before.entity_type,
            before.entity_id,
            reason.trim(),
            now
        ],
    )?;
    // Sources travel forward with the fact they support.
    tx.execute(
        "INSERT INTO canon_sources (id, canon_item_id, source_type, source_id, source_label, location, created_at)
         SELECT lower(hex(randomblob(16))), ?1, source_type, source_id, source_label, location, ?2
         FROM canon_sources WHERE canon_item_id = ?3 AND deleted_at IS NULL",
        params![new_id_value, now, canon_id],
    )?;
    // The previous row is retained, visibly marked, and linked to its successor.
    tx.execute(
        "UPDATE canon_items SET status = 'OUTDATED', superseded_by_id = ?1, superseded_at = ?2, updated_at = ?2
         WHERE id = ?3",
        params![new_id_value, now, canon_id],
    )?;
    tx.commit()?;

    history(
        vault,
        canon_id,
        "Superseded",
        Some(reason.trim()),
        Some(&format!("{{\"value\":{}}}", serde_json::to_string(&before.value)?)),
        Some(&format!("{{\"superseded_by\":\"{new_id_value}\"}}")),
    )?;
    history(
        vault,
        &new_id_value,
        "CreatedByChange",
        Some(reason.trim()),
        Some(&format!("{{\"value\":{}}}", serde_json::to_string(&before.value)?)),
        Some(&format!("{{\"value\":{}}}", serde_json::to_string(new_value.trim())?)),
    )?;
    vault.audit(
        "ChangeCanon",
        "CanonItem",
        Some(&new_id_value),
        Some(canon_id),
        Some(new_value.trim()),
    )?;
    get(vault, &new_id_value)
}

pub fn reject(vault: &Vault, canon_id: &str, reason: &str) -> Result<CanonItem> {
    let item = promote(vault, canon_id, CanonStatus::Rejected, reason)?;
    // A rejected fact keeps its row and its sources — it is an audit trail, not trash.
    Ok(item)
}

// ---------------------------------------------------------------------------
// History (append-only)
// ---------------------------------------------------------------------------

fn history(
    vault: &Vault,
    canon_id: &str,
    action: &str,
    reason: Option<&str>,
    before: Option<&str>,
    after: Option<&str>,
) -> Result<()> {
    vault.conn.execute(
        "INSERT INTO canon_history (id, canon_item_id, novel_id, action, reason, before_json, after_json, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![new_id(), canon_id, vault.novel_id, action, reason, before, after, now_iso()],
    )?;
    Ok(())
}

pub fn history_for(vault: &Vault, canon_id: &str) -> Result<Vec<CanonHistoryRow>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, canon_item_id, action, reason, before_json, after_json, at FROM canon_history
         WHERE canon_item_id = ?1 ORDER BY at, rowid",
    )?;
    let rows = stmt
        .query_map(params![canon_id], |r| {
            Ok(CanonHistoryRow {
                id: r.get(0)?,
                canon_item_id: r.get(1)?,
                action: r.get(2)?,
                reason: r.get(3)?,
                before_json: r.get(4)?,
                after_json: r.get(5)?,
                at: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Character knowledge (R4)
// ---------------------------------------------------------------------------

pub fn set_knowledge(
    vault: &Vault,
    character_id: &str,
    canon_id: &str,
    state: KnowledgeState,
    note: Option<&str>,
) -> Result<KnowledgeRow> {
    crate::story::exists_in_novel(vault, entity_kinds::CHARACTER, character_id)?;
    crate::story::exists_in_novel(vault, entity_kinds::CANON_ITEM, canon_id)?;
    let now = now_iso();
    // A SQLite upsert cannot target a *partial* unique index (ours excludes
    // soft-deleted rows), so this is an explicit update-then-insert: exactly one row
    // per (character, fact), and no second row ever appears by accident.
    let updated = vault.conn.execute(
        "UPDATE character_knowledge SET state = ?1, note = ?2, updated_at = ?3
         WHERE character_id = ?4 AND canon_item_id = ?5 AND deleted_at IS NULL",
        params![state.as_str(), note, now, character_id, canon_id],
    )?;
    if updated == 0 {
        vault.conn.execute(
            "INSERT INTO character_knowledge
             (id, novel_id, character_id, canon_item_id, state, note, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![new_id(), vault.novel_id, character_id, canon_id, state.as_str(), note, now],
        )?;
    }
    vault.audit("SetKnowledge", "Character", Some(character_id), None, Some(state.as_str()))?;
    knowledge_rows(vault, Some(canon_id))?
        .into_iter()
        .find(|r| r.character_id == character_id)
        .ok_or_else(|| VaultError::NotFound("newly written knowledge row".into()))
}

pub fn knowledge_rows(vault: &Vault, canon_id: Option<&str>) -> Result<Vec<KnowledgeRow>> {
    let mut stmt = vault.conn.prepare(
        "SELECT k.id, k.character_id, c.name, k.canon_item_id, i.subject, k.state, k.note
         FROM character_knowledge k
         JOIN characters c ON c.id = k.character_id
         JOIN canon_items i ON i.id = k.canon_item_id
         WHERE k.novel_id = ?1 AND k.deleted_at IS NULL AND (?2 IS NULL OR k.canon_item_id = ?2)
         ORDER BY c.is_reader_sentinel DESC, c.name",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id, canon_id], |r| {
            let state_raw: String = r.get(5)?;
            let state = KnowledgeState::parse(&state_raw).unwrap_or(KnowledgeState::Unknown);
            Ok(KnowledgeRow {
                id: r.get(0)?,
                character_id: r.get(1)?,
                character_name: r.get(2)?,
                canon_item_id: r.get(3)?,
                subject: r.get(4)?,
                state: state.as_str().to_string(),
                glyph: state.glyph().to_string(),
                note: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Impact analysis (§29)
// ---------------------------------------------------------------------------

pub fn impact(vault: &Vault, canon_id: &str) -> Result<ImpactReport> {
    let item = get(vault, canon_id)?;
    let status = CanonStatus::parse(&item.status).unwrap_or(CanonStatus::Unknown);

    // Files that still contain the current value, and files that mention the subject.
    let files_with_value: Vec<String> = store::search(vault, &item.value, 50)?
        .into_iter()
        .map(|hit| format!("{} (v{}) — {}", hit.family_name, hit.version_number, hit.file_status))
        .collect();
    let files_with_subject: Vec<String> = store::search(vault, &item.subject, 50)?
        .into_iter()
        .map(|hit| format!("{} (v{}) — {}", hit.family_name, hit.version_number, hit.file_status))
        .collect();

    // Notes whose body mentions either string, plus notes explicitly linked to the item.
    let mut affected_notes: Vec<String> = Vec::new();
    for note in crate::story::list_notes(vault, None, true)? {
        let body = note.body.to_lowercase();
        let linked = note
            .links
            .iter()
            .any(|l| l.entity_type == entity_kinds::CANON_ITEM && l.entity_id == canon_id);
        if linked
            || body.contains(&item.subject.to_lowercase())
            || body.contains(&item.value.to_lowercase())
        {
            affected_notes.push(format!("[{}] {}", note.category, truncate(&note.body, 80)));
        }
    }

    let affected_characters: Vec<String> = knowledge_rows(vault, Some(canon_id))?
        .into_iter()
        .map(|row| format!("{} {}", row.character_name, row.state))
        .collect();

    let affected_conflicts: Vec<String> = conflicts(vault, None)?
        .into_iter()
        .filter(|c| c.subject.eq_ignore_ascii_case(&item.subject) && c.status == "OPEN")
        .map(|c| {
            format!(
                "open conflict: {:?} vs {:?}",
                c.current_value.clone().unwrap_or_default(),
                c.conflicting_value.clone().unwrap_or_default()
            )
        })
        .collect();

    let chapters: Vec<String> = crate::story::list_chapters(vault, None)?
        .into_iter()
        .filter(|c| {
            let haystack = format!(
                "{} {} {}",
                c.title,
                c.summary.clone().unwrap_or_default(),
                c.purpose.clone().unwrap_or_default()
            )
            .to_lowercase();
            haystack.contains(&item.subject.to_lowercase())
                || haystack.contains(&item.value.to_lowercase())
        })
        .map(|c| format!("Arc {} Ch {} — {}", c.arc_number, c.number, c.title))
        .collect();

    let summary = format!(
        "{} note(s), {} character knowledge row(s), {} open conflict(s), {} file(s) containing \"{}\", {} file(s) mentioning \"{}\", {} chapter record(s).",
        affected_notes.len(),
        affected_characters.len(),
        affected_conflicts.len(),
        files_with_value.len(),
        item.value,
        files_with_subject.len(),
        item.subject,
        chapters.len()
    );

    Ok(ImpactReport {
        canon_item_id: item.id,
        subject: item.subject,
        value: item.value,
        status: item.status,
        is_locked: status == CanonStatus::LockedCanon,
        affected_notes,
        affected_characters,
        affected_conflicts,
        files_mentioning_old_value: files_with_value,
        files_mentioning_subject: files_with_subject,
        chapters_mentioning_subject: chapters,
        summary,
    })
}

fn truncate(value: &str, width: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= width {
        return value.to_string();
    }
    let mut out: String = chars[..width.saturating_sub(1)].iter().collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// Conflict detection — it raises rows, it never resolves them
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ProbeReport {
    pub conflicts_raised: usize,
    pub subjects_checked: usize,
    pub details: Vec<String>,
}

/// Two detectors, both conservative:
///
/// 1. **Superseded value still in an active file.** A fact was changed (picture frame →
///    wooden bird) but a file that is CURRENT or WORKING_DRAFT still contains the old
///    value. That is exactly the failure this product exists to catch.
/// 2. **Two live canon facts disagree about one subject.** Neither is superseded and
///    both hold canon-level status with different values.
pub fn probe(vault: &Vault, limit_files: usize) -> Result<ProbeReport> {
    let mut report = ProbeReport { conflicts_raised: 0, subjects_checked: 0, details: Vec::new() };

    // (1) outdated values still present in active documents
    let all = list(vault, None, true)?;
    let outdated: Vec<CanonItem> = all.iter().filter(|i| i.is_superseded).cloned().collect();
    report.subjects_checked = outdated.len();

    for old in &outdated {
        let successor = match old.superseded_by_id.as_deref() {
            Some(id) => get(vault, id)?,
            None => continue,
        };
        let hits = store::search(vault, &old.value, limit_files)?;
        for hit in hits {
            // Only active documents count. An archived draft holding the old value is
            // expected and correct — that is why it was archived.
            let active = hit.is_current || hit.file_status == "WORKING_DRAFT";
            if !active {
                continue;
            }
            // A question already asked about this exact pair is never asked twice —
            // including one the author has already answered.
            let exists: Option<String> = vault
                .conn
                .query_row(
                    "SELECT id FROM canon_conflicts
                     WHERE novel_id = ?1 AND subject = ?2 AND conflicting_source = ?3
                       AND detector = 'superseded_value_in_active_file'",
                    params![vault.novel_id, old.subject, hit.rel_path],
                    |r| r.get(0),
                )
                .optional()?;
            if exists.is_some() {
                continue;
            }
            let id = new_id();
            vault.conn.execute(
                "INSERT INTO canon_conflicts
                 (id, novel_id, subject, current_value, conflicting_value, current_source, conflicting_source,
                  detector, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'superseded_value_in_active_file', 'OPEN', ?8, ?8)",
                params![
                    id,
                    vault.novel_id,
                    old.subject,
                    successor.value,
                    old.value,
                    format!("canon ({})", successor.status_label),
                    hit.rel_path,
                    now_iso()
                ],
            )?;
            vault.audit("ConflictRaised", "CanonConflict", Some(&id), None, Some(&old.subject))?;
            report.conflicts_raised += 1;
            report.details.push(format!(
                "{}: canon says \"{}\", but {} still contains \"{}\"",
                old.subject, successor.value, hit.rel_path, old.value
            ));
        }
    }

    // (2) two live canon facts, one subject, different values
    let live: Vec<CanonItem> = list(vault, None, false)?
        .into_iter()
        .filter(|i| {
            matches!(
                CanonStatus::parse(&i.status).unwrap_or(CanonStatus::Unknown),
                CanonStatus::WorkingCanon | CanonStatus::Canon | CanonStatus::LockedCanon
            )
        })
        .collect();

    for (index, first) in live.iter().enumerate() {
        for second in live.iter().skip(index + 1) {
            if !first.subject.eq_ignore_ascii_case(&second.subject) {
                continue;
            }
            if first.value.trim().eq_ignore_ascii_case(second.value.trim()) {
                continue;
            }
            let exists: Option<String> = vault
                .conn
                .query_row(
                    "SELECT id FROM canon_conflicts
                     WHERE novel_id = ?1 AND subject = ?2
                       AND detector = 'two_live_canon_facts_disagree'
                       AND conflicting_value = ?3 AND current_value = ?4",
                    params![vault.novel_id, first.subject, second.value, first.value],
                    |r| r.get(0),
                )
                .optional()?;
            if exists.is_some() {
                continue;
            }
            let id = new_id();
            vault.conn.execute(
                "INSERT INTO canon_conflicts
                 (id, novel_id, subject, current_value, conflicting_value, current_source, conflicting_source,
                  detector, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'two_live_canon_facts_disagree', 'OPEN', ?8, ?8)",
                params![
                    id,
                    vault.novel_id,
                    first.subject,
                    first.value,
                    second.value,
                    format!("canon {}", first.status_label),
                    format!("canon {}", second.status_label),
                    now_iso()
                ],
            )?;
            vault.audit("ConflictRaised", "CanonConflict", Some(&id), None, Some(&first.subject))?;
            report.conflicts_raised += 1;
            report.details.push(format!(
                "{}: \"{}\" ({}) vs \"{}\" ({})",
                first.subject, first.value, first.status_label, second.value, second.status_label
            ));
        }
    }

    Ok(report)
}

pub fn conflicts(vault: &Vault, status: Option<&str>) -> Result<Vec<CanonConflict>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, subject, current_value, conflicting_value, current_source, conflicting_source,
                detector, status, resolution, created_at
         FROM canon_conflicts
         WHERE novel_id = ?1 AND (?2 IS NULL OR status = ?2)
         ORDER BY CASE status WHEN 'OPEN' THEN 0 ELSE 1 END, created_at DESC",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id, status], |r| {
            Ok(CanonConflict {
                id: r.get(0)?,
                subject: r.get(1)?,
                current_value: r.get(2)?,
                conflicting_value: r.get(3)?,
                current_source: r.get(4)?,
                conflicting_source: r.get(5)?,
                detector: r.get(6)?,
                status: r.get(7)?,
                resolution: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Resolving is a human act with a reason. The detector never chooses.
pub fn resolve_conflict(vault: &Vault, conflict_id: &str, decision: &str, reason: &str) -> Result<()> {
    if reason.trim().is_empty() {
        return Err(VaultError::Other("resolving a conflict requires a reason".into()));
    }
    let allowed = ["keep_current", "accept_change", "defer"];
    if !allowed.contains(&decision) {
        return Err(VaultError::Other(format!(
            "unknown decision '{decision}'. Expected one of: {}",
            allowed.join(", ")
        )));
    }
    vault.conn.execute(
        "UPDATE canon_conflicts SET status = 'RESOLVED', resolution = ?1, resolved_at = ?2, updated_at = ?2
         WHERE id = ?3 AND novel_id = ?4",
        params![format!("{decision}: {reason}"), now_iso(), conflict_id, vault.novel_id],
    )?;
    vault.audit("ResolveConflict", "CanonConflict", Some(conflict_id), None, Some(decision))?;
    // Nothing is changed on the canon side here — a resolution records the author's
    // decision; acting on it is a separate, explicit canon operation.
    Ok(())
}

// ---------------------------------------------------------------------------
// Dashboard counters
// ---------------------------------------------------------------------------

pub fn counts(vault: &Vault) -> Result<std::collections::BTreeMap<String, i64>> {
    let mut out = std::collections::BTreeMap::new();
    let pairs: [(&str, &str); 4] = [
        ("canon_items", "SELECT COUNT(*) FROM canon_items WHERE novel_id = ?1 AND deleted_at IS NULL AND superseded_by_id IS NULL"),
        ("canon_locked", "SELECT COUNT(*) FROM canon_items WHERE novel_id = ?1 AND status = 'LOCKED_CANON' AND superseded_by_id IS NULL"),
        ("canon_conflicts_open", "SELECT COUNT(*) FROM canon_conflicts WHERE novel_id = ?1 AND status = 'OPEN'"),
        ("canon_without_source", "SELECT COUNT(*) FROM canon_items i WHERE i.novel_id = ?1 AND i.deleted_at IS NULL AND i.status IN ('WORKING_CANON','CANON','LOCKED_CANON') AND NOT EXISTS (SELECT 1 FROM canon_sources s WHERE s.canon_item_id = i.id AND s.deleted_at IS NULL)"),
    ];
    for (key, sql) in pairs {
        let value: i64 = vault.conn.query_row(sql, params![vault.novel_id], |r| r.get(0))?;
        out.insert(key.to_string(), value);
    }
    Ok(out)
}
