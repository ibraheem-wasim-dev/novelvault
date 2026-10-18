//! Phase 4 acceptance tests — the local AI librarian and the library of novels.
//!
//! These test the promises:
//!
//! * The librarian never changes a file, and never creates or promotes canon on its own.
//! * Canon outranks files, current outranks old, and a change in canon is announced.
//! * The model is only ever a *local* one; anything else is refused before a socket opens.
//! * With no model at all, the librarian still works.
//! * What the assistant notices waits for the author's decision.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use novelvault_core::db::Vault;
use novelvault_core::llm::LlmConfig;
use novelvault_core::model::*;
use novelvault_core::util::hash_file;
use novelvault_core::{addfiles, ai, canon, library, store, story};

fn temp_dir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(prefix).tempdir().expect("temp dir")
}

fn write_file(path: &Path, body: &str) {
    fs::write(path, body).expect("write");
}

struct Fixture {
    _parent: tempfile::TempDir,
    _sources: tempfile::TempDir,
    vault: Vault,
}

/// "Saqar" with a current Arc 2 draft (wooden bird) and an old Arc 2 draft (picture frame).
fn saqar() -> Fixture {
    let parent = temp_dir("nv-ai-parent");
    let (_root, vault) = addfiles::create_novel(parent.path(), "Saqar", Some("Saqar")).expect("novel");
    let sources = temp_dir("nv-ai-src");
    write_file(
        &sources.path().join("Arc 2 Revised v2.md"),
        "Azmirah kept the wooden bird on her windowsill. The wooden bird had been her brother's.\n\nLucifer watched the wooden bird from the doorway.",
    );
    write_file(
        &sources.path().join("Old Arc 2.md"),
        "Azmirah kept the picture frame on her windowsill. The picture frame had been her brother's.",
    );
    addfiles::apply(&vault, &[sources.path().display().to_string()], false, true).expect("apply");
    let versions = store::list_versions(&vault, false).expect("versions");
    let current = versions.iter().find(|v| v.rel_path.contains("Revised")).expect("revised");
    let old = versions.iter().find(|v| v.rel_path.contains("Old Arc")).expect("old");
    store::set_current(&vault, &current.id).expect("current");
    store::classify(&vault, &old.id, Some(FileStatus::Historical), None, None, None).expect("historical");
    Fixture { _parent: parent, _sources: sources, vault }
}

fn add_canon(vault: &Vault, subject: &str, value: &str) -> CanonItem {
    let item = canon::propose(vault, subject, value, Some("object"), None, None, None).expect("propose");
    let v = store::list_versions(vault, false).unwrap().into_iter().find(|v| v.is_current).unwrap();
    canon::add_source(vault, &item.id, "FileVersion", &v.id, Some(&v.family_name), None).expect("source");
    canon::promote(vault, &item.id, CanonStatus::Canon, "author confirmed").expect("promote")
}

// ---------------------------------------------------------------------------
// Index and search
// ---------------------------------------------------------------------------

#[test]
fn indexing_is_incremental() {
    let f = saqar();
    let first = ai::index_new(&f.vault).expect("index");
    assert_eq!(first.indexed_files, 2);
    let second = ai::index_new(&f.vault).expect("index again");
    assert_eq!(second.indexed_files, 0, "nothing new, nothing re-read");
    let status = ai::status(&f.vault).expect("status");
    assert_eq!(status.indexed_files, 2);
    assert_eq!(status.pending_files, 0);
    assert_eq!(status.mode, "offline");
}

#[test]
fn current_files_outrank_old_drafts_and_old_drafts_carry_a_warning() {
    let f = saqar();
    ai::index_new(&f.vault).unwrap();
    let sources = ai::retrieve(&f.vault, "who had the keepsake on the windowsill", 6).unwrap();
    let files: Vec<_> = sources.iter().filter(|s| s.kind == "file").collect();
    assert!(files.len() >= 2, "both drafts should be found");
    assert!(files[0].title.contains("Revised"), "the current draft is listed first");
    let old = files.iter().find(|s| s.title.contains("Old Arc")).expect("old draft still shown");
    assert!(old.warning.as_deref().unwrap_or("").contains("outdated"), "old draft is flagged");
    assert!(files[0].warning.is_none(), "the current draft carries no warning");
}

