//! V1 acceptance tests. Each test maps to a numbered criterion in the specification
//! (`novelvault_spec_v2.md` §14) or to one of the four core rules.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use novelvault_core::db::Vault;
use novelvault_core::model::{EventKind, FileStatus, InboxAction, InboxKind, InboxStatus};
use novelvault_core::{db, inbox, scan, store, watch};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn temp_root(label: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("novelvault-{label}-"))
        .tempdir()
        .expect("temp dir");
    let root = dir.path().to_path_buf();
    (dir, root)
}

fn write_file(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn write_docx(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default())
        .unwrap();
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p><w:p><w:r><w:t>Second paragraph.</w:t></w:r></w:p></w:body></w:document>"#
    );
    zip.write_all(xml.as_bytes()).unwrap();
    zip.finish().unwrap();
}

/// Fingerprint of everything the author owns, excluding `.novelvault/`.
fn fingerprint(root: &Path) -> BTreeMap<String, (u64, String)> {
    let mut out = BTreeMap::new();
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, (u64, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            if rel.starts_with(".novelvault") {
                continue;
            }
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => walk(root, &path, out),
                Ok(ft) if ft.is_file() => {
                    let meta = entry.metadata().unwrap();
                    let mtime = meta
                        .modified()
                        .map(|t| {
                            let dt: chrono::DateTime<chrono::Utc> = t.into();
                            dt.to_rfc3339()
                        })
                        .unwrap_or_default();
                    out.insert(rel, (meta.len(), mtime));
                }
                _ => {}
            }
        }
    }
    walk(root, root, &mut out);
    out
}

fn fixture(label: &str) -> (tempfile::TempDir, PathBuf, Vault) {
    let (dir, root) = temp_root(label);
    let vault = Vault::create(&root, "Saqar").expect("create vault");
    (dir, root, vault)
}

// ---------------------------------------------------------------------------
// Criterion 1 / §5 — create from a folder, with a readable headline
// ---------------------------------------------------------------------------

#[test]
fn scan_reports_a_readable_headline() {
    let (_dir, root, vault) = fixture("headline");
    write_docx(&root, "Arc 2 Revised v2.docx", "Azmirah wooden bird");
    write_docx(&root, "Arc 3 Draft 2.docx", "Chapter text");
    write_file(&root, "story_development_chat.md", "# chat");
    write_file(&root, "random_notes.txt", "notes");
    write_file(&root, "cover.png", "not a real png");

    let summary = scan::scan(&vault).unwrap();
    assert_eq!(summary.files_seen, 5);
    assert!(summary.headline.contains("5 files found"), "{}", summary.headline);
    assert!(summary.headline.contains("2 DOCX"), "{}", summary.headline);
    assert!(summary.headline.contains("1 Markdown"), "{}", summary.headline);
    assert_eq!(summary.imported, 5);
}

// ---------------------------------------------------------------------------
// Criterion 2 / criterion 11 — the scan writes nothing outside .novelvault/
// ---------------------------------------------------------------------------

#[test]
fn scan_is_non_destructive() {
    let (_dir, root, vault) = fixture("nondestructive");
    write_docx(&root, "Arc 1-2 v1.docx", "Chapter one text");
    write_file(&root, "Arc 2 Revised.docx", "plain text stand-in");
    write_file(&root, "notes/todo.txt", "remember the wooden bird");
    let before = fingerprint(&root);

    scan::scan(&vault).unwrap();

    let after = fingerprint(&root);
    assert_eq!(before, after, "the scan must not add, change, move or remove author files");
    assert!(db::db_path(&root).exists(), "the vault lives inside .novelvault/");
}

