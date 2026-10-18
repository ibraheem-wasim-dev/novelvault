//! V2 — story records: arcs, chapters, scenes, characters, locations, objects, notes
//! and ideas.
//!
//! Two rules are enforced here rather than in the UI:
//!
//! * **R4** — every novel carries exactly one reader sentinel, created with the novel
//!   and protected by a filtered unique index.
//! * **Duplicates are rejected by the database**, not just by a validation message:
//!   `(novel, arc number)` and `(arc, chapter number)` are unique, with soft-deleted
//!   rows excluded from the constraint.

use rusqlite::{params, OptionalExtension};

use crate::db::Vault;
use crate::model::*;
use crate::util::{new_id, now_iso};
use crate::{Result, VaultError};

/// Human-readable name of the row that stands for the reader.
pub const READER_SENTINEL_NAME: &str = "Reader";

// ---------------------------------------------------------------------------
// Arcs
// ---------------------------------------------------------------------------

pub fn create_arc(
    vault: &Vault,
    number: i64,
    title: &str,
    purpose: Option<&str>,
    status: WorkStatus,
) -> Result<Arc> {
    let now = now_iso();
    let id = new_id();
    let existing: Option<String> = vault
        .conn
        .query_row(
            "SELECT id FROM arcs WHERE novel_id = ?1 AND number = ?2 AND deleted_at IS NULL",
            params![vault.novel_id, number],
            |r| r.get(0),
        )
        .optional()?;
    if existing.is_some() {
        return Err(VaultError::Other(format!(
            "Arc {number} already exists in this novel. Arc numbers must be unique."
        )));
    }
    vault.conn.execute(
        "INSERT INTO arcs (id, novel_id, number, title, purpose, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![id, vault.novel_id, number, title, purpose, status.as_str(), now],
    )?;
    vault.audit("CreateArc", "Arc", Some(&id), None, Some(&format!(
        "{{\"number\":{number},\"title\":{}}}",
        serde_json::to_string(title)?
    )))?;
    get_arc(vault, &id)
}

pub fn update_arc(
    vault: &Vault,
    arc_id: &str,
    title: Option<&str>,
    purpose: Option<&str>,
    summary: Option<&str>,
    status: Option<WorkStatus>,
) -> Result<Arc> {
    let now = now_iso();
    if let Some(title) = title {
        vault.conn.execute(
            "UPDATE arcs SET title = ?1, updated_at = ?2 WHERE id = ?3 AND novel_id = ?4",
            params![title, now, arc_id, vault.novel_id],
        )?;
    }
    if let Some(purpose) = purpose {
        vault.conn.execute(
            "UPDATE arcs SET purpose = ?1, updated_at = ?2 WHERE id = ?3",
            params![purpose, now, arc_id],
        )?;
    }
    if let Some(summary) = summary {
        vault.conn.execute(
            "UPDATE arcs SET summary = ?1, updated_at = ?2 WHERE id = ?3",
            params![summary, now, arc_id],
        )?;
    }
    if let Some(status) = status {
        vault.conn.execute(
            "UPDATE arcs SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now, arc_id],
        )?;
    }
    vault.audit("UpdateArc", "Arc", Some(arc_id), None, None)?;
    get_arc(vault, arc_id)
}

fn arc_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Arc> {
    Ok(Arc {
        id: row.get("id")?,
        novel_id: row.get("novel_id")?,
        number: row.get("number")?,
        title: row.get("title")?,
        purpose: row.get("purpose")?,
        summary: row.get("summary")?,
        status: row.get("status")?,
        chapters: row.get("chapters")?,
        words: row.get("words")?,
    })
}

