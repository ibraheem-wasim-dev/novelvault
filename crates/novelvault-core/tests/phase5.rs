//! Phase 5 acceptance tests — the Reader: a crawler that learns the novel as it reads.
//!
//! The promises under test:
//!
//! * It reads incrementally, in story order, and what it learned from chapter 1 helps it
//!   read chapter 2.
//! * Everything it knows points back to a file; it never touches the author's files or canon.
//! * Authority follows the file: archive a draft and its facts become history, instantly.
//! * Disagreements between drafts become notices that wait for the author.
//! * It works with no model; with a model, invented claims are discarded.

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
use novelvault_core::{addfiles, ai, canon, reader, store};

fn temp_dir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(prefix).tempdir().expect("temp dir")
}

struct Novel {
    _parent: tempfile::TempDir,
    vault: Vault,
}

fn new_novel() -> Novel {
    let parent = temp_dir("nv-reader");
    let (_root, vault) = addfiles::create_novel(parent.path(), "Saqar", Some("Saqar")).expect("novel");
    Novel { _parent: parent, vault }
}

/// Drop one file into the novel the way the author would, make it CURRENT, and return its version id.
fn add_chapter(n: &Novel, name: &str, body: &str, arc: &str, chapter: &str) -> String {
    let src = temp_dir("nv-reader-src");
    fs::write(src.path().join(name), body).unwrap();
    addfiles::apply(&n.vault, &[src.path().join(name).display().to_string()], false, true).expect("apply");
    let v = store::list_versions(&n.vault, false)
        .unwrap()
        .into_iter()
        .find(|v| v.rel_path.ends_with(name))
        .expect("version");
    store::classify(&n.vault, &v.id, Some(FileStatus::WorkingDraft), Some(arc), Some(chapter), None).expect("classify");
    v.id
}

const CH1: &str = "Azmirah kept the wooden bird on her windowsill. It had been her brother's.\n\n\
Lucifer watched Azmirah from the doorway. Nobody knew why Lucifer had come to Zanzibar.\n\n\
She promised Maheen that she would return before winter. Maheen believed Azmirah, and Lucifer said nothing.\n\n\
Azmirah walked to the harbour with Maheen. Later Azmirah told Maheen the secret of the bird.";

const CH2: &str = "Maheen, afraid and cold, sat on the harbour wall. Azmirah found a letter in the sand.\n\n\
Lucifer killed the guard and escaped from Zanzibar. Azmirah never saw Lucifer again.\n\n\
Azmirah lived in Zanzibar. Maheen left the harbour at dawn.";

// ---------------------------------------------------------------------------
// Learning side by side with the novel
// ---------------------------------------------------------------------------

#[test]
fn the_reader_learns_each_file_as_it_arrives_and_never_rereads() {
    let n = new_novel();
    add_chapter(&n, "Arc 1 Chapter 1.md", CH1, "Arc 1", "Chapter 1");

    let first = reader::crawl(&n.vault, 10).unwrap();
    assert_eq!(first.files_read, 1);
    assert_eq!(first.remaining, 0);
    let step = &first.steps[0];
    assert!(step.sentences >= 8);
    assert!(step.learned.iter().any(|x| x == "Azmirah"), "it met Azmirah: {:?}", step.learned);
    assert!(step.learned.iter().any(|x| x == "Lucifer"));

    // Nothing new, nothing read.
    assert_eq!(reader::crawl(&n.vault, 10).unwrap().files_read, 0);

    // A second chapter is folded into what it already knows.
    add_chapter(&n, "Arc 1 Chapter 2.md", CH2, "Arc 1", "Chapter 2");
    let second = reader::crawl(&n.vault, 10).unwrap();
    assert_eq!(second.files_read, 1, "only the new file is read");

    let u = reader::understanding(&n.vault).unwrap();
    assert_eq!(u.coverage.files_total, 2);
    assert_eq!(u.coverage.files_read, 2);
    assert_eq!(u.coverage.files_pending, 0);
    let names: Vec<&str> = u.entities.iter().map(|e| e.name.as_str()).collect();
    for want in ["Azmirah", "Lucifer", "Maheen"] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    let azmirah = u.entities.iter().find(|e| e.name == "Azmirah").unwrap();
    assert_eq!(azmirah.kind, "character");
    assert_eq!(azmirah.files, 2);
    assert!(u.entities.iter().find(|e| e.name == "Zanzibar").map(|e| e.kind == "location").unwrap_or(false));
    assert!(u.chapters.len() == 2 && u.chapters[0].label.contains("Chapter 1"), "story order, not arrival order");
}

