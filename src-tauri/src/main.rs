//! Tauri shell. Every command is a thin wrapper over `novelvault_core`; the desktop
//! window therefore has exactly the same guarantees as the CLI — reads only, writes
//! confined to `.novelvault/`, nothing promoted to CURRENT or canon without a click.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use novelvault_core::db::Vault;
use novelvault_core::model::{
    CanonStatus, FileStatus, IdeaStatus, InboxAction, InboxStatus, KnowledgeState, NoteCategory,
    WorkStatus,
};
use novelvault_core::llm::LlmConfig;
use novelvault_core::{addfiles, ai, canon, inbox, library, reader, scan, store, story, watch};
use serde_json::json;
use tauri::{Emitter, Manager, State};

struct AppState {
    vault: Mutex<Option<Vault>>,
    watching: Arc<AtomicBool>,
}

impl AppState {
    fn new() -> Self {
        AppState {
            vault: Mutex::new(None),
            watching: Arc::new(AtomicBool::new(false)),
        }
    }
}

type CmdResult = Result<serde_json::Value, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn with_vault<T, F>(state: &State<'_, AppState>, f: F) -> Result<T, String>
where
    F: FnOnce(&Vault) -> Result<T, String>,
{
    let guard = state.vault.lock().map_err(|_| "vault lock poisoned".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| "No novel is open yet.".to_string())?;
    f(vault)
}

// ---------------------------------------------------------------------------
// Novel handling
// ---------------------------------------------------------------------------

#[tauri::command]
fn open_novel(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    title: Option<String>,
) -> CmdResult {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    let name = title.unwrap_or_else(|| {
        root.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    });
    let vault = Vault::open_or_create(&root, &name).map_err(err)?;
    let report = store::open_report(&vault).map_err(err)?;
    remember(&app, &root, &vault.title);
    let _ = ai::index_new(&vault);
    let _ = reader::crawl(&vault, 5);
    let mut guard = state.vault.lock().map_err(|_| "vault lock poisoned")?;
    *guard = Some(vault);
    Ok(serde_json::to_value(report).map_err(err)?)
}

#[tauri::command]
fn close_novel(state: State<'_, AppState>) -> CmdResult {
    let mut guard = state.vault.lock().map_err(|_| "vault lock poisoned")?;
    *guard = None;
    Ok(json!({ "closed": true }))
}

#[tauri::command]
fn current_novel(state: State<'_, AppState>) -> CmdResult {
    with_vault(&state, |vault| {
        Ok(json!({
            "title": vault.title,
            "root": vault.root.display().to_string(),
            "vault_dir": novelvault_core::db::vault_dir(vault.root.as_path()).display().to_string(),
        }))
    })
}

// ---------------------------------------------------------------------------
// Scan / watch
// ---------------------------------------------------------------------------

#[tauri::command]
fn scan_folder(state: State<'_, AppState>) -> CmdResult {
    with_vault(&state, |vault| {
        let summary = scan::scan(vault).map_err(err)?;
        Ok(serde_json::to_value(summary).map_err(err)?)
    })
}

#[tauri::command]
fn watch_start(app: tauri::AppHandle, state: State<'_, AppState>, quiet_ms: Option<u64>) -> CmdResult {
    if state.watching.swap(true, Ordering::SeqCst) {
        return Ok(json!({ "watching": true, "already": true }));
    }
    let root = with_vault(&state, |vault| Ok(vault.root.clone()))?;
    let flag = state.watching.clone();
    let quiet = quiet_ms.unwrap_or(1500);

    std::thread::spawn(move || {
        let handle = app.clone();
        let stop = flag.clone();
        let outcome = watch::run_watch(&root, quiet, None, move |vault, batch| {
            if !stop.load(Ordering::SeqCst) {
                let _ = scan::apply_events(vault, batch);
                // Keep the librarian's index current as drafts are saved.
                let _ = ai::index_new(vault);
                // …and let the Reader fold the new draft into its understanding of the plot.
                let _ = reader::crawl(vault, 10);
            }
            let payload: Vec<serde_json::Value> = batch
                .iter()
                .map(|e| json!({ "kind": e.kind.as_str(), "relPath": e.rel_path }))
                .collect();
            let _ = handle.emit("vault://changes", payload);
            if !stop.load(Ordering::SeqCst) {
                if let Ok(counts) = inbox::open_counts(vault) {
                    let _ = handle.emit("vault://inbox", counts);
                }
            }
            Ok(())
        });
        if let Err(e) = outcome {
            let _ = app.emit("vault://watch-error", e.to_string());
        }
    });

    Ok(json!({ "watching": true, "quietMs": quiet }))
}

#[tauri::command]
fn watch_stop(state: State<'_, AppState>) -> CmdResult {
    state.watching.store(false, Ordering::SeqCst);
    // The watcher thread notices the flag and exits; the vault itself is untouched.
    Ok(json!({ "watching": false }))
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_versions(state: State<'_, AppState>, include_missing: Option<bool>) -> CmdResult {
    with_vault(&state, |vault| {
        let versions = store::list_versions(vault, include_missing.unwrap_or(false)).map_err(err)?;
        Ok(serde_json::to_value(versions).map_err(err)?)
    })
}

#[tauri::command]
fn list_inbox(state: State<'_, AppState>, include_resolved: Option<bool>) -> CmdResult {
    with_vault(&state, |vault| {
        let status = if include_resolved.unwrap_or(false) {
            None
        } else {
            Some(InboxStatus::Open)
        };
        let items = inbox::list(vault, status).map_err(err)?;
        Ok(serde_json::to_value(items).map_err(err)?)
    })
}

#[tauri::command]
fn dashboard(state: State<'_, AppState>) -> CmdResult {
    with_vault(&state, |vault| {
        let dash = store::dashboard(vault).map_err(err)?;
        Ok(serde_json::to_value(dash).map_err(err)?)
    })
}

#[tauri::command]
fn search_vault(state: State<'_, AppState>, query: String, limit: Option<usize>) -> CmdResult {
    with_vault(&state, |vault| {
        let hits = store::search(vault, &query, limit.unwrap_or(30)).map_err(err)?;
        Ok(serde_json::to_value(hits).map_err(err)?)
    })
}

#[tauri::command]
fn audit_tail(state: State<'_, AppState>, limit: Option<i64>) -> CmdResult {
    with_vault(&state, |vault| {
        let rows = store::audit_tail(vault, limit.unwrap_or(50)).map_err(err)?;
        let payload: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(at, action, entity, id)| json!({ "at": at, "action": action, "entityType": entity, "entityId": id }))
            .collect();
        Ok(serde_json::Value::Array(payload))
    })
}

// ---------------------------------------------------------------------------
// Phase 2 — create a novel folder, and bring files into it by browsing
// ---------------------------------------------------------------------------

#[tauri::command]
fn create_novel(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    parent: String,
    name: String,
    title: Option<String>,
) -> CmdResult {
    let parent_path = PathBuf::from(&parent);
    addfiles::parent_is_usable(&parent_path).map_err(err)?;
    let (root, vault) = addfiles::create_novel(&parent_path, &name, title.as_deref()).map_err(err)?;
    story::ensure_reader_sentinel(&vault).map_err(err)?;
    let report = store::open_report(&vault).map_err(err)?;
    remember(&app, &root, &vault.title);
    let mut guard = state.vault.lock().map_err(|_| "vault lock poisoned")?;
    *guard = Some(vault);
    Ok(json!({
        "report": serde_json::to_value(report).map_err(err)?,
        "root": root.display().to_string(),
        "folder_created": true
    }))
}

/// Free to call: reports exactly what a copy or move would do, changing nothing.
#[tauri::command]
fn plan_add_files(
    state: State<'_, AppState>,
    paths: Vec<String>,
    move_files: Option<bool>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let plan = addfiles::plan(vault, &paths, move_files.unwrap_or(false)).map_err(err)?;
        Ok(serde_json::to_value(plan).map_err(err)?)
    })
}