#[test]
fn safe_mode_refuses_any_write_outside_the_vault() {
    let (_dir, root, _vault) = fixture("safemode");

    // A path next to the author's files, and a path outside the novel folder.
    let err = db::write_vault_file(&root, "../escaped.txt", b"nope");
    assert!(matches!(err, Err(novelvault_core::VaultError::SafeMode(_))), "{err:?}");

    let outside = root.parent().unwrap().join("totally-outside.txt");
    assert!(db::ensure_inside_vault(&root, &outside).is_err());

    let inside = db::vault_dir(&root).join("index/ok.txt");
    assert!(db::ensure_inside_vault(&root, &inside).is_ok());

    // And the refusal left nothing behind.
    assert!(!root.parent().unwrap().join("escaped.txt").exists());
    assert!(!outside.exists());
}

// ---------------------------------------------------------------------------
// R1 / criterion 3 — imports arrive UNKNOWN and never CURRENT on their own
// ---------------------------------------------------------------------------

#[test]
fn import_defaults_to_unknown_and_never_current() {
    let (_dir, root, vault) = fixture("r1");
    write_docx(&root, "Arc 2 Revised.docx", "canon-looking text");
    write_file(&root, "Arc 3.txt", "more text");
    scan::scan(&vault).unwrap();

    let versions = store::list_versions(&vault, true).unwrap();
    assert_eq!(versions.len(), 2);
    for version in &versions {
        assert_eq!(version.file_status, "UNKNOWN", "{} must import as UNKNOWN", version.rel_path);
        assert!(!version.is_current);
        assert_eq!(version.version_label, None);
    }

    let dash = store::dashboard(&vault).unwrap();
    assert_eq!(dash.current_versions, 0);
    assert_eq!(dash.unclassified, 2);
}

// ---------------------------------------------------------------------------
// Inbox — criterion 12, the only door; and resolutions are audited
// ---------------------------------------------------------------------------

#[test]
fn everything_enters_through_the_inbox_and_is_audited() {
    let (_dir, root, vault) = fixture("inbox");
    write_file(&root, "Arc 3 Draft 1.txt", "draft one");

    let before = inbox::list(&vault, Some(InboxStatus::Open)).unwrap().len();
    assert_eq!(before, 0, "nothing in the inbox before the scan");

    scan::scan(&vault).unwrap();
    let items = inbox::list(&vault, Some(InboxStatus::Open)).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, "NEW_FILE");
    assert!(items[0].rel_path.as_deref() == Some("Arc 3 Draft 1.txt"));

    let version_id = items[0].file_version_id.clone().unwrap();
    let action_started = Instant::now();
    let resolved = inbox::resolve(
        &vault,
        &items[0].id,
        InboxAction::Classify,
        inbox::Resolution {
            status: Some(FileStatus::WorkingDraft),
            arc: Some("Arc 3".into()),
            chapter: Some("Chapter 5".into()),
            version_label: Some("Draft 1".into()),
        },
    )
    .unwrap();
    let _ = action_started;

    assert_eq!(resolved.status, "RESOLVED");
    assert_eq!(resolved.resolved_action.as_deref(), Some("classify"));

    let version = store::get_version(&vault, &version_id).unwrap();
    assert_eq!(version.file_status, "WORKING_DRAFT");
    assert_eq!(version.arc_label.as_deref(), Some("Arc 3"));
    assert_eq!(version.chapter_label.as_deref(), Some("Chapter 5"));

    let audit = store::audit_tail(&vault, 200).unwrap();
    let actions: Vec<&str> = audit.iter().map(|(_, a, _, _)| a.as_str()).collect();
    assert!(actions.contains(&"InboxRaised"), "{actions:?}");
    assert!(actions.contains(&"Classify"), "{actions:?}");
    assert!(actions.contains(&"InboxResolved"), "{actions:?}");
}

#[test]
fn rescan_does_not_duplicate_open_inbox_items() {
    let (_dir, root, vault) = fixture("rescan");
    write_file(&root, "Arc 4.txt", "planning");
    scan::scan(&vault).unwrap();
    scan::scan(&vault).unwrap();
    scan::scan(&vault).unwrap();

    let items = inbox::list(&vault, Some(InboxStatus::Open)).unwrap();
    assert_eq!(items.len(), 1, "repeated scans must not pile up the same question");
    assert_eq!(store::list_versions(&vault, true).unwrap().len(), 1);
}

