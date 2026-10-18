//! Phase 2 + Phase 3 acceptance tests.
//!
//! These test the promises, not the buttons:
//!
//! * R1 — a canon fact can never reach a canon tier on its own.
//! * R2 — changing canon never destroys the previous value.
//! * R3 — a canon fact cannot be promoted without a source.
//! * R4 — reader knowledge and character knowledge never mix.
//!
//! Plus the file-bringing guarantees: nothing is overwritten, a copy is verified
//! before it is trusted, and a move only deletes the original afterwards.

use std::fs;
use std::path::Path;

use novelvault_core::db::Vault;
use novelvault_core::model::*;
use novelvault_core::util::hash_file;
use novelvault_core::{addfiles, canon, store, story};

fn temp_dir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .expect("temp dir")
}

fn write_file(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, body).expect("write");
}

/// A novel folder with two source documents sitting outside it.
fn novel_with_sources() -> (tempfile::TempDir, tempfile::TempDir, Vault) {
    let parent = temp_dir("nv-novel-parent");
    let (root, vault) = addfiles::create_novel(parent.path(), "Saqar", Some("Saqar")).expect("novel");
    let sources = temp_dir("nv-sources");
    write_file(
        &sources.path().join("Arc 2 Revised v2.md"),
        "Azmirah turned the picture frame over in her hands.",
    );
    write_file(
        &sources.path().join("Old Arc 2 notes.txt"),
        "Earlier plan: the keepsake was a picture frame.",
    );
    let _ = root;
    (parent, sources, vault)
}

// ---------------------------------------------------------------------------
// Creating a novel, and browsing files into it
// ---------------------------------------------------------------------------

#[test]
fn creating_a_novel_makes_the_folder_and_the_reader_sentinel() {
    let parent = temp_dir("nv-new-parent");
    let (root, vault) = addfiles::create_novel(parent.path(), "Saqar", Some("Saqar")).expect("create");

    assert!(root.is_dir());
    assert!(root.join(".novelvault").join("vault.db").exists());
    assert_eq!(vault.title, "Saqar");

    // R4: exactly one reader sentinel, and asking twice does not create a second.
    let first = story::ensure_reader_sentinel(&vault).expect("sentinel");
    let second = story::ensure_reader_sentinel(&vault).expect("sentinel again");
    assert_eq!(first.id, second.id);
    assert!(first.is_reader_sentinel);

    let sentinels = story::list_characters(&vault)
        .expect("characters")
        .into_iter()
        .filter(|c| c.is_reader_sentinel)
        .count();
    assert_eq!(sentinels, 1);
}

#[test]
fn a_bad_folder_name_is_refused() {
    let parent = temp_dir("nv-bad-name");
    assert!(addfiles::create_novel(parent.path(), "Arc/2:bad", None).is_err());
    assert!(addfiles::create_novel(parent.path(), "   ", None).is_err());
}

#[test]
fn planning_changes_nothing_and_applying_needs_confirmation() {
    let (_parent, sources, vault) = novel_with_sources();
    let picked = vec![sources.path().display().to_string()];

    let plan = addfiles::plan(&vault, &picked, false).expect("plan");
    assert_eq!(plan.entries.len(), 2, "both files should be proposed");
    assert!(!plan.headline.is_empty());
    // Planning must not have written anything into the novel folder.
    assert!(!vault.root.join("Arc 2 Revised v2.md").exists());

    // Applying without confirmation is refused outright.
    let refused = addfiles::apply(&vault, &picked, false, false);
    assert!(refused.is_err());
    assert!(!vault.root.join("Arc 2 Revised v2.md").exists());
}