#[test]
fn awkward_questions_never_break_the_search() {
    let f = saqar();
    for q in ["\"unclosed", "wooden OR NOT AND", "what's (the) bird*?", "***", "NEAR(", "a b"] {
        let r = ai::ask(&f.vault, q);
        assert!(r.is_ok(), "question {q:?} should be handled, got {r:?}");
    }
}

// ---------------------------------------------------------------------------
// The Canon Shield
// ---------------------------------------------------------------------------

#[test]
fn canon_comes_first_and_a_superseded_value_is_announced_as_history() {
    let f = saqar();
    let item = add_canon(&f.vault, "Keepsake", "picture frame");
    canon::change_value(&f.vault, &item.id, "wooden bird", "revised: the keepsake is a wooden bird", Some(CanonStatus::Canon)).expect("change");

    let answer = ai::ask(&f.vault, "What is the keepsake?").expect("ask");
    assert_eq!(answer.mode, "offline");
    assert_eq!(answer.sources[0].kind, "canon", "canon is consulted before any file");
    assert!(answer.sources[0].text.contains("wooden bird"));
    assert_eq!(answer.sources[0].authority, "canon");

    let outdated = answer.sources.iter().find(|s| s.status == "OUTDATED").expect("history is still shown");
    assert!(outdated.text.contains("picture frame"));
    assert!(outdated.warning.as_deref().unwrap().contains("wooden bird"), "points forward to the current value");
    assert!(
        answer.warnings.iter().any(|w| w.contains("Outdated canon")),
        "the change is announced, not hidden: {:?}",
        answer.warnings
    );
}

#[test]
fn asking_never_touches_files_or_canon() {
    let f = saqar();
    let item = add_canon(&f.vault, "Keepsake", "wooden bird");
    let hashes = |v: &Vault| -> Vec<String> {
        let mut h: Vec<String> = store::list_versions(v, false)
            .unwrap()
            .iter()
            .map(|x| hash_file(&v.abs_path(&x.rel_path)).unwrap().0)
            .collect();
        h.sort();
        h
    };
    let before_files = hashes(&f.vault);
    let before_canon = canon::list(&f.vault, None, true).unwrap().len();
    let before_ideas = story::list_ideas(&f.vault, None).unwrap().len();

    ai::ask(&f.vault, "Where is the wooden bird?").unwrap();
    ai::scan_suggestions(&f.vault).unwrap();
    ai::memory(&f.vault).unwrap();

    assert_eq!(before_files, hashes(&f.vault), "no file changed");
    assert_eq!(before_canon, canon::list(&f.vault, None, true).unwrap().len(), "no canon created");
    assert_eq!(before_ideas, story::list_ideas(&f.vault, None).unwrap().len());
    let after = canon::get(&f.vault, &item.id).unwrap();
    assert_eq!(after.status, "CANON", "no promotion either");
}

#[test]
fn an_unknown_subject_says_so_instead_of_inventing() {
    let f = saqar();
    let answer = ai::ask(&f.vault, "Who is the king of Zanzibar?").unwrap();
    assert!(answer.sources.is_empty());
    assert!(answer.answer.contains("found nothing"));
}