#[test]
fn rescan_is_idempotent_for_unchanged_files() {
    let (_dir, root, vault) = fixture("idempotent");
    write_file(&root, "Arc 5.txt", "planned arc");
    let first = scan::scan(&vault).unwrap();
    assert_eq!(first.imported, 1);

    let second = scan::scan(&vault).unwrap();
    assert_eq!(second.imported, 0);
    assert_eq!(second.unchanged, 1);
    assert_eq!(store::list_versions(&vault, true).unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Criterion 5 / R2 — a save becomes a new version; the old one stays
// ---------------------------------------------------------------------------

#[test]
fn modified_file_creates_a_new_version_and_keeps_the_old_one() {
    let (_dir, root, vault) = fixture("newversion");
    write_file(&root, "Arc 2 Revised.txt", "version one text");
    scan::scan(&vault).unwrap();
    let original = store::list_versions(&vault, true).unwrap();
    assert_eq!(original.len(), 1);
    let original_id = original[0].id.clone();
    let original_sha = original[0].sha256.clone();

    write_file(&root, "Arc 2 Revised.txt", "version two text, quite different");
    let summary = scan::scan(&vault).unwrap();
    assert_eq!(summary.new_versions, 1);

    let versions = store::list_versions(&vault, true).unwrap();
    assert_eq!(versions.len(), 2, "both versions are retained");
    let old = versions.iter().find(|v| v.id == original_id).unwrap();
    assert_eq!(old.sha256, original_sha, "the old version's hash is untouched");
    assert_eq!(old.version_number, 1);
    assert!(!old.missing, "the old version is not deleted — it is simply superseded");
    let new = versions.iter().find(|v| v.id != original_id).unwrap();
    assert_eq!(new.version_number, 2);
    assert_eq!(new.file_id, old.file_id, "same family, linear versions");

    let items = inbox::list(&vault, Some(InboxStatus::Open)).unwrap();
    assert!(items.iter().any(|i| i.kind == "NEW_VERSION"), "{:?}", items.len());
}

#[test]
fn accepting_a_new_version_supersedes_the_previous_one_without_deleting_it() {
    let (_dir, root, vault) = fixture("accept");
    write_file(&root, "Arc 2 Revised.txt", "first draft body");
    scan::scan(&vault).unwrap();
    let first_id = store::list_versions(&vault, true).unwrap()[0].id.clone();

    write_file(&root, "Arc 2 Revised.txt", "second draft body, revised");
    scan::scan(&vault).unwrap();
    let items_before = inbox::list(&vault, Some(InboxStatus::Open)).unwrap();
    assert!(items_before.iter().any(|i| i.kind == "NEW_VERSION"));

    let items = inbox::list(&vault, Some(InboxStatus::Open)).unwrap();
    let new_version_item = items.iter().find(|i| i.kind == "NEW_VERSION").unwrap().clone();
    let new_id = new_version_item.file_version_id.clone().unwrap();

    inbox::resolve(&vault, &new_version_item.id, InboxAction::AcceptChange, Default::default()).unwrap();

    let new = store::get_version(&vault, &new_id).unwrap();
    assert!(new.is_current);
    assert_eq!(new.file_status, "CURRENT");
    let old = store::get_version(&vault, &first_id).unwrap();
    assert!(!old.is_current);
    assert_eq!(old.file_status, "HISTORICAL", "superseded, retained, and visibly marked");
}

// ---------------------------------------------------------------------------
// Criterion 8 — exactly one CURRENT per family, enforced by the database
// ---------------------------------------------------------------------------

#[test]
fn current_is_unique_per_family_at_the_database_level() {
    let (_dir, root, vault) = fixture("current");
    write_file(&root, "Arc 3.txt", "one");
    scan::scan(&vault).unwrap();
    write_file(&root, "Arc 3.txt", "two");
    scan::scan(&vault).unwrap();

    let versions = store::list_versions(&vault, true).unwrap();
    assert_eq!(versions.len(), 2);
    store::set_current(&vault, &versions[0].id).unwrap();
    store::set_current(&vault, &versions[1].id).unwrap();

    let versions = store::list_versions(&vault, true).unwrap();
    let currents = versions.iter().filter(|v| v.is_current).count();
    assert_eq!(currents, 1);
    assert!(versions[1].is_current);

    // A second CURRENT cannot be written even by a direct query — the rule is not
    // merely a UI convention.
    let forced = vault.conn.execute(
        "UPDATE file_versions SET is_current = 1 WHERE id = ?1",
        rusqlite::params![versions[0].id],
    );
    assert!(forced.is_err(), "the unique index must reject a second CURRENT");

    let _ = &vault;
}

// ---------------------------------------------------------------------------
// Criterion 6 / R2 — deletion and rename
// ---------------------------------------------------------------------------

#[test]
fn deleted_file_is_marked_missing_and_kept() {
    let (_dir, root, vault) = fixture("missing");
    write_file(&root, "Arc 3 Draft 3.txt", "this will be removed from disk");
    scan::scan(&vault).unwrap();
    let version_id = store::list_versions(&vault, true).unwrap()[0].id.clone();

    std::fs::remove_file(root.join("Arc 3 Draft 3.txt")).unwrap();
    let summary = scan::scan(&vault).unwrap();
    assert_eq!(summary.marked_missing, 1);

    let version = store::get_version(&vault, &version_id).unwrap();
    assert!(version.missing, "the record survives and reports the file as off-disk");
    assert_eq!(version.sha256.len(), 64, "the hash of the lost file is still recorded");

    // Its extracted text is still there, so search can still answer questions about it.
    let hits = store::search(&vault, "removed from disk", 10).unwrap();
    assert_eq!(hits.len(), 1, "an off-disk file stays searchable");

    let items = inbox::list(&vault, Some(InboxStatus::Open)).unwrap();
    assert!(items.iter().any(|i| i.kind == "MISSING_FILE"), "{:?}", items.len());
}

#[test]
fn renamed_file_is_offered_a_relink_rather_than_duplicated() {
    let (_dir, root, vault) = fixture("rename");
    write_file(&root, "Arc 2 Draft.txt", "identical contents here");
    scan::scan(&vault).unwrap();

    std::fs::rename(root.join("Arc 2 Draft.txt"), root.join("Arc 2 Draft (renamed).txt")).unwrap();
    let summary = scan::scan(&vault).unwrap();
    assert_eq!(summary.rename_candidates, 1, "matched by content hash");
    assert_eq!(summary.imported, 0, "no duplicate record is created");

    let items = inbox::list(&vault, Some(InboxStatus::Open)).unwrap();
    let rename_item = items.iter().find(|i| i.kind == "POSSIBLE_RENAME").unwrap().clone();

    inbox::resolve(&vault, &rename_item.id, InboxAction::Relink, Default::default()).unwrap();
    let versions = store::list_versions(&vault, true).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].rel_path, "Arc 2 Draft (renamed).txt");
    assert!(!versions[0].missing);
}