#[test]
fn a_confirmed_copy_lands_verified_and_leaves_the_original_alone() {
    let (_parent, sources, vault) = novel_with_sources();
    let picked = vec![sources.path().display().to_string()];

    let outcomes = addfiles::apply(&vault, &picked, false, true).expect("apply");
    assert_eq!(outcomes.len(), 2);
    for outcome in &outcomes {
        assert!(outcome.error.is_none(), "{:?}", outcome.error);
        assert!(outcome.verified);
        assert!(!outcome.source_removed);
    }

    let arrived = vault.root.join("Arc 2 Revised v2.md");
    assert!(arrived.exists(), "the file should be inside the novel folder");
    // Byte-for-byte identical to the source, and the source is still there.
    let (source_hash, _) = hash_file(&sources.path().join("Arc 2 Revised v2.md")).expect("hash");
    let (copy_hash, _) = hash_file(&arrived).expect("hash");
    assert_eq!(source_hash, copy_hash);
    assert!(sources.path().join("Arc 2 Revised v2.md").exists());

    // It was indexed, not just copied: the vault knows about it.
    let versions = store::list_versions(&vault, false).expect("versions");
    assert!(versions.iter().any(|v| v.rel_path.ends_with("Arc 2 Revised v2.md")));
    // Nothing is CURRENT on its own (R1).
    assert!(versions
        .iter()
        .filter(|v| v.rel_path.ends_with("Arc 2 Revised v2.md"))
        .all(|v| !v.is_current));
}

#[test]
fn a_move_removes_the_original_only_after_the_copy_is_verified() {
    let (_parent, sources, vault) = novel_with_sources();
    let picked = vec![sources.path().display().to_string()];

    let outcomes = addfiles::apply(&vault, &picked, true, true).expect("apply");
    assert!(outcomes.iter().all(|o| o.verified && o.source_removed));
    assert!(vault.root.join("Arc 2 Revised v2.md").exists());
    assert!(!sources.path().join("Arc 2 Revised v2.md").exists());
}

#[test]
fn an_existing_file_is_never_overwritten() {
    let (_parent, sources, vault) = novel_with_sources();
    // The author already has a file by that name inside the novel folder.
    write_file(&vault.root.join("Arc 2 Revised v2.md"), "the author's own version");

    let picked = vec![sources.path().display().to_string()];
    let plan = addfiles::plan(&vault, &picked, false).expect("plan");
    let entry = plan
        .entries
        .iter()
        .find(|e| e.file_name == "Arc 2 Revised v2.md")
        .expect("entry");
    assert_ne!(entry.rel_target, "Arc 2 Revised v2.md");
    assert!(entry.note.is_some(), "the plan should explain the rename");

    addfiles::apply(&vault, &picked, false, true).expect("apply");
    let kept = fs::read_to_string(vault.root.join("Arc 2 Revised v2.md")).expect("read");
    assert_eq!(kept, "the author's own version", "the original must be untouched");
    assert!(vault.root.join(&entry.rel_target).exists());
}

#[test]
fn files_already_inside_the_folder_are_skipped_not_duplicated() {
    let (_parent, _sources, vault) = novel_with_sources();
    let inside = vault.root.join("Arc 2 Revised v2.md");
    write_file(&inside, "already here");

    let plan = addfiles::plan(&vault, &[inside.display().to_string()], false).expect("plan");
    assert!(plan.entries.is_empty());
    assert_eq!(plan.skipped.len(), 1);
    assert!(plan.skipped[0].note.as_deref().unwrap_or("").contains("already inside"));
}

// ---------------------------------------------------------------------------
// Story records: duplicates are impossible
// ---------------------------------------------------------------------------

#[test]
fn duplicate_arc_and_chapter_numbers_are_refused_by_the_database() {
    let (_parent, _sources, vault) = novel_with_sources();

    let arc_one = story::create_arc(&vault, 1, "Arc One", None, WorkStatus::Developing).expect("arc 1");
    assert!(story::create_arc(&vault, 1, "Arc One again", None, WorkStatus::Planned).is_err());

    let chapter = story::create_chapter(
        &vault,
        &arc_one.id,
        1,
        "Opening",
        Some("establish"),
        None,
        WorkStatus::Developing,
    )
    .expect("chapter");
    assert!(story::create_chapter(&vault, &arc_one.id, 1, "Again", None, None, WorkStatus::Planned).is_err());

    // The same chapter number in a different arc is fine.
    let arc_two = story::create_arc(&vault, 2, "Arc Two", None, WorkStatus::Planned).expect("arc 2");
    story::create_chapter(&vault, &arc_two.id, 1, "Another opening", None, None, WorkStatus::Planned)
        .expect("chapter 1 of arc 2");

    let listed = story::list_chapters(&vault, Some(&arc_one.id)).expect("chapters");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, chapter.id);
    assert_eq!(listed[0].arc_number, 1);
}