#[test]
fn history_is_kept_per_novel_and_can_be_cleared() {
    let f = saqar();
    ai::ask(&f.vault, "wooden bird").unwrap();
    assert_eq!(ai::history(&f.vault, 10).unwrap().len(), 2);
    let stored = &ai::history(&f.vault, 10).unwrap()[1];
    assert_eq!(stored.role, "assistant");
    assert!(!stored.sources.is_empty(), "sources survive a restart");
    ai::clear_history(&f.vault).unwrap();
    assert!(ai::history(&f.vault, 10).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// The local model
// ---------------------------------------------------------------------------

#[test]
fn a_remote_ai_host_is_refused_and_nothing_is_saved() {
    let f = saqar();
    let cfg = LlmConfig { provider: "ollama".into(), host: "api.example.com".into(), port: 11434, model: "x".into() };
    let err = ai::save_config(&f.vault, &cfg).unwrap_err().to_string();
    assert!(err.contains("not on this computer"), "{err}");
    assert_eq!(ai::load_config(&f.vault).provider, "none");
}

/// A pretend Ollama on a loopback port. Records every request body it receives.
fn fake_ollama(reply: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = seen.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = vec![0u8; 65536];
            let mut data = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                data.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&data).to_string();
                if let Some(split) = text.find("\r\n\r\n") {
                    let len = text[..split]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if data.len() >= split + 4 + len || n == 0 {
                        break;
                    }
                } else if n == 0 {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&data).to_string();
            let body = if text.starts_with("GET /api/tags") {
                r#"{"models":[{"name":"test-model"}]}"#.to_string()
            } else {
                log.lock().unwrap().push(text.clone());
                serde_json::json!({ "message": { "role": "assistant", "content": reply } }).to_string()
            };
            // Chunked on purpose: real servers do this.
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (port, seen)
}

#[test]
fn with_a_local_model_the_answer_is_grounded_in_numbered_sources() {
    let f = saqar();
    let item = add_canon(&f.vault, "Keepsake", "picture frame");
    canon::change_value(&f.vault, &item.id, "wooden bird", "revised", Some(CanonStatus::Canon)).unwrap();

    let (port, seen) = fake_ollama("The keepsake is a wooden bird [S1]. An old draft said picture frame [S2].");
    let cfg = LlmConfig { provider: "ollama".into(), host: "127.0.0.1".into(), port, model: "test-model".into() };
    ai::save_config(&f.vault, &cfg).expect("config");

    let status = ai::status(&f.vault).unwrap();
    assert_eq!(status.mode, "model");
    assert_eq!(status.models, vec!["test-model".to_string()]);

    let answer = ai::ask(&f.vault, "What is the keepsake?").expect("ask");
    assert_eq!(answer.mode, "model");
    assert_eq!(answer.model.as_deref(), Some("test-model"));
    assert!(answer.cited.contains(&"S1".to_string()));

    let requests = seen.lock().unwrap();
    let sent = requests.last().expect("the model was called");
    assert!(sent.starts_with("POST /api/chat"));
    assert!(sent.contains("[S1]") && sent.contains("wooden bird"), "sources were handed to the model");
    assert!(sent.contains("Never invent"), "the grounding rules were sent");
    assert!(sent.contains("OUTDATED"), "outdated canon is labelled for the model");
}

#[test]
fn a_model_that_cites_nothing_is_flagged() {
    let f = saqar();
    let (port, _) = fake_ollama("It is probably a wooden bird.");
    ai::save_config(
        &f.vault,
        &LlmConfig { provider: "ollama".into(), host: "localhost".into(), port, model: "test-model".into() },
    )
    .unwrap();
    let answer = ai::ask(&f.vault, "wooden bird").unwrap();
    assert!(answer.cited.is_empty());
    assert!(answer.warnings.iter().any(|w| w.contains("did not cite")));
}

#[test]
fn an_unreachable_model_falls_back_to_offline_instead_of_failing() {
    let f = saqar();
    // Grab a free port, then close it so nothing is listening there.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    ai::save_config(
        &f.vault,
        &LlmConfig { provider: "ollama".into(), host: "127.0.0.1".into(), port, model: "gone".into() },
    )
    .unwrap();
    let answer = ai::ask(&f.vault, "wooden bird").expect("must still answer");
    assert_eq!(answer.mode, "offline");
    assert!(answer.note.as_deref().unwrap_or("").contains("could not answer"));
    assert!(!answer.sources.is_empty());
}

// ---------------------------------------------------------------------------
// Noticing, memory and the author's decision
// ---------------------------------------------------------------------------

fn novel_with_unrecorded_name() -> Fixture {
    let parent = temp_dir("nv-ai-names");
    let (_root, vault) = addfiles::create_novel(parent.path(), "Names", Some("Names")).expect("novel");
    story::create_character(&vault, "Azmirah", None, None).unwrap();
    let sources = temp_dir("nv-ai-names-src");
    write_file(
        &sources.path().join("Arc 3 Chapter 5.md"),
        "Azmirah met Maheen at the gate. She asked Maheen about the river.\n\nLater, Azmirah told Maheen everything. Maheen only nodded.",
    );
    addfiles::apply(&vault, &[sources.path().display().to_string()], false, true).expect("apply");
    let v = store::list_versions(&vault, false).unwrap().remove(0);
    store::set_current(&vault, &v.id).unwrap();
    Fixture { _parent: parent, _sources: sources, vault }
}