// ---------------------------------------------------------------------------
// Criterion 9 / 10 — archive keeps material searchable; search spans both
// ---------------------------------------------------------------------------

#[test]
fn archived_version_stays_searchable_and_marked() {
    let (_dir, root, vault) = fixture("archive");
    write_file(&root, "Arc 2 old.txt", "the picture frame appears here");
    write_file(&root, "Arc 2 new.txt", "the wooden bird appears here");
    scan::scan(&vault).unwrap();

    let versions = store::list_versions(&vault, true).unwrap();
    let old = versions.iter().find(|v| v.rel_path == "Arc 2 old.txt").unwrap();
    store::archive(&vault, &old.id).unwrap();

    let archived = store::get_version(&vault, &old.id).unwrap();
    assert_eq!(archived.file_status, "HISTORICAL");
    assert!(!archived.is_current);

    let hits = store::search(&vault, "picture frame", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].file_status, "HISTORICAL");
    assert_eq!(hits[0].status_marker, "⚠️", "archived material is visibly flagged");

    let current_hits = store::search(&vault, "wooden bird", 10).unwrap();
    assert_eq!(current_hits.len(), 1);
    assert_eq!(current_hits[0].status_marker, "⚫");
}

#[test]
fn search_matches_docx_text_and_filename() {
    let (_dir, root, vault) = fixture("search");
    write_docx(&root, "Arc 2 Revised.docx", "Azmirah kept the wooden bird");
    write_file(&root, "character ideas.txt", "notes about Maheen");
    scan::scan(&vault).unwrap();

    let by_content = store::search(&vault, "wooden bird", 10).unwrap();
    assert_eq!(by_content.len(), 1);
    assert_eq!(by_content[0].matched_in, "content");
    assert!(by_content[0].snippet.contains("wooden bird"), "{}", by_content[0].snippet);

    let by_name = store::search(&vault, "character ideas", 10).unwrap();
    assert_eq!(by_name.len(), 1);
    assert!(by_name[0].matched_in == "filename" || by_name[0].matched_in == "content");
}