#[test]
fn chapters_can_be_reordered_without_touching_any_file() {
    let (_parent, _sources, vault) = novel_with_sources();
    let arc = story::create_arc(&vault, 1, "Arc One", None, WorkStatus::Developing).expect("arc");
    let first = story::create_chapter(&vault, &arc.id, 1, "One", None, None, WorkStatus::Developing).expect("c1");
    let second = story::create_chapter(&vault, &arc.id, 2, "Two", None, None, WorkStatus::Planned).expect("c2");

    story::reorder_chapters(&vault, &arc.id, &[second.id.clone(), first.id.clone()]).expect("reorder");

    let listed = story::list_chapters(&vault, Some(&arc.id)).expect("chapters");
    assert_eq!(listed[0].id, second.id);
    assert_eq!(listed[1].id, first.id);
}

#[test]
fn notes_and_ideas_keep_their_vocabularies() {
    let (_parent, _sources, vault) = novel_with_sources();
    let note = story::create_note(&vault, NoteCategory::Continuity, "The bird is wooden, not framed.")
        .expect("note");
    assert_eq!(note.category, "Continuity");
    assert!(story::create_note(&vault, NoteCategory::Quick, "   ").is_err());

    let arc = story::create_arc(&vault, 1, "Arc One", None, WorkStatus::Developing).expect("arc");
    story::link_note(&vault, &note.id, "Arc", &arc.id, Some("about")).expect("link");
    let reloaded = story::get_note(&vault, &note.id).expect("note");
    assert_eq!(reloaded.links.len(), 1);
    // A link to something that is not in this novel is refused.
    assert!(story::link_note(&vault, &note.id, "Arc", "no-such-arc", None).is_err());

    let idea = story::create_idea(&vault, "What if Lucian was in the camp too?").expect("idea");
    assert_eq!(idea.status, "UNDECIDED");
    let decided = story::update_idea(&vault, &idea.id, IdeaStatus::UnderReview, Some("test it in Arc 3"))
        .expect("update");
    assert_eq!(decided.status, "UNDER_REVIEW");
}

// ---------------------------------------------------------------------------
// Canon: R1, R2, R3
// ---------------------------------------------------------------------------

/// A working canon fact about the keepsake, sourced from a file version.
fn ready_novel() -> (tempfile::TempDir, tempfile::TempDir, Vault, FileVersion) {
    let (parent, sources, vault) = novel_with_sources();
    let picked = vec![sources.path().display().to_string()];
    addfiles::apply(&vault, &picked, false, true).expect("apply");
    let version = store::list_versions(&vault, false)
        .expect("versions")
        .into_iter()
        .find(|v| v.rel_path.ends_with("Arc 2 Revised v2.md"))
        .expect("the brought-in file");
    store::set_current(&vault, &version.id).expect("mark current");
    let current = store::get_version(&vault, &version.id).expect("reload");
    (parent, sources, vault, current)
}

#[test]
fn a_canon_fact_starts_as_proposed_and_never_promotes_itself() {
    let (_parent, _sources, vault, _version) = ready_novel();

    let item = canon::propose(&vault, "Keepsake", "picture frame", Some("object"), None, None, None)
        .expect("propose");
    assert_eq!(item.status, "PROPOSED", "R1: nothing becomes canon on its own");
    assert!(item.sources.is_empty());

    let live = canon::list(&vault, None, false).expect("list");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].status, "PROPOSED");
}

#[test]
fn promotion_without_a_source_is_refused_with_a_reason() {
    let (_parent, _sources, vault, _version) = ready_novel();
    let item = canon::propose(&vault, "Keepsake", "picture frame", None, None, None, None).expect("propose");

    let refused = canon::promote(&vault, &item.id, CanonStatus::Canon, "author said so");
    assert!(refused.is_err(), "R3 must block a canon fact with no source");
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("R3"), "the error should name the rule: {message}");
    assert_eq!(canon::get(&vault, &item.id).expect("reload").status, "PROPOSED");

    // An empty reason is refused too — the history has to say why.
    assert!(canon::promote(&vault, &item.id, CanonStatus::Canon, "  ").is_err());
}