/// Writes into the novel folder. Refuses without `confirmed`, and never overwrites an
/// existing file — a name clash comes in as a new name instead.
#[tauri::command]
fn apply_add_files(
    state: State<'_, AppState>,
    paths: Vec<String>,
    move_files: Option<bool>,
    confirmed: bool,
) -> CmdResult {
    with_vault(&state, |vault| {
        let outcomes = addfiles::apply(vault, &paths, move_files.unwrap_or(false), confirmed)
            .map_err(err)?;
        Ok(serde_json::to_value(outcomes).map_err(err)?)
    })
}

// ---------------------------------------------------------------------------
// Phase 2 — story records
// ---------------------------------------------------------------------------

#[tauri::command]
fn story_arcs(state: State<'_, AppState>) -> CmdResult {
    with_vault(&state, |vault| {
        Ok(serde_json::to_value(story::list_arcs(vault).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn story_arc_create(
    state: State<'_, AppState>,
    number: i64,
    title: String,
    purpose: Option<String>,
    status: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let status = match status.as_deref() {
            None => WorkStatus::Planned,
            Some(raw) => WorkStatus::parse(raw)
                .ok_or_else(|| format!("unknown status '{raw}'"))?,
        };
        let arc = story::create_arc(vault, number, &title, purpose.as_deref(), status).map_err(err)?;
        Ok(serde_json::to_value(arc).map_err(err)?)
    })
}

#[tauri::command]
fn story_chapters(state: State<'_, AppState>, arc_id: Option<String>) -> CmdResult {
    with_vault(&state, |vault| {
        let rows = story::list_chapters(vault, arc_id.as_deref()).map_err(err)?;
        Ok(serde_json::to_value(rows).map_err(err)?)
    })
}

#[tauri::command]
fn story_chapter_create(
    state: State<'_, AppState>,
    arc_id: String,
    number: i64,
    title: String,
    purpose: Option<String>,
    status: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let status = match status.as_deref() {
            None => WorkStatus::Planned,
            Some(raw) => WorkStatus::parse(raw)
                .ok_or_else(|| format!("unknown status '{raw}'"))?,
        };
        let chapter = story::create_chapter(
            vault,
            &arc_id,
            number,
            &title,
            purpose.as_deref(),
            None,
            status,
        )
        .map_err(err)?;
        Ok(serde_json::to_value(chapter).map_err(err)?)
    })
}

#[tauri::command]
fn story_chapter_update(
    state: State<'_, AppState>,
    chapter_id: String,
    title: Option<String>,
    summary: Option<String>,
    status: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let status = match status.as_deref() {
            None => None,
            Some(raw) => Some(
                WorkStatus::parse(raw).ok_or_else(|| format!("unknown status '{raw}'"))?,
            ),
        };
        let chapter = story::update_chapter(
            vault,
            &chapter_id,
            title.as_deref(),
            summary.as_deref(),
            None,
            status,
            None,
        )
        .map_err(err)?;
        Ok(serde_json::to_value(chapter).map_err(err)?)
    })
}