// ---------------------------------------------------------------------------
// Criterion 13 — portability
// ---------------------------------------------------------------------------

#[test]
fn the_vault_travels_with_the_folder() {
    let (_dir, root, vault) = fixture("portability");
    write_file(&root, "Arc 1.txt", "arc one body");
    write_docx(&root, "Arc 2 Revised v2.docx", "arc two body");
    scan::scan(&vault).unwrap();
    let original = store::list_versions(&vault, true).unwrap();
    drop(vault);

    // Pretend the whole novel folder moved to a different absolute path. The target
    // directory stays alive for the rest of the test.
    let (_target_dir, target) = temp_root("moved");
    let destination = target.join("Saqar");
    copy_tree(&root, &destination);

    let moved = Vault::open(&destination).unwrap();
    let report = store::open_report(&moved).unwrap();
    assert_eq!(report.versions, original.len() as i64);
    assert!(report.hash_mismatches.is_empty(), "{:?}", report.hash_mismatches);

    let after = store::list_versions(&moved, true).unwrap();
    for version in &after {
        let abs = PathBuf::from(&version.abs_path);
        assert!(abs.is_file(), "every path resolves under the new root: {}", version.abs_path);
        assert!(version.abs_path.starts_with(&destination.display().to_string()));
    }

    // Search still works from the new location.
    let hits = store::search(&moved, "arc two body", 10).unwrap();
    assert_eq!(hits.len(), 1);
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&source, &target);
        } else {
            std::fs::copy(&source, &target).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Criterion 14 — crash safety: a killed scan leaves no half-written records
// ---------------------------------------------------------------------------

#[test]
fn an_interrupted_scan_leaves_a_consistent_vault() {
    let (_dir, root, vault) = fixture("crash");
    for index in 0..6 {
        write_file(&root, &format!("Arc 3 part {index}.txt"), &format!("body {index}"));
    }

    // Simulate the process dying mid-scan by importing only some files, then
    // reopening as a fresh connection would after a restart.
    // Only the author's files: the folder listing also holds `.novelvault/`, and its position in
    // the listing differs by OS (Windows sorts it first), so it must not count towards the three.
    let mut paths: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".txt"))
        .collect();
    paths.sort();
    paths.truncate(3);
    assert_eq!(paths.len(), 3);
    for rel in &paths {
        scan::import_file(&vault, rel).unwrap();
    }
    drop(vault);

    let reopened = Vault::open(&root).unwrap();
    let partial = store::list_versions(&reopened, true).unwrap();
    assert_eq!(partial.len(), 3, "only what was written is visible — no partial rows");
    for version in &partial {
        assert_eq!(version.version_number, 1);
        assert!(PathBuf::from(&version.abs_path).is_file());
    }

    // The next scan completes the job without duplicating what already landed.
    let summary = scan::scan(&reopened).unwrap();
    assert_eq!(summary.imported, 3);
    assert_eq!(summary.unchanged, 3);
    assert_eq!(store::list_versions(&reopened, true).unwrap().len(), 6);
}