#[test]
fn with_a_source_the_fact_can_be_promoted_and_locked() {
    let (_parent, _sources, vault, version) = ready_novel();
    let item = canon::propose(&vault, "Keepsake", "picture frame", None, None, None, None).expect("propose");

    canon::add_source(
        &vault,
        &item.id,
        "FileVersion",
        &version.id,
        Some(&version.family_name),
        Some("Arc 2"),
    )
    .expect("source");

    let canon_item = canon::promote(&vault, &item.id, CanonStatus::Canon, "author confirmed in review")
        .expect("promote to canon");
    assert_eq!(canon_item.status, "CANON");
    assert_eq!(canon_item.sources.len(), 1);

    let locked = canon::promote(&vault, &item.id, CanonStatus::LockedCanon, "Arc 2 is frozen")
        .expect("lock");
    assert_eq!(locked.status, "LOCKED_CANON");
    assert_eq!(locked.status_label, "LOCKED CANON");

    let history = canon::history_for(&vault, &item.id).expect("history");
    assert!(history.len() >= 3, "proposal and both promotions are recorded");
    assert!(history.iter().any(|row| row.action == "Promoted" && row.reason.is_some()));
}

#[test]
fn changing_canon_preserves_the_previous_value_forever() {
    let (_parent, _sources, vault, version) = ready_novel();
    let item = canon::propose(&vault, "Keepsake", "picture frame", None, None, None, None).expect("propose");
    canon::add_source(&vault, &item.id, "FileVersion", &version.id, None, None).expect("source");
    canon::promote(&vault, &item.id, CanonStatus::LockedCanon, "locked").expect("lock");

    // A blank or unchanged value is not a change.
    assert!(canon::change_value(&vault, &item.id, "  ", "why", None).is_err());
    assert!(canon::change_value(&vault, &item.id, "picture frame", "why", None).is_err());

    let changed = canon::change_value(
        &vault,
        &item.id,
        "wooden bird",
        "author revised: the keepsake is a wooden bird",
        None,
    )
    .expect("change");
    assert_eq!(changed.value, "wooden bird");
    assert_eq!(changed.subject, "Keepsake");
    assert_eq!(changed.status, "LOCKED_CANON", "the tier travels with the fact");

    // The old row still exists, visibly marked, and points at its successor.
    let old = canon::get(&vault, &item.id).expect("old row");
    assert_eq!(old.value, "picture frame");
    assert_eq!(old.status, "OUTDATED");
    assert!(old.is_superseded);
    assert_eq!(old.superseded_by_id.as_deref(), Some(changed.id.as_str()));

    // Sources travel forward.
    assert_eq!(changed.sources.len(), 1);

    // R2: both values are still readable, in order.
    let with_history = canon::list(&vault, None, true).expect("list all");
    assert_eq!(with_history.len(), 2);
    assert!(with_history.iter().any(|i| i.value == "picture frame"));
    assert!(with_history.iter().any(|i| i.value == "wooden bird"));
    // ...and only one of them is the current truth.
    let live = canon::list(&vault, None, false).expect("live");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].value, "wooden bird");

    let current = canon::current_for_subject(&vault, "Keepsake").expect("current").expect("some");
    assert_eq!(current.value, "wooden bird");
}

#[test]
fn a_rejected_fact_keeps_its_row_and_its_reason() {
    let (_parent, _sources, vault, version) = ready_novel();
    let item = canon::propose(&vault, "Keepsake", "picture frame", None, None, None, None).expect("propose");
    canon::add_source(&vault, &item.id, "FileVersion", &version.id, None, None).expect("source");

    let rejected = canon::reject(&vault, &item.id, "superseded by the wooden bird").expect("reject");
    assert_eq!(rejected.status, "REJECTED");
    assert_eq!(rejected.reason.as_deref(), Some("superseded by the wooden bird"));
    assert_eq!(canon::history_for(&vault, &item.id).expect("history").last().map(|r| r.action.as_str()), Some("Promoted"));
}

// ---------------------------------------------------------------------------
// Conflicts: raised, never silently resolved
// ---------------------------------------------------------------------------

fn canon_with_picture_frame(vault: &Vault, version: &FileVersion) -> CanonItem {
    let item = canon::propose(vault, "Keepsake", "picture frame", Some("object"), None, None, None)
        .expect("propose");
    canon::add_source(vault, &item.id, "FileVersion", &version.id, Some(&version.family_name), None)
        .expect("source");
    canon::promote(vault, &item.id, CanonStatus::Canon, "author confirmed").expect("promote")
}