pub fn get_arc(vault: &Vault, arc_id: &str) -> Result<Arc> {
    vault
        .conn
        .query_row(
            "SELECT a.id, a.novel_id, a.number, a.title, a.purpose, a.summary, a.status,
                    (SELECT COUNT(*) FROM chapters c WHERE c.arc_id = a.id AND c.deleted_at IS NULL) AS chapters,
                    (SELECT COALESCE(SUM(c.word_count), 0) FROM chapters c WHERE c.arc_id = a.id AND c.deleted_at IS NULL) AS words
             FROM arcs a WHERE a.id = ?1 AND a.novel_id = ?2 AND a.deleted_at IS NULL",
            params![arc_id, vault.novel_id],
            arc_from_row,
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("arc {arc_id}")))
}

pub fn list_arcs(vault: &Vault) -> Result<Vec<Arc>> {
    let mut stmt = vault.conn.prepare(
        "SELECT a.id, a.novel_id, a.number, a.title, a.purpose, a.summary, a.status,
                (SELECT COUNT(*) FROM chapters c WHERE c.arc_id = a.id AND c.deleted_at IS NULL) AS chapters,
                (SELECT COALESCE(SUM(c.word_count), 0) FROM chapters c WHERE c.arc_id = a.id AND c.deleted_at IS NULL) AS words
         FROM arcs a WHERE a.novel_id = ?1 AND a.deleted_at IS NULL ORDER BY a.number",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], arc_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Find an arc by its number — convenient for the CLI.
pub fn arc_by_number(vault: &Vault, number: i64) -> Result<Arc> {
    let id: String = vault
        .conn
        .query_row(
            "SELECT id FROM arcs WHERE novel_id = ?1 AND number = ?2 AND deleted_at IS NULL",
            params![vault.novel_id, number],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("arc number {number}")))?;
    get_arc(vault, &id)
}

// ---------------------------------------------------------------------------
// Chapters
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn create_chapter(
    vault: &Vault,
    arc_id: &str,
    number: i64,
    title: &str,
    purpose: Option<&str>,
    emotional_movement: Option<&str>,
    status: WorkStatus,
) -> Result<Chapter> {
    // The arc must belong to this novel.
    let arc_number: i64 = vault
        .conn
        .query_row(
            "SELECT number FROM arcs WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
            params![arc_id, vault.novel_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("arc {arc_id} in this novel")))?;

    let duplicate: Option<String> = vault
        .conn
        .query_row(
            "SELECT id FROM chapters WHERE arc_id = ?1 AND number = ?2 AND deleted_at IS NULL",
            params![arc_id, number],
            |r| r.get(0),
        )
        .optional()?;
    if duplicate.is_some() {
        return Err(VaultError::Other(format!(
            "Chapter {number} already exists in Arc {arc_number}. Chapter numbers must be unique within an arc."
        )));
    }

    let now = now_iso();
    let id = new_id();
    vault.conn.execute(
        "INSERT INTO chapters (id, arc_id, number, title, purpose, emotional_movement, status, word_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?8)",
        params![id, arc_id, number, title, purpose, emotional_movement, status.as_str(), now],
    )?;
    vault.audit("CreateChapter", "Chapter", Some(&id), None, Some(&format!(
        "{{\"arc\":{arc_number},\"number\":{number}}}"
    )))?;
    get_chapter(vault, &id)
}

pub fn update_chapter(
    vault: &Vault,
    chapter_id: &str,
    title: Option<&str>,
    summary: Option<&str>,
    emotional_movement: Option<&str>,
    status: Option<WorkStatus>,
    word_count: Option<i64>,
) -> Result<Chapter> {
    let now = now_iso();
    let fields: [(Option<String>, &str); 5] = [
        (title.map(|v| v.to_string()), "title"),
        (summary.map(|v| v.to_string()), "summary"),
        (emotional_movement.map(|v| v.to_string()), "emotional_movement"),
        (status.map(|v| v.as_str().to_string()), "status"),
        (word_count.map(|v| v.to_string()), "word_count"),
    ];
    for (value, column) in fields {
        if let Some(value) = value {
            // Column names are from this fixed list, never from user input.
            vault.conn.execute(
                &format!("UPDATE chapters SET {column} = ?1, updated_at = ?2 WHERE id = ?3 AND arc_id IN (SELECT id FROM arcs WHERE novel_id = ?4)"),
                params![value, now, chapter_id, vault.novel_id],
            )?;
        }
    }
    vault.audit("UpdateChapter", "Chapter", Some(chapter_id), None, None)?;
    get_chapter(vault, chapter_id)
}

fn chapter_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Chapter> {
    Ok(Chapter {
        id: row.get("id")?,
        arc_id: row.get("arc_id")?,
        arc_number: row.get("arc_number")?,
        number: row.get("number")?,
        title: row.get("title")?,
        purpose: row.get("purpose")?,
        summary: row.get("summary")?,
        emotional_movement: row.get("emotional_movement")?,
        status: row.get("status")?,
        word_count: row.get("word_count")?,
        scenes: row.get("scenes")?,
    })
}

const CHAPTER_SELECT: &str = "SELECT c.id, c.arc_id, a.number AS arc_number, c.number, c.title,
       c.purpose, c.summary, c.emotional_movement, c.status, c.word_count,
       (SELECT COUNT(*) FROM scenes s WHERE s.chapter_id = c.id AND s.deleted_at IS NULL) AS scenes
FROM chapters c JOIN arcs a ON a.id = c.arc_id";

pub fn get_chapter(vault: &Vault, chapter_id: &str) -> Result<Chapter> {
    vault
        .conn
        .query_row(
            &format!("{CHAPTER_SELECT} WHERE c.id = ?1 AND a.novel_id = ?2 AND c.deleted_at IS NULL"),
            params![chapter_id, vault.novel_id],
            chapter_from_row,
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("chapter {chapter_id}")))
}