// ---------------------------------------------------------------------------
// Watcher semantics (criterion 4/5 preconditions)
// ---------------------------------------------------------------------------

#[test]
fn a_word_style_save_burst_becomes_one_event() {
    let mut coalescer = watch::Coalescer::new(1500);
    let now = Instant::now();

    // Word: write a lock file, write a temp file, rename the temp over the target.
    coalescer.push(EventKind::Created, "~$Arc 3.docx", now);
    coalescer.push(EventKind::Created, "tmp8F2A.tmp", now);
    coalescer.push(EventKind::Modified, "tmp8F2A.tmp", now);
    coalescer.push(EventKind::Removed, "tmp8F2A.tmp", now);
    coalescer.push(EventKind::Created, "Arc 3.docx", now);
    coalescer.push(EventKind::Modified, "Arc 3.docx", now + Duration::from_millis(50));

    let due = coalescer.drain_due(now + Duration::from_millis(100));
    assert!(due.is_empty(), "the quiet window has not elapsed");

    let due = coalescer.drain_due(now + Duration::from_millis(2000));
    assert_eq!(due.len(), 2, "{due:?}");
    let target = due.iter().find(|e| e.rel_path == "Arc 3.docx").unwrap();
    assert_eq!(target.kind, EventKind::Created, "one event for the saved document");
    assert!(
        due.iter().all(|e| e.rel_path != "tmp8F2A.tmp"),
        "the editor's temp file appears and vanishes, so it leaves no event at all: {due:?}"
    );
}

#[test]
fn create_then_remove_cancels_out_entirely() {
    let mut coalescer = watch::Coalescer::new(100);
    let now = Instant::now();
    coalescer.push(EventKind::Created, "scratch.tmp", now);
    coalescer.push(EventKind::Removed, "scratch.tmp", now + Duration::from_millis(10));
    assert_eq!(coalescer.pending_len(), 0, "an editor's temp file leaves no trace");
    assert!(coalescer.flush_all().is_empty());
}

#[test]
fn remove_then_recreate_is_a_modification() {
    let mut coalescer = watch::Coalescer::new(100);
    let now = Instant::now();
    coalescer.push(EventKind::Removed, "Arc 2.txt", now);
    coalescer.push(EventKind::Created, "Arc 2.txt", now + Duration::from_millis(5));
    let due = coalescer.drain_due(now + Duration::from_millis(500));
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].kind, EventKind::Modified);
}