#[test]
fn what_it_learned_from_chapter_one_lets_it_recognise_a_name_in_chapter_two() {
    let n = new_novel();
    // "Maheen," opens a sentence and is followed by a comma, not a verb: a first-time reader cannot tell it is a name.
    let cold = reader::read_text("Maheen, afraid and cold, sat on the wall.", &Default::default());
    assert!(!cold.names.contains_key("Maheen"));

    add_chapter(&n, "Arc 1 Chapter 1.md", CH1, "Arc 1", "Chapter 1");
    reader::crawl(&n.vault, 10).unwrap();
    add_chapter(&n, "Arc 1 Chapter 2.md", CH2, "Arc 1", "Chapter 2");
    reader::crawl(&n.vault, 10).unwrap();

    let profile = reader::entity_profile(&n.vault, "Maheen").unwrap();
    assert!(
        profile.files.iter().any(|f| f.contains("Chapter 2")) || profile.events.iter().any(|e| e.rel_path.contains("Chapter 2")),
        "chapter 2's Maheen was recognised because chapter 1 taught the name"
    );
    let conn = &n.vault.conn;
    let ch2_maheen: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(total),0) FROM reader_names n JOIN file_versions v ON v.id = n.version_id
             WHERE n.name = 'Maheen' AND v.rel_path LIKE '%Chapter 2%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ch2_maheen >= 2, "got {ch2_maheen}");
}

#[test]
fn it_builds_a_profile_a_timeline_and_open_threads_from_the_text() {
    let n = new_novel();
    add_chapter(&n, "Arc 1 Chapter 1.md", CH1, "Arc 1", "Chapter 1");
    add_chapter(&n, "Arc 1 Chapter 2.md", CH2, "Arc 1", "Chapter 2");
    reader::crawl(&n.vault, 10).unwrap();

    let p = reader::entity_profile(&n.vault, "azmirah").unwrap(); // case-insensitive
    let keeps = p.facts.iter().find(|f| f.predicate == "keeps").expect("keeps fact");
    assert!(keeps.object.contains("wooden bird"));
    assert!(keeps.rel_path.contains("Chapter 1"), "every fact names its file");
    assert!(keeps.excerpt.contains("windowsill"), "…and its sentence");
    assert!(p.facts.iter().any(|f| f.predicate == "lives in" && f.object == "Zanzibar"));
    assert!(p.related.iter().any(|(name, _)| name == "Lucifer" || name == "Maheen"));
    assert!(p.summary.contains("In current files"));

    let u = reader::understanding(&n.vault).unwrap();
    assert!(u.timeline.iter().any(|e| e.summary.contains("killed the guard")), "dramatic events are kept");
    assert!(u.threads.iter().any(|t| t.excerpt.contains("Nobody knew")), "an open question is flagged");
    assert!(u.threads.iter().any(|t| t.excerpt.to_lowercase().contains("secret")));
    assert!(u.story_so_far.contains("Chapter 1"), "story so far is assembled in story order");
    assert_eq!(u.story_so_far_method, "reading");

    assert!(reader::entity_profile(&n.vault, "Gandalf").is_err(), "it has not met him, and says so");
}

// ---------------------------------------------------------------------------
// Authority follows the file
// ---------------------------------------------------------------------------

fn revision_novel() -> (Novel, String, String) {
    let n = new_novel();
    let new_id = add_chapter(
        &n,
        "Arc 2 Revised v2.md",
        "Azmirah kept the wooden bird on her windowsill. The wooden bird had been her brother's.\n\nLucifer watched the wooden bird from the doorway.",
        "Arc 2",
        "Chapter 1",
    );
    let old_id = add_chapter(
        &n,
        "Old Arc 2.md",
        "Azmirah kept the picture frame on her windowsill. The picture frame had been her brother's.",
        "Arc 2",
        "Chapter 1",
    );
    store::set_current(&n.vault, &new_id).unwrap();
    store::classify(&n.vault, &old_id, Some(FileStatus::Historical), None, None, None).unwrap();
    (n, new_id, old_id)
}