#[tauri::command]
fn story_reorder_chapters(
    state: State<'_, AppState>,
    arc_id: String,
    ordered_ids: Vec<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        story::reorder_chapters(vault, &arc_id, &ordered_ids).map_err(err)?;
        Ok(json!({ "reordered": true }))
    })
}

#[tauri::command]
fn story_characters(state: State<'_, AppState>) -> CmdResult {
    with_vault(&state, |vault| {
        Ok(serde_json::to_value(story::list_characters(vault).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn story_character_create(
    state: State<'_, AppState>,
    name: String,
    role: Option<String>,
    description: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let character = story::create_character(
            vault,
            &name,
            role.as_deref(),
            description.as_deref(),
        )
        .map_err(err)?;
        Ok(serde_json::to_value(character).map_err(err)?)
    })
}

#[tauri::command]
fn notes_list(
    state: State<'_, AppState>,
    category: Option<String>,
    include_archived: Option<bool>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let category = match category.as_deref() {
            None => None,
            Some(raw) => Some(NoteCategory::parse(raw).ok_or_else(|| {
                format!("unknown note category '{raw}'")
            })?),
        };
        let rows = story::list_notes(vault, category, include_archived.unwrap_or(false)).map_err(err)?;
        Ok(serde_json::to_value(rows).map_err(err)?)
    })
}

#[tauri::command]
fn note_create(
    state: State<'_, AppState>,
    category: String,
    body: String,
    link_to: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let category = NoteCategory::parse(&category)
            .ok_or_else(|| format!("unknown note category '{category}'"))?;
        let note = story::create_note(vault, category, &body).map_err(err)?;
        if let Some(target) = link_to.as_deref() {
            if let Some((kind, id)) = target.split_once(':') {
                story::link_note(vault, &note.id, kind, id, None).map_err(err)?;
            }
        }
        let reloaded = story::get_note(vault, &note.id).map_err(err)?;
        Ok(serde_json::to_value(reloaded).map_err(err)?)
    })
}