pub fn list_chapters(vault: &Vault, arc_id: Option<&str>) -> Result<Vec<Chapter>> {
    let mut stmt = vault.conn.prepare(&format!(
        "{CHAPTER_SELECT} WHERE a.novel_id = ?1 AND c.deleted_at IS NULL AND (?2 IS NULL OR c.arc_id = ?2)
         ORDER BY a.number, c.number"
    ))?;
    let rows = stmt
        .query_map(params![vault.novel_id, arc_id], chapter_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn chapter_by_number(vault: &Vault, arc_number: i64, chapter_number: i64) -> Result<Chapter> {
    let id: String = vault
        .conn
        .query_row(
            "SELECT c.id FROM chapters c JOIN arcs a ON a.id = c.arc_id
             WHERE a.novel_id = ?1 AND a.number = ?2 AND c.number = ?3 AND c.deleted_at IS NULL",
            params![vault.novel_id, arc_number, chapter_number],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("Arc {arc_number} Chapter {chapter_number}")))?;
    get_chapter(vault, &id)
}

/// Reorder the planning record. The author's files are untouched by this — it moves
/// chapter numbers, not documents.
pub fn reorder_chapters(vault: &Vault, arc_id: &str, ordered_ids: &[String]) -> Result<()> {
    let tx = vault.conn.unchecked_transaction()?;
    // Park the numbers first so the unique (arc, number) constraint cannot trip.
    for (index, id) in ordered_ids.iter().enumerate() {
        tx.execute(
            "UPDATE chapters SET number = ?1 WHERE id = ?2",
            params![-(index as i64) - 1, id],
        )?;
    }
    for (index, id) in ordered_ids.iter().enumerate() {
        tx.execute(
            "UPDATE chapters SET number = ?1, updated_at = ?2 WHERE id = ?3 AND arc_id = ?4",
            params![index as i64 + 1, now_iso(), id, arc_id],
        )?;
    }
    tx.commit()?;
    vault.audit("ReorderChapters", "Arc", Some(arc_id), None, None)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Scenes
// ---------------------------------------------------------------------------

pub fn create_scene(
    vault: &Vault,
    chapter_id: &str,
    number: i64,
    title: &str,
    purpose: Option<&str>,
) -> Result<Scene> {
    let exists: Option<String> = vault
        .conn
        .query_row(
            "SELECT s.id FROM scenes s JOIN chapters c ON c.id = s.chapter_id
             JOIN arcs a ON a.id = c.arc_id
             WHERE s.chapter_id = ?1 AND s.number = ?2 AND a.novel_id = ?3 AND s.deleted_at IS NULL",
            params![chapter_id, number, vault.novel_id],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_some() {
        return Err(VaultError::Other(format!(
            "Scene {number} already exists in this chapter. Scene numbers must be unique within a chapter."
        )));
    }
    let now = now_iso();
    let id = new_id();
    vault.conn.execute(
        "INSERT INTO scenes (id, chapter_id, number, title, purpose, word_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6)",
        params![id, chapter_id, number, title, purpose, now],
    )?;
    vault.audit("CreateScene", "Scene", Some(&id), None, None)?;
    list_scenes(vault, Some(chapter_id))?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| VaultError::NotFound("newly created scene".into()))
}

pub fn list_scenes(vault: &Vault, chapter_id: Option<&str>) -> Result<Vec<Scene>> {
    let mut stmt = vault.conn.prepare(
        "SELECT s.id, s.chapter_id, s.number, s.title, s.purpose, s.pov_character_id, s.location, s.summary, s.word_count
         FROM scenes s JOIN chapters c ON c.id = s.chapter_id JOIN arcs a ON a.id = c.arc_id
         WHERE a.novel_id = ?1 AND s.deleted_at IS NULL AND (?2 IS NULL OR s.chapter_id = ?2)
         ORDER BY a.number, c.number, s.number",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id, chapter_id], |r| {
            Ok(Scene {
                id: r.get(0)?,
                chapter_id: r.get(1)?,
                number: r.get(2)?,
                title: r.get(3)?,
                purpose: r.get(4)?,
                pov_character_id: r.get(5)?,
                location: r.get(6)?,
                summary: r.get(7)?,
                word_count: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Characters — including the reader sentinel (R4)
// ---------------------------------------------------------------------------

/// Create the single reader row for a novel. Called when the novel is created; the
/// unique index makes a second one impossible.
pub fn ensure_reader_sentinel(vault: &Vault) -> Result<Character> {
    let existing: Option<String> = vault
        .conn
        .query_row(
            "SELECT id FROM characters WHERE novel_id = ?1 AND is_reader_sentinel = 1 AND deleted_at IS NULL",
            params![vault.novel_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return get_character(vault, &id);
    }
    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT INTO characters (id, novel_id, name, role, description, is_reader_sentinel, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'Reader', 'Stands for what the reader knows at each point. Not a story character.', 1, ?4, ?4)",
        params![id, vault.novel_id, READER_SENTINEL_NAME, now],
    )?;
    vault.audit("CreateReaderSentinel", "Character", Some(&id), None, None)?;
    get_character(vault, &id)
}

pub fn create_character(
    vault: &Vault,
    name: &str,
    role: Option<&str>,
    description: Option<&str>,
) -> Result<Character> {
    let duplicate: Option<String> = vault
        .conn
        .query_row(
            "SELECT id FROM characters WHERE novel_id = ?1 AND name = ?2 AND deleted_at IS NULL",
            params![vault.novel_id, name],
            |r| r.get(0),
        )
        .optional()?;
    if duplicate.is_some() {
        return Err(VaultError::Other(format!(
            "'{name}' already exists in this novel. Character names must be unique."
        )));
    }
    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT INTO characters (id, novel_id, name, role, description, is_reader_sentinel, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6)",
        params![id, vault.novel_id, name, role, description, now],
    )?;
    vault.audit("CreateCharacter", "Character", Some(&id), None, Some(name))?;
    get_character(vault, &id)
}

pub fn get_character(vault: &Vault, character_id: &str) -> Result<Character> {
    vault
        .conn
        .query_row(
            "SELECT id, name, role, description, is_reader_sentinel FROM characters
             WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
            params![character_id, vault.novel_id],
            character_from_row,
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("character {character_id}")))
}

fn character_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Character> {
    Ok(Character {
        id: row.get(0)?,
        name: row.get(1)?,
        role: row.get(2)?,
        description: row.get(3)?,
        is_reader_sentinel: row.get::<_, i64>(4)? == 1,
    })
}

pub fn list_characters(vault: &Vault) -> Result<Vec<Character>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, name, role, description, is_reader_sentinel FROM characters
         WHERE novel_id = ?1 AND deleted_at IS NULL
         ORDER BY is_reader_sentinel, name",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], character_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn character_by_name(vault: &Vault, name: &str) -> Result<Character> {
    let id: String = vault
        .conn
        .query_row(
            "SELECT id FROM characters WHERE novel_id = ?1 AND name = ?2 AND deleted_at IS NULL",
            params![vault.novel_id, name],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("character '{name}'")))?;
    get_character(vault, &id)
}

// ---------------------------------------------------------------------------
// Locations and objects — the wooden bird needs somewhere to live
// ---------------------------------------------------------------------------

pub fn create_location(vault: &Vault, name: &str, description: Option<&str>) -> Result<Location> {
    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT INTO locations (id, novel_id, name, description, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, vault.novel_id, name, description, now],
    )?;
    vault.audit("CreateLocation", "Location", Some(&id), None, Some(name))?;
    Ok(Location { id, name: name.to_string(), description: description.map(|d| d.to_string()) })
}

pub fn list_locations(vault: &Vault) -> Result<Vec<Location>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, name, description FROM locations WHERE novel_id = ?1 AND deleted_at IS NULL ORDER BY name",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| {
            Ok(Location { id: r.get(0)?, name: r.get(1)?, description: r.get(2)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn create_object(vault: &Vault, name: &str, description: Option<&str>) -> Result<StoryObject> {
    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT INTO story_objects (id, novel_id, name, description, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, vault.novel_id, name, description, now],
    )?;
    vault.audit("CreateObject", "Object", Some(&id), None, Some(name))?;
    Ok(StoryObject { id, name: name.to_string(), description: description.map(|d| d.to_string()) })
}

pub fn list_objects(vault: &Vault) -> Result<Vec<StoryObject>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, name, description FROM story_objects WHERE novel_id = ?1 AND deleted_at IS NULL ORDER BY name",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| {
            Ok(StoryObject { id: r.get(0)?, name: r.get(1)?, description: r.get(2)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Notes and links
// ---------------------------------------------------------------------------

pub fn create_note(vault: &Vault, category: NoteCategory, body: &str) -> Result<Note> {
    if body.trim().is_empty() {
        return Err(VaultError::Other("a note needs a body".into()));
    }
    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT INTO notes (id, novel_id, category, body, is_archived, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
        params![id, vault.novel_id, category.as_str(), body.trim(), now],
    )?;
    vault.audit("CreateNote", "Note", Some(&id), None, Some(category.as_str()))?;
    get_note(vault, &id)
}

pub fn link_note(
    vault: &Vault,
    note_id: &str,
    entity_type: &str,
    entity_id: &str,
    link_type: Option<&str>,
) -> Result<NoteLink> {
    let kind = entity_kinds::normalize(entity_type).ok_or_else(|| {
        VaultError::Other(format!(
            "unknown entity type '{entity_type}'. Expected one of: {}",
            entity_kinds::ALL.join(", ")
        ))
    })?;
    // The target must exist in this novel; a link to a foreign novel is refused.
    exists_in_novel(vault, kind, entity_id)?;
    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT OR IGNORE INTO note_links (id, note_id, entity_type, entity_id, link_type, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![id, note_id, kind, entity_id, link_type, now],
    )?;
    vault.audit("LinkNote", "Note", Some(note_id), None, Some(&format!("{kind}:{entity_id}")))?;
    Ok(NoteLink {
        entity_type: kind.to_string(),
        entity_id: entity_id.to_string(),
        link_type: link_type.map(|l| l.to_string()),
    })
}

/// Does this entity exist, and does it belong to the open novel?
pub fn exists_in_novel(vault: &Vault, entity_type: &str, entity_id: &str) -> Result<()> {
    let sql = match entity_type {
        entity_kinds::NOVEL => "SELECT id FROM novels WHERE id = ?1",
        entity_kinds::ARC => "SELECT id FROM arcs WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
        entity_kinds::CHAPTER => {
            "SELECT c.id FROM chapters c JOIN arcs a ON a.id = c.arc_id
             WHERE c.id = ?1 AND a.novel_id = ?2 AND c.deleted_at IS NULL"
        }
        entity_kinds::SCENE => {
            "SELECT s.id FROM scenes s JOIN chapters c ON c.id = s.chapter_id JOIN arcs a ON a.id = c.arc_id
             WHERE s.id = ?1 AND a.novel_id = ?2 AND s.deleted_at IS NULL"
        }
        entity_kinds::CHARACTER => "SELECT id FROM characters WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
        entity_kinds::LOCATION => "SELECT id FROM locations WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
        entity_kinds::OBJECT => "SELECT id FROM story_objects WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
        entity_kinds::NOTE => "SELECT id FROM notes WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
        entity_kinds::IDEA => "SELECT id FROM ideas WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
        entity_kinds::FILE_VERSION => {
            "SELECT id FROM file_versions WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL"
        }
        entity_kinds::CANON_ITEM => "SELECT id FROM canon_items WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
        _ => return Err(VaultError::Other(format!("unknown entity type '{entity_type}'"))),
    };
    let found: Option<String> = if entity_type == entity_kinds::NOVEL {
        vault.conn.query_row(sql, params![entity_id], |r| r.get(0)).optional()?
    } else {
        vault
            .conn
            .query_row(sql, params![entity_id, vault.novel_id], |r| r.get(0))
            .optional()?
    };
    if found.is_none() {
        return Err(VaultError::NotFound(format!(
            "{entity_type} {entity_id} does not exist in this novel"
        )));
    }
    Ok(())
}

fn note_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get("id")?,
        category: row.get("category")?,
        body: row.get("body")?,
        is_archived: row.get::<_, i64>("is_archived")? == 1,
        created_at: row.get("created_at")?,
        links: Vec::new(),
    })
}

pub fn get_note(vault: &Vault, note_id: &str) -> Result<Note> {
    let mut note = vault
        .conn
        .query_row(
            "SELECT id, category, body, is_archived, created_at FROM notes
             WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
            params![note_id, vault.novel_id],
            note_from_row,
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("note {note_id}")))?;
    note.links = links_for_note(vault, note_id)?;
    Ok(note)
}