#[test]
fn a_disagreement_between_an_old_draft_and_a_live_one_is_noticed_not_decided() {
    let (n, _new, _old) = revision_novel();
    let report = reader::crawl(&n.vault, 10).unwrap();
    assert_eq!(report.files_read, 2);
    assert_eq!(report.new_notices, 1);

    let changes = reader::story_changes(&n.vault).unwrap();
    assert_eq!(changes.len(), 1);
    let c = &changes[0];
    assert_eq!(c.kind, "revision");
    assert!(c.old_value.contains("picture frame") && c.new_value.contains("wooden bird"));
    assert!(c.old_file.contains("Old Arc 2") && c.new_file.contains("Revised"));

    let pending = ai::list_suggestions(&n.vault, Some("pending")).unwrap();
    let notice = pending.iter().find(|s| s.kind == "story_change").expect("a notice is waiting");
    assert!(notice.detail.contains("deliberate revision"));
    assert!(canon::list(&n.vault, None, true).unwrap().is_empty(), "noticing is not canon (R1)");

    // Crawling again does not duplicate the notice, and a dismissed one stays dismissed.
    assert_eq!(reader::sync_change_notices(&n.vault).unwrap(), 0);
    ai::decide_suggestion(&n.vault, &notice.id, "dismissed", Some("deliberate"), None).unwrap();
    assert_eq!(reader::sync_change_notices(&n.vault).unwrap(), 0);
}

#[test]
fn archiving_a_draft_turns_its_facts_into_history_without_rereading() {
    let n = new_novel();
    let a = add_chapter(&n, "Draft A.md", "Azmirah kept the picture frame on her windowsill.\n\nLucifer watched Azmirah.", "Arc 2", "Chapter 1");
    reader::crawl(&n.vault, 10).unwrap();
    let before = reader::entity_profile(&n.vault, "Azmirah");
    // Only one mention so far, so she is not yet "known"; add the rest and re-check authority.
    drop(before);
    let b = add_chapter(&n, "Draft B.md", "Azmirah kept the wooden bird.\n\nAzmirah smiled at Lucifer. Azmirah told Lucifer nothing.", "Arc 2", "Chapter 1");
    reader::crawl(&n.vault, 10).unwrap();

    let p = reader::entity_profile(&n.vault, "Azmirah").unwrap();
    assert!(p.facts.iter().filter(|f| f.predicate == "keeps").all(|f| f.is_live), "both drafts are working drafts");

    store::classify(&n.vault, &a, Some(FileStatus::Historical), None, None, None).unwrap();
    let after = reader::crawl(&n.vault, 10).unwrap();
    assert_eq!(after.files_read, 0, "classification does not trigger a re-read");
    let p = reader::entity_profile(&n.vault, "Azmirah").unwrap();
    let frame = p.facts.iter().find(|f| f.object.contains("picture frame")).unwrap();
    let bird = p.facts.iter().find(|f| f.object.contains("wooden bird")).unwrap();
    assert!(!frame.is_live && bird.is_live, "the archived draft is now history");
    assert!(p.summary.contains("Older drafts said"));
    let _ = b;
}

// ---------------------------------------------------------------------------
// Handing it to the librarian, and staying read-only
// ---------------------------------------------------------------------------

#[test]
fn the_librarian_uses_what_the_reader_learned_and_labels_it_as_derived() {
    let n = new_novel();
    add_chapter(&n, "Arc 1 Chapter 1.md", CH1, "Arc 1", "Chapter 1");
    add_chapter(&n, "Arc 1 Chapter 2.md", CH2, "Arc 1", "Chapter 2");

    // `ask` crawls on its own — nobody had to press a button.
    let a = ai::ask(&n.vault, "Where does Azmirah live?").unwrap();
    let reader_src = a.sources.iter().find(|s| s.kind == "understanding").expect("reader source");
    assert!(reader_src.text.contains("lives in Zanzibar"));
    assert!(reader_src.authority.contains("not canon"));
    assert!(a.answer.contains("WHAT THE READER HAS LEARNED"));
    let labels: Vec<&str> = a.sources.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, (1..=a.sources.len()).map(|i| format!("S{i}")).collect::<Vec<_>>());

    let plot = ai::ask(&n.vault, "Give me a recap of the plot").unwrap();
    assert!(plot.sources.iter().any(|s| s.title == "The story so far"));
}