#[tauri::command]
fn note_link(
    state: State<'_, AppState>,
    note_id: String,
    entity_type: String,
    entity_id: String,
) -> CmdResult {
    with_vault(&state, |vault| {
        story::link_note(vault, &note_id, &entity_type, &entity_id, None).map_err(err)?;
        Ok(json!({ "linked": true }))
    })
}

#[tauri::command]
fn ideas_list(state: State<'_, AppState>, status: Option<String>) -> CmdResult {
    with_vault(&state, |vault| {
        let status = match status.as_deref() {
            None => None,
            Some(raw) => Some(
                IdeaStatus::parse(raw).ok_or_else(|| format!("unknown idea status '{raw}'"))?,
            ),
        };
        Ok(serde_json::to_value(story::list_ideas(vault, status).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn idea_create(state: State<'_, AppState>, text: String) -> CmdResult {
    with_vault(&state, |vault| {
        Ok(serde_json::to_value(story::create_idea(vault, &text).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn idea_update(
    state: State<'_, AppState>,
    idea_id: String,
    status: String,
    reason: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let status = IdeaStatus::parse(&status)
            .ok_or_else(|| format!("unknown idea status '{status}'"))?;
        let idea = story::update_idea(vault, &idea_id, status, reason.as_deref()).map_err(err)?;
        Ok(serde_json::to_value(idea).map_err(err)?)
    })
}

// ---------------------------------------------------------------------------
// Phase 3 — canon engine
// ---------------------------------------------------------------------------

#[tauri::command]
fn canon_list(
    state: State<'_, AppState>,
    status: Option<String>,
    include_superseded: Option<bool>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let status = match status.as_deref() {
            None => None,
            Some(raw) => {
                Some(CanonStatus::parse(raw).ok_or_else(|| format!("unknown canon status '{raw}'"))?)
            }
        };
        let items = canon::list(vault, status, include_superseded.unwrap_or(false)).map_err(err)?;
        Ok(serde_json::to_value(items).map_err(err)?)
    })
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn canon_propose(
    state: State<'_, AppState>,
    subject: String,
    value: String,
    category: Option<String>,
    entity_type: Option<String>,
    entity_id: Option<String>,
    reason: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let item = canon::propose(
            vault,
            &subject,
            &value,
            category.as_deref(),
            entity_type.as_deref(),
            entity_id.as_deref(),
            reason.as_deref(),
        )
        .map_err(err)?;
        Ok(serde_json::to_value(item).map_err(err)?)
    })
}

#[tauri::command]
fn canon_add_source(
    state: State<'_, AppState>,
    canon_id: String,
    source_type: String,
    source_id: String,
    source_label: Option<String>,
    location: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let source = canon::add_source(
            vault,
            &canon_id,
            &source_type,
            &source_id,
            source_label.as_deref(),
            location.as_deref(),
        )
        .map_err(err)?;
        Ok(serde_json::to_value(source).map_err(err)?)
    })
}

#[tauri::command]
fn canon_promote(
    state: State<'_, AppState>,
    canon_id: String,
    status: String,
    reason: String,
) -> CmdResult {
    with_vault(&state, |vault| {
        let target = CanonStatus::parse(&status)
            .ok_or_else(|| format!("unknown canon status '{status}'"))?;
        let item = canon::promote(vault, &canon_id, target, &reason).map_err(err)?;
        Ok(serde_json::to_value(item).map_err(err)?)
    })
}

#[tauri::command]
fn canon_change_value(
    state: State<'_, AppState>,
    canon_id: String,
    new_value: String,
    reason: String,
) -> CmdResult {
    with_vault(&state, |vault| {
        let item = canon::change_value(vault, &canon_id, &new_value, &reason, None).map_err(err)?;
        Ok(serde_json::to_value(item).map_err(err)?)
    })
}

#[tauri::command]
fn canon_impact(state: State<'_, AppState>, canon_id: String) -> CmdResult {
    with_vault(&state, |vault| {
        Ok(serde_json::to_value(canon::impact(vault, &canon_id).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn canon_history(state: State<'_, AppState>, canon_id: String) -> CmdResult {
    with_vault(&state, |vault| {
        Ok(serde_json::to_value(canon::history_for(vault, &canon_id).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn canon_knowledge(state: State<'_, AppState>, canon_id: Option<String>) -> CmdResult {
    with_vault(&state, |vault| {
        let rows = canon::knowledge_rows(vault, canon_id.as_deref()).map_err(err)?;
        Ok(serde_json::to_value(rows).map_err(err)?)
    })
}

#[tauri::command]
fn canon_set_knowledge(
    state: State<'_, AppState>,
    character_id: String,
    canon_id: String,
    state_value: String,
    note: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let state = KnowledgeState::parse(&state_value)
            .ok_or_else(|| format!("unknown knowledge state '{state_value}'"))?;
        let row = canon::set_knowledge(vault, &character_id, &canon_id, state, note.as_deref())
            .map_err(err)?;
        Ok(serde_json::to_value(row).map_err(err)?)
    })
}

#[tauri::command]
fn canon_probe(state: State<'_, AppState>) -> CmdResult {
    with_vault(&state, |vault| {
        let report = canon::probe(vault, 50).map_err(err)?;
        Ok(json!({
            "conflicts_raised": report.conflicts_raised,
            "subjects_checked": report.subjects_checked,
            "details": report.details
        }))
    })
}

#[tauri::command]
fn canon_conflicts(state: State<'_, AppState>, include_resolved: Option<bool>) -> CmdResult {
    with_vault(&state, |vault| {
        let status = if include_resolved.unwrap_or(false) { None } else { Some("OPEN") };
        Ok(serde_json::to_value(canon::conflicts(vault, status).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn canon_resolve_conflict(
    state: State<'_, AppState>,
    conflict_id: String,
    decision: String,
    reason: String,
) -> CmdResult {
    with_vault(&state, |vault| {
        canon::resolve_conflict(vault, &conflict_id, &decision, &reason).map_err(err)?;
        Ok(json!({ "resolved": true }))
    })
}

#[tauri::command]
fn canon_counts(state: State<'_, AppState>) -> CmdResult {
    with_vault(&state, |vault| {
        Ok(serde_json::to_value(canon::counts(vault).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn list_snapshots(state: State<'_, AppState>) -> CmdResult {
    with_vault(&state, |vault| {
        let snaps = store::list_snapshots(vault).map_err(err)?;
        Ok(serde_json::to_value(snaps).map_err(err)?)
    })
}

// ---------------------------------------------------------------------------
// Author actions — every one of them is an explicit request from the UI
// ---------------------------------------------------------------------------

fn parse_status(raw: Option<String>) -> Result<Option<FileStatus>, String> {
    match raw {
        None => Ok(None),
        Some(value) if value.trim().is_empty() => Ok(None),
        Some(value) => FileStatus::parse(&value).map(Some).ok_or_else(|| {
            format!("unknown file status '{value}'. Expected CURRENT, WORKING_DRAFT, REFERENCE, HISTORICAL, REJECTED or UNKNOWN.")
        }),
    }
}

#[tauri::command]
fn classify(
    state: State<'_, AppState>,
    version_id: String,
    status: Option<String>,
    arc: Option<String>,
    chapter: Option<String>,
    version_label: Option<String>,
) -> CmdResult {
    let status = parse_status(status)?;
    with_vault(&state, |vault| {
        let version = store::classify(
            vault,
            &version_id,
            status,
            arc.as_deref(),
            chapter.as_deref(),
            version_label.as_deref(),
        )
        .map_err(err)?;
        Ok(serde_json::to_value(version).map_err(err)?)
    })
}

#[tauri::command]
fn set_current(state: State<'_, AppState>, version_id: String) -> CmdResult {
    with_vault(&state, |vault| {
        let version = store::set_current(vault, &version_id).map_err(err)?;
        Ok(serde_json::to_value(version).map_err(err)?)
    })
}

#[tauri::command]
fn archive(state: State<'_, AppState>, version_id: String) -> CmdResult {
    with_vault(&state, |vault| {
        let version = store::archive(vault, &version_id).map_err(err)?;
        Ok(serde_json::to_value(version).map_err(err)?)
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn resolve_inbox(
    state: State<'_, AppState>,
    item_id: String,
    action: String,
    status: Option<String>,
    arc: Option<String>,
    chapter: Option<String>,
    version_label: Option<String>,
) -> CmdResult {
    let action = InboxAction::parse(&action)
        .ok_or_else(|| format!("unknown action '{action}'"))?;
    let resolution = inbox::Resolution {
        status: parse_status(status)?,
        arc,
        chapter,
        version_label,
    };
    with_vault(&state, |vault| {
        let item = inbox::resolve(vault, &item_id, action, resolution).map_err(err)?;
        Ok(serde_json::to_value(item).map_err(err)?)
    })
}

#[tauri::command]
fn merge_family(state: State<'_, AppState>, from_file_id: String, into_file_id: String) -> CmdResult {
    with_vault(&state, |vault| {
        inbox::merge_family(vault, &from_file_id, &into_file_id).map_err(err)?;
        Ok(json!({ "merged": true }))
    })
}

#[tauri::command]
fn create_snapshot(
    state: State<'_, AppState>,
    name: String,
    with_files: Option<bool>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let record = store::create_snapshot(vault, &name, with_files.unwrap_or(false), None).map_err(err)?;
        Ok(serde_json::to_value(record).map_err(err)?)
    })
}

// ---------------------------------------------------------------------------
// Library of novels (app-level; stores paths and titles only)
// ---------------------------------------------------------------------------

fn library_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(err)
}

/// Best-effort: a library write failing must never stop a novel from opening.
fn remember(app: &tauri::AppHandle, root: &std::path::Path, title: &str) {
    if let Ok(dir) = library_dir(app) {
        let _ = library::register(&dir, root, title);
    }
}

#[tauri::command]
fn library_list(app: tauri::AppHandle) -> CmdResult {
    let dir = library_dir(&app)?;
    Ok(serde_json::to_value(library::summaries(&dir).map_err(err)?).map_err(err)?)
}

/// Removes the list entry only. The novel folder is never touched.
#[tauri::command]
fn library_forget(app: tauri::AppHandle, path: String) -> CmdResult {
    let dir = library_dir(&app)?;
    let removed = library::forget(&dir, &path).map_err(err)?;
    Ok(json!({ "removed": removed }))
}

// ---------------------------------------------------------------------------
// Local AI librarian
//
// These commands open their own connection to the vault instead of holding the shared
// one, so a slow local model never freezes the rest of the app. The heavy ones are
// async + spawn_blocking so the window stays responsive while the model thinks.
// ---------------------------------------------------------------------------

fn vault_root(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    with_vault(state, |vault| Ok(vault.root.clone()))
}

async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

#[tauri::command]
async fn ai_status(state: State<'_, AppState>) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        Ok(serde_json::to_value(ai::status(&vault).map_err(err)?).map_err(err)?)
    })
    .await
}

#[tauri::command]
async fn ai_configure(
    state: State<'_, AppState>,
    provider: String,
    host: Option<String>,
    port: Option<u16>,
    model: Option<String>,
) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        let cfg = LlmConfig {
            port: port.unwrap_or_else(|| LlmConfig::default_port(&provider)),
            host: host.unwrap_or_else(|| "127.0.0.1".into()),
            model: model.unwrap_or_default(),
            provider,
        };
        ai::save_config(&vault, &cfg).map_err(err)?;
        Ok(serde_json::to_value(ai::status(&vault).map_err(err)?).map_err(err)?)
    })
    .await
}

#[tauri::command]
async fn ai_index(state: State<'_, AppState>) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        Ok(serde_json::to_value(ai::index_new(&vault).map_err(err)?).map_err(err)?)
    })
    .await
}

#[tauri::command]
async fn ai_ask(state: State<'_, AppState>, question: String) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        Ok(serde_json::to_value(ai::ask(&vault, &question).map_err(err)?).map_err(err)?)
    })
    .await
}

#[tauri::command]
fn ai_history(state: State<'_, AppState>, limit: Option<i64>) -> CmdResult {
    with_vault(&state, |vault| {
        Ok(serde_json::to_value(ai::history(vault, limit.unwrap_or(50)).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn ai_clear_history(state: State<'_, AppState>) -> CmdResult {
    with_vault(&state, |vault| {
        let n = ai::clear_history(vault).map_err(err)?;
        Ok(json!({ "cleared": n }))
    })
}

#[tauri::command]
async fn ai_memory(state: State<'_, AppState>) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        Ok(serde_json::to_value(ai::memory(&vault).map_err(err)?).map_err(err)?)
    })
    .await
}

#[tauri::command]
async fn ai_summarize(state: State<'_, AppState>, max_files: Option<usize>) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        let n = ai::summarize_with_model(&vault, max_files.unwrap_or(3)).map_err(err)?;
        Ok(json!({ "summarized": n }))
    })
    .await
}

#[tauri::command]
async fn ai_scan_suggestions(state: State<'_, AppState>) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        let created = ai::scan_suggestions(&vault).map_err(err)?;
        Ok(json!({ "created": created }))
    })
    .await
}

#[tauri::command]
fn ai_suggestions(state: State<'_, AppState>, status: Option<String>) -> CmdResult {
    with_vault(&state, |vault| {
        Ok(serde_json::to_value(ai::list_suggestions(vault, status.as_deref()).map_err(err)?).map_err(err)?)
    })
}

#[tauri::command]
fn ai_decide_suggestion(
    state: State<'_, AppState>,
    id: String,
    decision: String,
    note: Option<String>,
    as_kind: Option<String>,
) -> CmdResult {
    with_vault(&state, |vault| {
        let s = ai::decide_suggestion(vault, &id, &decision, note.as_deref(), as_kind.as_deref()).map_err(err)?;
        Ok(serde_json::to_value(s).map_err(err)?)
    })
}

// ---------------------------------------------------------------------------
// The Reader — crawls drafts in story order and learns the plot as it goes
// ---------------------------------------------------------------------------

#[tauri::command]
async fn reader_crawl(state: State<'_, AppState>, max_files: Option<usize>) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        Ok(serde_json::to_value(reader::crawl(&vault, max_files.unwrap_or(10)).map_err(err)?).map_err(err)?)
    })
    .await
}

#[tauri::command]
async fn reader_understanding(state: State<'_, AppState>) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        Ok(serde_json::to_value(reader::understanding(&vault).map_err(err)?).map_err(err)?)
    })
    .await
}

#[tauri::command]
async fn reader_entity(state: State<'_, AppState>, name: String) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        Ok(serde_json::to_value(reader::entity_profile(&vault, &name).map_err(err)?).map_err(err)?)
    })
    .await
}

#[tauri::command]
async fn reader_deepen(state: State<'_, AppState>, max_files: Option<usize>) -> CmdResult {
    let root = vault_root(&state)?;
    blocking(move || {
        let vault = Vault::open(&root).map_err(err)?;
        let n = reader::deepen_with_model(&vault, max_files.unwrap_or(2)).map_err(err)?;
        Ok(json!({ "deepened": n }))
    })
    .await
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            open_novel,
            close_novel,
            current_novel,
            scan_folder,
            watch_start,
            watch_stop,
            list_versions,
            list_inbox,
            dashboard,
            search_vault,
            audit_tail,
            list_snapshots,
            classify,
            set_current,
            archive,
            resolve_inbox,
            merge_family,
            create_snapshot,
            create_novel,
            plan_add_files,
            apply_add_files,
            story_arcs,
            story_arc_create,
            story_chapters,
            story_chapter_create,
            story_chapter_update,
            story_reorder_chapters,
            story_characters,
            story_character_create,
            notes_list,
            note_create,
            note_link,
            ideas_list,
            idea_create,
            idea_update,
            canon_list,
            canon_propose,
            canon_add_source,
            canon_promote,
            canon_change_value,
            canon_impact,
            canon_history,
            canon_knowledge,
            canon_set_knowledge,
            canon_probe,
            canon_conflicts,
            canon_resolve_conflict,
            canon_counts,
            library_list,
            library_forget,
            ai_status,
            ai_configure,
            ai_index,
            ai_ask,
            ai_history,
            ai_clear_history,
            ai_memory,
            ai_summarize,
            ai_scan_suggestions,
            ai_suggestions,
            ai_decide_suggestion,
            reader_crawl,
            reader_understanding,
            reader_entity,
            reader_deepen
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("NovelVault");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running NovelVault");
}