#[test]
fn a_superseded_value_still_in_an_active_file_raises_a_conflict() {
    let (_parent, _sources, vault, version) = ready_novel();
    let item = canon_with_picture_frame(&vault, &version);
    // The active document still says "picture frame".
    assert!(version.is_current, "the test needs an active file");

    canon::change_value(&vault, &item.id, "wooden bird", "author revised the keepsake", None)
        .expect("change");

    let report = canon::probe(&vault, 50).expect("probe");
    assert!(report.conflicts_raised >= 1, "the stale value must be caught: {:?}", report.details);

    let conflicts = canon::conflicts(&vault, Some("OPEN")).expect("conflicts");
    let found = conflicts
        .iter()
        .find(|c| c.subject == "Keepsake")
        .expect("a Keepsake conflict");
    assert_eq!(found.current_value.as_deref(), Some("wooden bird"));
    assert_eq!(found.conflicting_value.as_deref(), Some("picture frame"));
    assert_eq!(found.detector, "superseded_value_in_active_file");
    assert!(found.resolution.is_none(), "the detector must not decide anything");

    // Canon is untouched by the detector: the current fact is still the wooden bird.
    let current = canon::current_for_subject(&vault, "Keepsake").expect("current").expect("some");
    assert_eq!(current.value, "wooden bird");

    // Resolving needs a decision and a reason, and it never edits canon itself.
    assert!(canon::resolve_conflict(&vault, &found.id, "keep_current", "  ").is_err());
    assert!(canon::resolve_conflict(&vault, &found.id, "guess", "reason").is_err());
    canon::resolve_conflict(&vault, &found.id, "keep_current", "the draft is outdated").expect("resolve");
    let resolved = canon::conflicts(&vault, Some("RESOLVED")).expect("resolved");
    assert!(resolved.iter().any(|c| c.id == found.id));
    assert!(canon::conflicts(&vault, Some("OPEN"))
        .expect("open")
        .iter()
        .all(|c| c.id != found.id));

    // Probing again does not raise the same open conflict twice.
    let second = canon::probe(&vault, 50).expect("probe again");
    assert_eq!(second.conflicts_raised, 0, "an already-open question is not asked twice");
}

#[test]
fn two_live_canon_facts_that_disagree_are_surfaced_not_merged() {
    let (_parent, _sources, vault, version) = ready_novel();
    let first = canon_with_picture_frame(&vault, &version);

    let second = canon::propose(&vault, "Keepsake", "wooden bird", None, None, None, None).expect("propose");
    canon::add_source(&vault, &second.id, "FileVersion", &version.id, None, None).expect("source");
    canon::promote(&vault, &second.id, CanonStatus::Canon, "from the revised draft").expect("promote");

    let report = canon::probe(&vault, 50).expect("probe");
    assert!(report.conflicts_raised >= 1);
    let conflicts = canon::conflicts(&vault, Some("OPEN")).expect("conflicts");
    assert!(conflicts
        .iter()
        .any(|c| c.detector == "two_live_canon_facts_disagree"));

    // Neither fact was deleted or demoted by the detector.
    assert_eq!(canon::get(&vault, &first.id).expect("first").status, "CANON");
    assert_eq!(canon::get(&vault, &second.id).expect("second").status, "CANON");
}

// ---------------------------------------------------------------------------
// Knowledge matrix (R4) and impact analysis
// ---------------------------------------------------------------------------