#[test]
fn reading_never_touches_a_file_or_canon() {
    let (n, _new, _old) = revision_novel();
    let hashes = |n: &Novel| -> Vec<String> {
        let mut v: Vec<String> = store::list_versions(&n.vault, false)
            .unwrap()
            .iter()
            .map(|v| hash_file(Path::new(&v.abs_path)).unwrap().0)
            .collect();
        v.sort();
        v
    };
    let before = hashes(&n);
    let canon_before = canon::list(&n.vault, None, true).unwrap().len();
    reader::crawl(&n.vault, 10).unwrap();
    reader::understanding(&n.vault).unwrap();
    ai::ask(&n.vault, "Who is Azmirah?").unwrap();
    assert_eq!(before, hashes(&n));
    assert_eq!(canon_before, canon::list(&n.vault, None, true).unwrap().len());
}

#[test]
fn a_bounded_crawl_reads_in_story_order_and_reports_what_remains() {
    let n = new_novel();
    // Added out of order on purpose.
    add_chapter(&n, "c10.md", "Azmirah walked to the harbour.", "Arc 1", "Chapter 10");
    add_chapter(&n, "c2.md", "Azmirah walked to the market.", "Arc 1", "Chapter 2");
    add_chapter(&n, "c1.md", "Azmirah walked to the gate.", "Arc 1", "Chapter 1");
    let r = reader::crawl(&n.vault, 2).unwrap();
    assert_eq!(r.files_read, 2);
    assert_eq!(r.remaining, 1);
    assert!(r.steps[0].rel_path.ends_with("c1.md") && r.steps[1].rel_path.ends_with("c2.md"), "chapter 2 before chapter 10");
    assert_eq!(reader::crawl(&n.vault, 2).unwrap().remaining, 0);
}

// ---------------------------------------------------------------------------
// With a local model
// ---------------------------------------------------------------------------

fn fake_ollama(reply: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(0usize));
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
                *seen.lock().unwrap() += 1;
                serde_json::json!({ "message": { "role": "assistant", "content": reply } }).to_string()
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

#[test]
fn a_local_model_deepens_the_reading_but_invented_claims_are_thrown_away() {
    let n = new_novel();
    add_chapter(&n, "Arc 1 Chapter 1.md", CH1, "Arc 1", "Chapter 1");
    reader::crawl(&n.vault, 10).unwrap();
    assert!(reader::deepen_with_model(&n.vault, 3).is_err(), "no model, no deepening — and it says so");

    let port = fake_ollama(
        r#"{"summary":"Azmirah keeps a wooden bird while Lucifer watches and Maheen hears a secret.",
            "characters":[{"name":"Azmirah"},{"name":"Maheen"},{"name":"Gandalf"}],
            "facts":[{"subject":"Azmirah","predicate":"keeps","object":"a wooden bird"},
                     {"subject":"Gandalf","predicate":"carries","object":"a staff"}],
            "events":["Azmirah tells Maheen the secret of the bird","A dragon destroys Atlantis"]}"#,
    );
    ai::save_config(
        &n.vault,
        &LlmConfig { provider: "ollama".into(), host: "127.0.0.1".into(), port, model: "test-model".into() },
    )
    .unwrap();

    assert_eq!(reader::deepen_with_model(&n.vault, 3).unwrap(), 1);
    let u = reader::understanding(&n.vault).unwrap();
    assert_eq!(u.coverage.files_deepened, 1);
    assert_eq!(u.chapters[0].method, "model");
    assert!(u.chapters[0].summary.contains("wooden bird"));
    assert!(u.timeline.iter().any(|e| e.source == "model" && e.summary.contains("secret of the bird")));
    assert!(u.timeline.iter().all(|e| !e.summary.contains("Atlantis")), "invented event discarded");
    assert!(u.entities.iter().all(|e| e.name != "Gandalf"), "invented character discarded");
    assert_eq!(u.story_so_far_method, "model", "a rolling story-so-far was written");

    // Already deepened: nothing more to do.
    assert_eq!(reader::deepen_with_model(&n.vault, 3).unwrap(), 0);
}

// ---------------------------------------------------------------------------
// Older vaults
// ---------------------------------------------------------------------------

#[test]
fn a_phase_four_vault_opens_and_is_read_from_scratch() {
    let n = new_novel();
    add_chapter(&n, "Arc 1 Chapter 1.md", CH1, "Arc 1", "Chapter 1");
    // Simulate a vault written before the Reader existed.
    for t in ["reader_files", "reader_names", "reader_facts", "reader_events", "reader_threads", "reader_relations"] {
        n.vault.conn.execute(&format!("DROP TABLE {t}"), []).unwrap();
    }
    let root = n.vault.root.clone();
    let reopened = Vault::open(&root).expect("reopens and migrates");
    assert_eq!(reader::crawl(&reopened, 10).unwrap().files_read, 1);
}