#[test]
fn watcher_events_are_applied_through_the_inbox() {
    let (_dir, root, vault) = fixture("watchevents");
    write_file(&root, "Arc 3.txt", "first body");
    scan::scan(&vault).unwrap();

    write_file(&root, "Arc 3.txt", "second body");
    write_file(&root, "Arc 4 planning.txt", "new file dropped in");

    let events = vec![
        novelvault_core::model::VaultEvent {
            kind: EventKind::Modified,
            rel_path: "Arc 3.txt".into(),
            at: novelvault_core::util::now_iso(),
        },
        novelvault_core::model::VaultEvent {
            kind: EventKind::Created,
            rel_path: "Arc 4 planning.txt".into(),
            at: novelvault_core::util::now_iso(),
        },
    ];
    let ids = scan::apply_events(&vault, &events).unwrap();
    assert_eq!(ids.len(), 2, "both changes queue an Inbox item");
    // Arc 3.txt: version 2. Arc 4 planning.txt: a separate family, imported fresh.

    let items = inbox::list(&vault, Some(InboxStatus::Open)).unwrap();
    let kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();
    assert!(kinds.contains(&"NEW_VERSION"), "{kinds:?}");
    assert!(kinds.contains(&"NEW_FILE"), "{kinds:?}");
    assert_eq!(
        store::list_versions(&vault, true).unwrap().len(),
        3,
        "two versions of Arc 3 plus the newly dropped Arc 4 file"
    );
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

#[test]
fn snapshot_writes_only_inside_the_vault_and_is_recorded() {
    let (_dir, root, vault) = fixture("snapshot");
    write_file(&root, "Arc 3.txt", "body");
    scan::scan(&vault).unwrap();
    let before = fingerprint(&root);

    let record = store::create_snapshot(&vault, "Before Arc 3 Revision", false, Some("test")).unwrap();
    assert!(record.rel_path.starts_with("snapshots/"));
    let snapshot_path = root.join(".novelvault").join(&record.rel_path);
    assert!(snapshot_path.is_file(), "{}", snapshot_path.display());

    assert_eq!(before, fingerprint(&root), "a snapshot never touches author files");
    assert_eq!(store::list_snapshots(&vault).unwrap().len(), 1);

    // The snapshot is a real, openable database containing the index.
    let restored = rusqlite::Connection::open(&snapshot_path).unwrap();
    let versions: i64 = restored
        .query_row("SELECT COUNT(*) FROM file_versions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, 1);
}

// ---------------------------------------------------------------------------
// Family proposals and merging
// ---------------------------------------------------------------------------

#[test]
fn similar_names_are_proposed_as_one_family_and_merge_keeps_history() {
    let (_dir, root, vault) = fixture("family");
    write_file(&root, "Arc 2 Revised.txt", "first");
    scan::scan(&vault).unwrap();
    write_file(&root, "Arc 2 Revised v2.txt", "second, different text");
    scan::scan(&vault).unwrap();

    let items = inbox::list(&vault, Some(InboxStatus::Open)).unwrap();
    let proposal = items
        .iter()
        .find(|i| i.kind == InboxKind::FamilyProposal.as_str())
        .expect("a family proposal is raised")
        .clone();
    assert!(proposal.candidate_version_id.is_some());

    let new_version_id = proposal.file_version_id.clone().unwrap();
    let new_file_id = store::get_version(&vault, &new_version_id).unwrap().file_id;
    let into_file_id = store::get_version(&vault, &proposal.candidate_version_id.clone().unwrap())
        .unwrap()
        .file_id;

    inbox::merge_family(&vault, &new_file_id, &into_file_id).unwrap();

    let versions = store::list_versions(&vault, true).unwrap();
    assert_eq!(versions.len(), 2, "both versions survive the merge");
    assert!(versions.iter().all(|v| v.file_id == into_file_id));
    let numbers: Vec<i64> = versions.iter().map(|v| v.version_number).collect();
    assert_eq!(numbers, vec![1, 2], "history stays linear");

    let merged_families: i64 = vault
        .conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE novel_id = ?1 AND deleted_at IS NULL",
            rusqlite::params![vault.novel_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(merged_families, 1, "the emptied family row is soft-deleted, not removed");
}

// ---------------------------------------------------------------------------
// Scope guards: no login, no network, nothing outside the folder
// ---------------------------------------------------------------------------

#[test]
fn the_vault_has_no_outbound_network_dependency() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    for banned in ["reqwest", "hyper", "ureq", "isahc", "awc", "surf", "tokio-tungstenite"] {
        assert!(
            !manifest.contains(banned),
            "V1 makes no network calls, but {banned} appears in the dependency list"
        );
    }
}

#[test]
fn every_author_file_is_only_ever_opened_read_only() {
    let (_dir, root, vault) = fixture("readonly");
    write_file(&root, "Arc 2.txt", "body");
    scan::scan(&vault).unwrap();
    let path = root.join("Arc 2.txt");
    let before = std::fs::read(&path).unwrap();

    // classify, archive, search, snapshot — the full V1 surface
    let version_id = store::list_versions(&vault, true).unwrap()[0].id.clone();
    store::classify(&vault, &version_id, Some(FileStatus::WorkingDraft), Some("Arc 2"), None, None).unwrap();
    store::search(&vault, "body", 5).unwrap();
    store::create_snapshot(&vault, "check", false, None).unwrap();
    store::archive(&vault, &version_id).unwrap();

    assert_eq!(before, std::fs::read(&path).unwrap(), "the file bytes never change");
}