pub fn links_for_note(vault: &Vault, note_id: &str) -> Result<Vec<NoteLink>> {
    let mut stmt = vault.conn.prepare(
        "SELECT entity_type, entity_id, link_type FROM note_links
         WHERE note_id = ?1 AND deleted_at IS NULL ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map(params![note_id], |r| {
            Ok(NoteLink {
                entity_type: r.get(0)?,
                entity_id: r.get(1)?,
                link_type: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn list_notes(vault: &Vault, category: Option<NoteCategory>, include_archived: bool) -> Result<Vec<Note>> {
    let category_value = category.map(|c| c.as_str().to_string());
    let mut stmt = vault.conn.prepare(
        "SELECT id, category, body, is_archived, created_at FROM notes
         WHERE novel_id = ?1 AND deleted_at IS NULL
           AND (?2 IS NULL OR category = ?2)
           AND (?3 = 1 OR is_archived = 0)
         ORDER BY created_at DESC",
    )?;
    let mut notes = stmt
        .query_map(
            params![vault.novel_id, category_value, include_archived as i64],
            note_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for note in &mut notes {
        note.links = links_for_note(vault, &note.id)?;
    }
    Ok(notes)
}

pub fn archive_note(vault: &Vault, note_id: &str) -> Result<()> {
    vault.conn.execute(
        "UPDATE notes SET is_archived = 1, updated_at = ?1 WHERE id = ?2 AND novel_id = ?3",
        params![now_iso(), note_id, vault.novel_id],
    )?;
    vault.audit("ArchiveNote", "Note", Some(note_id), None, None)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Ideas
// ---------------------------------------------------------------------------

pub fn create_idea(vault: &Vault, text: &str) -> Result<Idea> {
    if text.trim().is_empty() {
        return Err(VaultError::Other("an idea needs text".into()));
    }
    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT INTO ideas (id, novel_id, text, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'UNDECIDED', ?4, ?4)",
        params![id, vault.novel_id, text.trim(), now],
    )?;
    vault.audit("CreateIdea", "Idea", Some(&id), None, None)?;
    get_idea(vault, &id)
}

pub fn update_idea(
    vault: &Vault,
    idea_id: &str,
    status: IdeaStatus,
    reason: Option<&str>,
) -> Result<Idea> {
    vault.conn.execute(
        "UPDATE ideas SET status = ?1, decision_reason = COALESCE(?2, decision_reason),
                          decided_at = ?3, updated_at = ?3
         WHERE id = ?4 AND novel_id = ?5",
        params![status.as_str(), reason, now_iso(), idea_id, vault.novel_id],
    )?;
    vault.audit("UpdateIdea", "Idea", Some(idea_id), None, Some(status.as_str()))?;
    get_idea(vault, idea_id)
}

fn idea_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Idea> {
    Ok(Idea {
        id: row.get(0)?,
        text: row.get(1)?,
        status: row.get(2)?,
        decision_reason: row.get(3)?,
    })
}

pub fn get_idea(vault: &Vault, idea_id: &str) -> Result<Idea> {
    vault
        .conn
        .query_row(
            "SELECT id, text, status, decision_reason FROM ideas
             WHERE id = ?1 AND novel_id = ?2 AND deleted_at IS NULL",
            params![idea_id, vault.novel_id],
            idea_from_row,
        )
        .optional()?
        .ok_or_else(|| VaultError::NotFound(format!("idea {idea_id}")))
}

pub fn list_ideas(vault: &Vault, status: Option<IdeaStatus>) -> Result<Vec<Idea>> {
    let status_value = status.map(|s| s.as_str().to_string());
    let mut stmt = vault.conn.prepare(
        "SELECT id, text, status, decision_reason FROM ideas
         WHERE novel_id = ?1 AND deleted_at IS NULL AND (?2 IS NULL OR status = ?2)
         ORDER BY created_at DESC",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id, status_value], idea_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Counting helpers used by the dashboard
// ---------------------------------------------------------------------------

pub fn counts(vault: &Vault) -> Result<std::collections::BTreeMap<String, i64>> {
    let mut out = std::collections::BTreeMap::new();
    let pairs: [(&str, &str); 6] = [
        ("arcs", "SELECT COUNT(*) FROM arcs WHERE novel_id = ?1 AND deleted_at IS NULL"),
        ("chapters", "SELECT COUNT(*) FROM chapters c JOIN arcs a ON a.id = c.arc_id WHERE a.novel_id = ?1 AND c.deleted_at IS NULL"),
        ("scenes", "SELECT COUNT(*) FROM scenes s JOIN chapters c ON c.id = s.chapter_id JOIN arcs a ON a.id = c.arc_id WHERE a.novel_id = ?1 AND s.deleted_at IS NULL"),
        ("characters", "SELECT COUNT(*) FROM characters WHERE novel_id = ?1 AND deleted_at IS NULL AND is_reader_sentinel = 0"),
        ("notes", "SELECT COUNT(*) FROM notes WHERE novel_id = ?1 AND deleted_at IS NULL AND is_archived = 0"),
        ("ideas", "SELECT COUNT(*) FROM ideas WHERE novel_id = ?1 AND deleted_at IS NULL"),
    ];
    for (key, sql) in pairs {
        let value: i64 = vault.conn.query_row(sql, params![vault.novel_id], |r| r.get(0))?;
        out.insert(key.to_string(), value);
    }
    Ok(out)
}