#[test]
fn an_unrecorded_name_is_suggested_but_nothing_is_created_until_the_author_accepts() {
    let f = novel_with_unrecorded_name();
    let created = ai::scan_suggestions(&f.vault).unwrap();
    assert!(created >= 1);
    let pending = ai::list_suggestions(&f.vault, Some("pending")).unwrap();
    let maheen = pending.iter().find(|s| s.title == "Maheen").expect("Maheen noticed");
    assert!(!pending.iter().any(|s| s.title == "Azmirah"), "a recorded character is not re-suggested");
    assert!(!maheen.evidence.is_empty(), "the suggestion points at its evidence");

    // Noticed, not recorded.
    assert!(story::list_characters(&f.vault).unwrap().iter().all(|c| c.name != "Maheen"));

    // The author decides; only then does a record appear.
    ai::decide_suggestion(&f.vault, &maheen.id, "accepted", Some("yes, a new character"), Some("character")).unwrap();
    assert!(story::list_characters(&f.vault).unwrap().iter().any(|c| c.name == "Maheen"));
    // …and it is a story record, not canon.
    assert!(canon::list(&f.vault, None, true).unwrap().is_empty());
}

#[test]
fn a_dismissed_suggestion_stays_dismissed() {
    let f = novel_with_unrecorded_name();
    ai::scan_suggestions(&f.vault).unwrap();
    let maheen = ai::list_suggestions(&f.vault, Some("pending")).unwrap().into_iter().find(|s| s.title == "Maheen").unwrap();
    ai::decide_suggestion(&f.vault, &maheen.id, "dismissed", Some("not a character"), None).unwrap();
    assert_eq!(ai::scan_suggestions(&f.vault).unwrap(), 0, "a rescan does not resurrect it");
    assert!(ai::decide_suggestion(&f.vault, &maheen.id, "accepted", None, None).is_err(), "decided means decided");
    assert!(story::list_characters(&f.vault).unwrap().iter().all(|c| c.name != "Maheen"));
}

#[test]
fn memory_tracks_what_the_novel_contains_and_where_it_is_still_live() {
    let f = saqar();
    story::create_character(&f.vault, "Azmirah", Some("protagonist"), None).unwrap();
    story::create_object(&f.vault, "wooden bird", None).unwrap();
    story::create_object(&f.vault, "picture frame", None).unwrap();
    let m = ai::memory(&f.vault).unwrap();
    assert_eq!(m.files.len(), 2);
    assert!(m.files.iter().all(|x| !x.digest.is_empty()));
    let bird = m.entities.iter().find(|e| e.name == "wooden bird").unwrap();
    assert!(bird.passages_live >= 1 && bird.passages_older == 0);
    let frame = m.entities.iter().find(|e| e.name == "picture frame").unwrap();
    assert_eq!(frame.passages_live, 0, "the picture frame survives only in the old draft");
    assert!(frame.passages_older >= 1);
}

// ---------------------------------------------------------------------------
// The library of novels
// ---------------------------------------------------------------------------

#[test]
fn the_library_remembers_several_novels_and_forgetting_never_deletes_a_folder() {
    let app_data = temp_dir("nv-library");
    let parent = temp_dir("nv-lib-novels");
    let (root_a, vault_a) = addfiles::create_novel(parent.path(), "Saqar", Some("Saqar")).unwrap();
    let (root_b, _vault_b) = addfiles::create_novel(parent.path(), "Another Novel", Some("Another Novel")).unwrap();
    story::create_character(&vault_a, "Azmirah", None, None).unwrap();

    library::register(app_data.path(), &root_a, "Saqar").unwrap();
    library::register(app_data.path(), &root_b, "Another Novel").unwrap();
    library::register(app_data.path(), &root_a, "Saqar").unwrap(); // re-opening is not a duplicate

    let list = library::summaries(app_data.path()).unwrap();
    assert_eq!(list.len(), 2);
    let saqar = list.iter().find(|s| s.title == "Saqar").unwrap();
    assert!(saqar.exists && saqar.has_vault);
    assert_eq!(saqar.characters, 1);
    assert_eq!(list[0].title, "Saqar", "most recently opened first");

    assert!(library::forget(app_data.path(), &saqar.path).unwrap());
    assert_eq!(library::summaries(app_data.path()).unwrap().len(), 1);
    assert!(root_a.join(".novelvault").join("vault.db").exists(), "the novel itself is untouched");
}

#[test]
fn a_moved_novel_is_kept_in_the_library_and_reported_missing() {
    let app_data = temp_dir("nv-library-missing");
    let gone = app_data.path().join("nowhere");
    library::register(app_data.path(), &gone, "Lost Novel").unwrap();
    let list = library::summaries(app_data.path()).unwrap();
    assert_eq!(list.len(), 1);
    assert!(!list[0].exists);
}