#[test]
fn reader_and_character_knowledge_are_tracked_separately() {
    let (_parent, _sources, vault, version) = ready_novel();
    let item = canon_with_picture_frame(&vault, &version);
    let reader = story::ensure_reader_sentinel(&vault).expect("sentinel");
    let azmirah = story::create_character(&vault, "Azmirah", Some("protagonist"), None).expect("character");

    canon::set_knowledge(&vault, &reader.id, &item.id, KnowledgeState::DoesNotKnow, Some("not revealed yet"))
        .expect("reader row");
    canon::set_knowledge(&vault, &azmirah.id, &item.id, KnowledgeState::Knows, None).expect("character row");

    let rows = canon::knowledge_rows(&vault, Some(&item.id)).expect("rows");
    assert_eq!(rows.len(), 2);
    let reader_row = rows.iter().find(|r| r.character_id == reader.id).expect("reader");
    let character_row = rows.iter().find(|r| r.character_id == azmirah.id).expect("character");
    assert_eq!(reader_row.state, "DOES_NOT_KNOW");
    assert_eq!(character_row.state, "KNOWS");
    assert_eq!(reader_row.glyph, "✗");
    assert_eq!(character_row.glyph, "✓");

    // Updating is an update, not a second row.
    canon::set_knowledge(&vault, &azmirah.id, &item.id, KnowledgeState::Partial, None).expect("update");
    let rows = canon::knowledge_rows(&vault, Some(&item.id)).expect("rows again");
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().find(|r| r.character_id == azmirah.id).expect("character").state,
        "PARTIAL"
    );

    // The matrix spans every fact, so a character's knowledge can be read across the novel.
    let all = canon::knowledge_rows(&vault, None).expect("all rows");
    assert_eq!(all.len(), 2);
}

#[test]
fn the_impact_report_says_what_a_change_would_touch() {
    let (_parent, _sources, vault, version) = ready_novel();
    let item = canon_with_picture_frame(&vault, &version);
    let note = story::create_note(
        &vault,
        NoteCategory::Continuity,
        "Keep the picture frame consistent with the wooden bird decision.",
    )
    .expect("note");
    story::link_note(&vault, &note.id, "CanonItem", &item.id, Some("about")).expect("link");

    let report = canon::impact(&vault, &item.id).expect("impact");
    assert_eq!(report.subject, "Keepsake");
    assert_eq!(report.value, "picture frame");
    assert!(!report.is_locked);
    assert_eq!(report.affected_notes.len(), 1, "the linked note is listed");
    assert!(report.files_mentioning_old_value.iter().any(|f| f.contains("Arc 2 Revised")),
        "the active file containing the value is listed: {:?}",
        report.files_mentioning_old_value);
    assert!(report.summary.contains("Keepsake") || report.summary.contains("picture frame"));
}

#[test]
fn canon_counts_feed_the_dashboard() {
    let (_parent, _sources, vault, version) = ready_novel();
    let item = canon_with_picture_frame(&vault, &version);
    canon::promote(&vault, &item.id, CanonStatus::LockedCanon, "frozen").expect("lock");

    let counts = canon::counts(&vault).expect("counts");
    assert_eq!(counts["canon_items"], 1);
    assert_eq!(counts["canon_locked"], 1);
    assert_eq!(counts["canon_without_source"], 0);
    assert_eq!(counts["canon_conflicts_open"], 0);
}

#[test]
fn an_archived_draft_holding_the_old_value_is_expected_not_a_conflict() {
    let (_parent, _sources, vault, version) = ready_novel();
    let item = canon_with_picture_frame(&vault, &version);
    canon::change_value(&vault, &item.id, "wooden bird", "revised", None).expect("change");

    // The author archives the old draft deliberately: that is why it was archived.
    store::archive(&vault, &version.id).expect("archive");

    let report = canon::probe(&vault, 50).expect("probe");
    assert_eq!(
        report.conflicts_raised, 0,
        "an archived file is history, not a contradiction: {:?}",
        report.details
    );
}

#[test]
fn a_canon_fact_cannot_source_itself_and_foreign_ids_are_refused() {
    let (_parent, _sources, vault, version) = ready_novel();
    let item = canon::propose(&vault, "Keepsake", "picture frame", None, None, None, None).expect("propose");

    assert!(canon::add_source(&vault, &item.id, "CanonItem", &item.id, None, None).is_err());
    assert!(canon::add_source(&vault, &item.id, "Arc", "not-an-arc", None, None).is_err());
    assert!(canon::add_source(&vault, &item.id, "Nonsense", &version.id, None, None).is_err());

    // A different novel's row is not a valid source either.
    let other_parent = temp_dir("nv-other");
    let (_other_root, other_vault) = addfiles::create_novel(other_parent.path(), "Other", None).expect("other");
    let other_arc = story::create_arc(&other_vault, 1, "Arc", None, WorkStatus::Planned).expect("arc");
    assert!(canon::add_source(&vault, &item.id, "Arc", &other_arc.id, None, None).is_err());
}
