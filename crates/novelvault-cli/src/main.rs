//! NovelVault V1 CLI. Same core as the desktop app; useful for scripting, for CI,
//! and for watching a folder from a terminal while the UI is not open.
//!
//! Every command reads the novel folder and writes only inside `.novelvault/`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use novelvault_core::db::Vault;
use novelvault_core::model::{
    CanonStatus, FileStatus, IdeaStatus, InboxAction, InboxStatus, KnowledgeState, NoteCategory,
    WorkStatus,
};
use novelvault_core::llm::LlmConfig;
use novelvault_core::{addfiles, ai, canon, inbox, reader, scan, store, story, watch};

const USAGE: &str = r#"novelvault — local-first novel vault (V1: folder, files, versions, inbox)

USAGE:
  novelvault init     <novel-folder> [--title "Saqar"]
  novelvault scan     <novel-folder> [--json]
  novelvault files    <novel-folder> [--all] [--json]
  novelvault inbox    <novel-folder> [--all] [--json]
  novelvault open     <novel-folder> [--json]
  novelvault dashboard<novel-folder> [--json]
  novelvault search   <novel-folder> "query" [--limit 20] [--json]
  novelvault classify <novel-folder> <version-id> [--status CURRENT] [--arc "Arc 2"]
                                              [--chapter "Ch 5"] [--version-label "Draft 2"]
  novelvault archive  <novel-folder> <version-id>
  novelvault resolve  <novel-folder> <item-id> <action> [--status CURRENT] [--arc ...]
                        actions: classify | keep_separate | relink | ignore | accept_change | reject_change
  novelvault snapshot <novel-folder> --name "Before Arc 3 Revision" [--with-files]
  novelvault audit    <novel-folder> [--limit 25]
  novelvault watch    <novel-folder> [--seconds 30] [--quiet-ms 1500]

  # bring files in by browsing (Phase 2)
  novelvault new      <parent-folder> --name "Saqar" [--title "Saqar"]
  novelvault add      <novel-folder> --from "/path/a.docx,/path/b.pdf" [--move] [--yes] [--plan]

  # story records (Phase 2)
  novelvault structure    <novel-folder> [--json]
  novelvault arc-add      <novel-folder> <number> --title "Arc 2" [--status Developing]
  novelvault chapter-add  <novel-folder> <arc-number> <number> --title "Wooden Bird"
  novelvault character-add <novel-folder> "Azmirah" [--role protagonist]
  novelvault note-add     <novel-folder> <category> "text" [--link Arc:<id>]
  novelvault idea-add     <novel-folder> "text"
  novelvault idea-set     <novel-folder> <idea-id> <UNDECIDED|UNDER_REVIEW|ACCEPTED|REJECTED> [--reason ""]

  # canon engine (Phase 3)
  novelvault canon          <novel-folder> [--all] [--subject "Keepsake"] [--json]
  novelvault canon-propose  <novel-folder> "Keepsake" "wooden bird" [--category object]
  novelvault canon-source   <novel-folder> <canon-id> <FileVersion|Chapter|Note|...> <source-id>
  novelvault canon-promote  <novel-folder> <canon-id> <PROPOSED|WORKING_CANON|CANON|LOCKED_CANON|REJECTED> --reason ""
  novelvault canon-change   <novel-folder> <canon-id> "new value" --reason ""
  novelvault canon-impact   <novel-folder> <canon-id>
  novelvault ai-status      <novel-folder>
  novelvault ai-setup       <novel-folder> <ollama|openai_local|none> [--model NAME] [--port N]
  novelvault ai-index       <novel-folder>
  novelvault ai-ask         <novel-folder> "question"
  novelvault ai-notice      <novel-folder>       (look for unrecorded names and unreviewed ideas)
  novelvault ai-memory      <novel-folder>
  novelvault read           <novel-folder> [--max N]   (the Reader crawls unread drafts in story order)
  novelvault brain          <novel-folder>             (what the Reader has understood so far)
  novelvault brain-entity   <novel-folder> "Name"      (one character/place in depth)
  novelvault brain-deepen   <novel-folder> [--max N]   (re-read with the local model; claims are checked)
  novelvault canon-history  <novel-folder> <canon-id>
  novelvault knowledge-set  <novel-folder> <character> <canon-id> <KNOWS|DOES_NOT_KNOW|PARTIAL|BELIEVES_FALSELY|UNKNOWN>
  novelvault knowledge      <novel-folder> [<canon-id>]
  novelvault probe          <novel-folder>
  novelvault conflicts      <novel-folder> [--all]
  novelvault resolve-conflict <novel-folder> <conflict-id> <keep_current|accept_change|defer> --reason ""

NOTES:
  * Nothing outside <novel-folder>/.novelvault/ is ever written, renamed, moved or deleted,
    except `add`, which writes only where you have explicitly confirmed it should.
  * Imported files start as UNKNOWN; becoming CURRENT is always an explicit action.
  * Nothing becomes canon on its own (R1), a canon fact needs a source (R3), and no
    previous value is ever destroyed (R2).
"#;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    positional: Vec<String>,
    flags: std::collections::HashMap<String, String>,
    switches: std::collections::HashSet<String>,
}

impl Args {
    fn parse(raw: &[String]) -> Args {
        let mut positional = Vec::new();
        let mut flags = std::collections::HashMap::new();
        let mut switches = std::collections::HashSet::new();
        let mut i = 0;
        while i < raw.len() {
            let token = &raw[i];
            if let Some(name) = token.strip_prefix("--") {
                let key = name.to_string();
                if i + 1 < raw.len() && !raw[i + 1].starts_with("--") {
                    flags.insert(key, raw[i + 1].clone());
                    i += 2;
                } else {
                    switches.insert(key);
                    i += 1;
                }
            } else {
                positional.push(token.clone());
                i += 1;
            }
        }
        Args { positional, flags, switches }
    }

    fn flag(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(|s| s.as_str())
    }
    fn has(&self, name: &str) -> bool {
        self.switches.contains(name)
    }
    fn num(&self, name: &str, fallback: i64) -> i64 {
        self.flag(name).and_then(|v| v.parse().ok()).unwrap_or(fallback)
    }
}

fn run(args: &[String]) -> novelvault_core::Result<()> {
    let command = args[0].as_str();
    let parsed = Args::parse(&args[1..]);
    let json = parsed.has("json");

    let root_of = |index: usize| -> PathBuf {
        parsed
            .positional
            .get(index)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    };

    match command {
        "init" => {
            let root = root_of(0);
            let title = parsed
                .flag("title")
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    root.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Untitled".into())
                });
            let vault = Vault::create(&root, &title)?;
            println!(
                "Vault created for \"{}\"\n  folder : {}\n  data   : {}\n  schema : v{}",
                vault.title,
                root.display(),
                novelvault_core::db::db_path(&root).display(),
                novelvault_core::db::SCHEMA_VERSION
            );
            if json {
                println!(
                    "{}",
                    serde_json::json!({"novel": vault.title, "root": root.display().to_string()})
                );
            }
        }

        "scan" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let summary = scan::scan(&vault)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                println!("{}", summary.headline);
                println!();
                println!("  new imports      {}", summary.imported);
                println!("  unchanged        {}", summary.unchanged);
                println!("  new versions     {}", summary.new_versions);
                println!("  rename candidates{}", summary.rename_candidates);
                println!("  duplicates       {}", summary.duplicates);
                println!("  marked missing   {}", summary.marked_missing);
                println!("  extraction failed{}", summary.extraction_failed);
                println!("  unsupported      {}", summary.extraction_unsupported);
                println!("  scan time        {} ms", summary.duration_ms);
                let pending = inbox::list(&vault, Some(InboxStatus::Open))?;
                println!();
                println!("  {} item(s) waiting in the Inbox", pending.len());
            }
        }

        "files" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let versions = store::list_versions(&vault, parsed.has("all"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&versions)?);
            } else {
                println!(
                    "{:<28} {:>3}  {:<14} {:<12} {:>8}  {}",
                    "FAMILY", "V", "STATUS", "ARTIFACT", "SIZE", "PATH"
                );
                for v in &versions {
                    println!(
                        "{:<28} {:>3}  {:<14} {:<12} {:>8}  {}",
                        truncate(&v.family_name, 28),
                        v.version_number,
                        format!("{} {}", v.status_marker, v.file_status),
                        if v.missing { "⛔ off-disk" } else { &v.extraction_state },
                        novelvault_core::util::human_bytes(v.size_bytes.max(0) as u64),
                        v.rel_path
                    );
                }
                println!("\n{} version(s)", versions.len());
            }
        }

        "inbox" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let status = if parsed.has("all") { None } else { Some(InboxStatus::Open) };
            let items = inbox::list(&vault, status)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else {
                if items.is_empty() {
                    println!("Inbox is empty.");
                }
                for item in &items {
                    println!(
                        "[{}] {}  ({})\n    {}\n    id: {}{}",
                        item.kind,
                        item.kind_label,
                        item.status,
                        item.detail.clone().unwrap_or_default(),
                        item.id,
                        item.rel_path
                            .as_ref()
                            .map(|p| format!("\n    path: {p}"))
                            .unwrap_or_default()
                    );
                    println!();
                }
                println!("{} item(s)", items.len());
            }
        }

        "open" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let report = store::open_report(&vault)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("\"{}\" at {}", report.novel.title, report.root);
                println!("  versions indexed  {}", report.versions);
                println!("  files present     {}", report.resolved);
                println!("  files off-disk    {}", report.missing);
                println!("  hash mismatches   {}", report.hash_mismatches.len());
                for mismatch in &report.hash_mismatches {
                    println!("    ! {mismatch}");
                }
                println!("  portable          yes (all paths are stored relative to the novel folder)");
            }
        }

        "dashboard" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let dash = store::dashboard(&vault)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&dash)?);
            } else {
                println!("{}", dash.novel.title);
                println!("  files                {}", dash.files);
                println!("  versions             {}", dash.versions);
                println!("  marked CURRENT       {}", dash.current_versions);
                println!("  unclassified         {}", dash.unclassified);
                println!("  off-disk (kept)      {}", dash.missing);
                println!("  extraction problems  {}", dash.extraction_failed);
                println!("  snapshots            {}", dash.snapshots);
                println!();
                println!("  INBOX open           {}", dash.inbox_open);
                for (kind, count) in &dash.inbox_by_kind {
                    println!("    {:<20} {}", kind, count);
                }
                println!();
                println!("  {}", dash.counts_note);
            }
        }

        "search" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let query = parsed
                .positional
                .get(1)
                .cloned()
                .ok_or_else(|| novelvault_core::VaultError::Other("search needs a query".into()))?;
            let limit = parsed.num("limit", 20).max(1) as usize;
            let hits = store::search(&vault, &query, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else {
                if hits.is_empty() {
                    println!("No matches for \"{query}\".");
                }
                for hit in &hits {
                    println!(
                        "{} {}  v{}  {:<14} {}\n    {}\n    {}",
                        hit.status_marker,
                        hit.family_name,
                        hit.version_number,
                        hit.file_status,
                        if hit.is_current { "(current)" } else { "" },
                        hit.rel_path,
                        if hit.snippet.is_empty() {
                            format!("matched in {}", hit.matched_in)
                        } else {
                            format!("matched in {}: {}", hit.matched_in, truncate(&hit.snippet, 160))
                        }
                    );
                }
                println!("{} hit(s)", hits.len());
            }
        }

        "classify" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let version_id = parsed
                .positional
                .get(1)
                .cloned()
                .ok_or_else(|| novelvault_core::VaultError::Other("classify needs a version id".into()))?;
            let status = parse_status(parsed.flag("status"))?;
            let version = store::classify(
                &vault,
                &version_id,
                status,
                parsed.flag("arc"),
                parsed.flag("chapter"),
                parsed.flag("version-label"),
            )?;
            println!(
                "{} {} → {} {}", version.family_name, version.version_number, version.status_marker, version.file_status
            );
            if version.is_current {
                println!("This version is now the CURRENT version of its family.");
            }
        }

        "archive" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let version_id = parsed
                .positional
                .get(1)
                .cloned()
                .ok_or_else(|| novelvault_core::VaultError::Other("archive needs a version id".into()))?;
            let version = store::archive(&vault, &version_id)?;
            println!(
                "⚠️ {} {} archived as HISTORICAL — still stored, still searchable, nothing deleted.",
                version.family_name, version.version_number
            );
        }

        "resolve" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let item_id = parsed
                .positional
                .get(1)
                .cloned()
                .ok_or_else(|| novelvault_core::VaultError::Other("resolve needs an item id".into()))?;
            let action_raw = parsed
                .positional
                .get(2)
                .cloned()
                .ok_or_else(|| novelvault_core::VaultError::Other("resolve needs an action".into()))?;
            let action = InboxAction::parse(&action_raw).ok_or_else(|| {
                novelvault_core::VaultError::Other(format!("unknown action '{action_raw}'"))
            })?;
            let res = inbox::Resolution {
                status: parse_status(parsed.flag("status"))?,
                arc: parsed.flag("arc").map(|s| s.to_string()),
                chapter: parsed.flag("chapter").map(|s| s.to_string()),
                version_label: parsed.flag("version-label").map(|s| s.to_string()),
            };
            let item = inbox::resolve(&vault, &item_id, action, res)?;
            println!("{} → {}", item.title, item.status);
            if let Some(used) = item.resolved_action {
                println!("  action: {used}");
            }
        }

        "snapshot" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let name = parsed.flag("name").unwrap_or("Snapshot");
            let record = store::create_snapshot(&vault, name, parsed.has("with-files"), None)?;
            println!("Snapshot created: {} ({})", record.name, record.rel_path);
            let all = store::list_snapshots(&vault)?;
            println!("\nSnapshots");
            for snap in all {
                println!("  {}  {}", snap.created_at, snap.name);
            }
        }

        "audit" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let limit = parsed.num("limit", 25);
            let rows = store::audit_tail(&vault, limit)?;
            for (at, action, entity, id) in rows {
                println!("{at}  {action:<14} {entity:<12} {}", truncate(&id, 8));
            }
            println!("\n{} entr(ies)", limit);
        }

        "watch" => {
            let root = root_of(0);
            let vault = Vault::open(&root)?;
            let seconds = parsed.num("seconds", 30).max(1) as u64;
            let quiet = parsed.num("quiet-ms", 1500).max(0) as u64;
            println!(
                "Watching {} for {}s (quiet window {} ms). Drop a file in to see it indexed.\n",
                root.display(),
                seconds,
                quiet
            );
            let _ = &vault;
            watch::run_watch(&root, quiet, Some(Duration::from_secs(seconds)), |vault, batch| {
                for event in batch {
                    println!("• {} {}", event.kind.as_str(), event.rel_path);
                }
                let ids = scan::apply_events(vault, batch)?;
                for id in ids {
                    if let Ok(item) = inbox::get(vault, &id) {
                        println!("    ↳ inbox [{}] {}  ({})", item.kind, item.title, item.id);
                    }
                }
                Ok(())
            })?;
            let pending = inbox::list(&vault, Some(InboxStatus::Open))?;
            println!("\nWatcher stopped. {} item(s) in the Inbox.", pending.len());
        }

        // -------------------------------------------------------------------
        // Phase 2 — creating a novel, and bringing files into it
        // -------------------------------------------------------------------
        "new" => {
            let parent = root_of(0);
            let name = parsed
                .flag("name")
                .ok_or_else(|| novelvault_core::VaultError::Other("new needs --name \"Saqar\"".into()))?;
            let title = parsed.flag("title").unwrap_or(name);
            let (root, vault) = addfiles::create_novel(&parent, name, Some(title))?;
            story::ensure_reader_sentinel(&vault)?;
            println!(
                "Novel folder ready\n  folder : {}\n  data   : {}\n  reader : one sentinel created (R4)",
                root.display(),
                novelvault_core::db::db_path(&root).display()
            );
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "novel": vault.title,
                        "root": root.display().to_string(),
                        "folder_created": true
                    })
                );
            }
        }

        "add" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let from = parsed.flag("from").unwrap_or("");
            let sources: Vec<String> = if from.trim().is_empty() {
                parsed.positional.iter().skip(1).cloned().collect()
            } else {
                from.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            if sources.is_empty() {
                return Err(novelvault_core::VaultError::Other(
                    "add needs --from \"/path/a.docx,/path/b.pdf\" or file paths".into(),
                ));
            }
            let move_files = parsed.has("move");
            let plan = addfiles::plan(&vault, &sources, move_files)?;
            println!("{}", plan.headline);
            for entry in &plan.entries {
                println!(
                    "  [{}] {}  →  {}",
                    entry.action, entry.source_path, entry.rel_target
                );
                if let Some(note) = &entry.note {
                    println!("        note: {note}");
                }
            }
            for skipped in &plan.skipped {
                println!(
                    "  [SKIP] {}  ({})",
                    skipped.source_path,
                    skipped.note.clone().unwrap_or_default()
                );
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&plan)?);
            }
            if parsed.has("plan") || !parsed.has("yes") {
                println!("\nNothing was changed. Re-run with --yes to apply this plan.");
            } else {
                let outcomes = addfiles::apply(&vault, &sources, move_files, true)?;
                let mut ok = 0;
                for outcome in &outcomes {
                    match &outcome.error {
                        None => {
                            ok += 1;
                            println!(
                                "  copied & verified {}  (sha256 {})",
                                outcome.rel_target,
                                &outcome.sha256[..12.min(outcome.sha256.len())]
                            );
                        }
                        Some(error) => println!("  FAILED {}: {error}", outcome.source_path),
                    }
                }
                println!("\n{ok}/{} file(s) brought in.", outcomes.len());
                println!("They are UNKNOWN and waiting in the Inbox — nothing became CURRENT.");
            }
        }

        "structure" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let arcs = story::list_arcs(&vault)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&arcs)?);
            } else if arcs.is_empty() {
                println!("No arcs recorded yet. Use `arc-add`.");
            } else {
                for arc in &arcs {
                    println!(
                        "Arc {} — {}  [{}]  {} chapter(s), {} word(s)",
                        arc.number, arc.title, arc.status, arc.chapters, arc.words
                    );
                    for chapter in story::list_chapters(&vault, Some(&arc.id))? {
                        println!(
                            "    {:>2}. {}  [{}]  {} word(s)",
                            chapter.number, chapter.title, chapter.status, chapter.word_count
                        );
                    }
                }
            }
        }

        "arc-add" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let number: i64 = parsed
                .positional
                .get(1)
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| novelvault_core::VaultError::Other("arc-add needs an arc number".into()))?;
            let title = parsed.flag("title").unwrap_or("Untitled arc");
            let status = parse_work_status(parsed.flag("status"))?.unwrap_or(WorkStatus::Planned);
            let arc = story::create_arc(&vault, number, title, parsed.flag("purpose"), status)?;
            println!("Arc {} — {}  [{}]  id {}", arc.number, arc.title, arc.status, arc.id);
        }

        "chapter-add" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let arc_number: i64 = parsed
                .positional
                .get(1)
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| novelvault_core::VaultError::Other("chapter-add needs an arc number".into()))?;
            let number: i64 = parsed
                .positional
                .get(2)
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| novelvault_core::VaultError::Other("chapter-add needs a chapter number".into()))?;
            let arc = story::arc_by_number(&vault, arc_number)?;
            let title = parsed.flag("title").unwrap_or("Untitled chapter");
            let status = parse_work_status(parsed.flag("status"))?.unwrap_or(WorkStatus::Planned);
            let chapter = story::create_chapter(
                &vault,
                &arc.id,
                number,
                title,
                parsed.flag("purpose"),
                parsed.flag("emotional-movement"),
                status,
            )?;
            println!(
                "Arc {} Ch {} — {}  [{}]  id {}",
                chapter.arc_number, chapter.number, chapter.title, chapter.status, chapter.id
            );
        }

        "character-add" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let name = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("character-add needs a name".into()))?;
            let character = story::create_character(
                &vault,
                name,
                parsed.flag("role"),
                parsed.flag("description"),
            )?;
            if json {
                println!("{}", serde_json::to_string(&character)?);
            } else {
                println!(
                    "{} — role: {}  id {}",
                    character.name,
                    character.role.clone().unwrap_or_else(|| "—".to_string()),
                    character.id
                );
            }
        }

        "note-add" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let category_raw = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("note-add needs a category".into()))?;
            let category = NoteCategory::parse(category_raw).ok_or_else(|| {
                novelvault_core::VaultError::Other(format!(
                    "unknown note category '{category_raw}'. Expected one of: {}",
                    NoteCategory::ALL
                        .iter()
                        .map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
            let body = parsed
                .positional
                .get(2)
                .ok_or_else(|| novelvault_core::VaultError::Other("note-add needs the note text".into()))?;
            let note = story::create_note(&vault, category, body)?;
            if let Some(link) = parsed.flag("link") {
                if let Some((kind, id)) = link.split_once(':') {
                    story::link_note(&vault, &note.id, kind, id, None)?;
                }
            }
            if json {
                println!("{}", serde_json::to_string(&note)?);
            } else {
                println!("[{}] note {} — {}", note.category, note.id, note.body);
            }
        }

        "idea-add" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let text = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("idea-add needs the idea text".into()))?;
            let idea = story::create_idea(&vault, text)?;
            if json {
                println!("{}", serde_json::to_string(&idea)?);
            } else {
                println!("[{}] idea {} — {}", idea.status, idea.id, idea.text);
            }
        }

        "idea-set" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let idea_id = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("idea-set needs an idea id".into()))?;
            let status_raw = parsed
                .flag("to")
                .or_else(|| parsed.positional.get(2).map(|s| s.as_str()))
                .ok_or_else(|| novelvault_core::VaultError::Other("idea-set needs a status".into()))?;
            let status = IdeaStatus::parse(status_raw).ok_or_else(|| {
                novelvault_core::VaultError::Other(format!(
                    "unknown idea status '{status_raw}'. Expected one of: UNDECIDED, UNDER_REVIEW, ACCEPTED, REJECTED"
                ))
            })?;
            let idea = story::update_idea(&vault, idea_id, status, parsed.flag("reason"))?;
            if json {
                println!("{}", serde_json::to_string(&idea)?);
            } else {
                println!("[{}] idea {} — {}", idea.status, idea.id, idea.text);
            }
        }

        // -------------------------------------------------------------------
        // Phase 3 — the Canon Engine
        // -------------------------------------------------------------------
        "canon" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let include_superseded = parsed.has("all");
            let subject = parsed.flag("subject");
            let items: Vec<_> = canon::list(&vault, None, include_superseded)?
                .into_iter()
                .filter(|i| match subject {
                    None => true,
                    Some(wanted) => i.subject.eq_ignore_ascii_case(wanted),
                })
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else if items.is_empty() {
                println!("No canon recorded yet. Use `canon-propose` — nothing becomes canon on its own.");
            } else {
                for item in &items {
                    println!(
                        "[{}] {} = \"{}\"  ({} source(s)){}",
                        item.status_label,
                        item.subject,
                        item.value,
                        item.sources.len(),
                        if item.is_superseded { "  ← superseded" } else { "" }
                    );
                    for source in &item.sources {
                        println!(
                            "      source: {} {} {}",
                            source.source_type,
                            source.source_id,
                            source.location.clone().unwrap_or_default()
                        );
                    }
                }
            }
        }

        "canon-propose" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let subject = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("canon-propose needs a subject".into()))?;
            let value = parsed
                .positional
                .get(2)
                .ok_or_else(|| novelvault_core::VaultError::Other("canon-propose needs a value".into()))?;
            let item = canon::propose(
                &vault,
                subject,
                value,
                parsed.flag("category"),
                parsed.flag("entity-type"),
                parsed.flag("entity-id"),
                parsed.flag("reason"),
            )?;
            if json {
                println!("{}", serde_json::to_string(&item)?);
            } else {
                println!(
                    "[{}] {} = \"{}\"  id {}\nProposed only. Attach a source, then promote with a reason.",
                    item.status_label, item.subject, item.value, item.id
                );
            }
        }

        "canon-source" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let canon_id = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("canon-source needs a canon id".into()))?;
            let source_type = parsed
                .positional
                .get(2)
                .ok_or_else(|| novelvault_core::VaultError::Other("canon-source needs a source type".into()))?;
            let source_id = parsed
                .positional
                .get(3)
                .ok_or_else(|| novelvault_core::VaultError::Other("canon-source needs a source id".into()))?;
            let source = canon::add_source(
                &vault,
                canon_id,
                source_type,
                source_id,
                parsed.flag("label"),
                parsed.flag("location"),
            )?;
            println!("Source attached: {} {}", source.source_type, source.source_id);
        }

        "canon-promote" | "canon-change" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let canon_id = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other(format!("{command} needs a canon id")))?;
            let reason = parsed.flag("reason").unwrap_or("");
            let item = if command == "canon-promote" {
                let status_raw = parsed
                    .positional
                    .get(2)
                    .ok_or_else(|| novelvault_core::VaultError::Other("canon-promote needs a status".into()))?;
                let target = CanonStatus::parse(status_raw).ok_or_else(|| {
                    novelvault_core::VaultError::Other(format!(
                        "unknown canon status '{status_raw}'. Expected one of: {}",
                        CanonStatus::ALL
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })?;
                canon::promote(&vault, canon_id, target, reason)?
            } else {
                let value = parsed.positional.get(2).ok_or_else(|| {
                    novelvault_core::VaultError::Other("canon-change needs the new value".into())
                })?;
                canon::change_value(&vault, canon_id, value, reason, None)?
            };
            if json {
                println!("{}", serde_json::to_string(&item)?);
            } else {
                println!(
                    "[{}] {} = \"{}\"\nThe previous value is retained as OUTDATED and linked forward (R2).",
                    item.status_label, item.subject, item.value
                );
            }
        }

        "canon-impact" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let canon_id = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("canon-impact needs a canon id".into()))?;
            let report = canon::impact(&vault, canon_id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Impact of changing {} = \"{}\"  [{}]", report.subject, report.value, report.status);
                println!("  {}", report.summary);
                if report.is_locked {
                    println!("  ⚠ this fact is LOCKED CANON — changing it is a deliberate act.");
                }
                for note in &report.affected_notes {
                    println!("  note     {note}");
                }
                for character in &report.affected_characters {
                    println!("  knows    {character}");
                }
                for conflict in &report.affected_conflicts {
                    println!("  conflict {conflict}");
                }
                for file in &report.files_mentioning_old_value {
                    println!("  file     {file}  (contains the current value)");
                }
                for chapter in &report.chapters_mentioning_subject {
                    println!("  chapter  {chapter}");
                }
            }
        }

        "canon-history" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let canon_id = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("canon-history needs a canon id".into()))?;
            let rows = canon::history_for(&vault, canon_id)?;
            for row in &rows {
                println!(
                    "{}  {:14}  {}{}{}",
                    row.at,
                    row.action,
                    row.reason.clone().unwrap_or_default(),
                    if row.before_json.is_some() { "  before: " } else { "" },
                    row.before_json.clone().unwrap_or_default()
                );
                if let Some(after) = &row.after_json {
                    println!("                                      after:  {after}");
                }
            }
        }

        "knowledge-set" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let character = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("knowledge-set needs a character".into()))?;
            let canon_id = parsed
                .positional
                .get(2)
                .ok_or_else(|| novelvault_core::VaultError::Other("knowledge-set needs a canon id".into()))?;
            let state_raw = parsed
                .positional
                .get(3)
                .ok_or_else(|| novelvault_core::VaultError::Other("knowledge-set needs a state".into()))?;
            let state = KnowledgeState::parse(state_raw).ok_or_else(|| {
                novelvault_core::VaultError::Other(format!(
                    "unknown knowledge state '{state_raw}'. Expected one of: KNOWS, DOES_NOT_KNOW, PARTIAL, BELIEVES_FALSELY, UNKNOWN"
                ))
            })?;
            let row = story::character_by_name(&vault, character)?;
            let written = canon::set_knowledge(&vault, &row.id, canon_id, state, parsed.flag("note"))?;
            if json {
                println!("{}", serde_json::to_string(&written)?);
            } else {
                println!("{} {} {}", written.character_name, written.glyph, written.state);
            }
        }

        "knowledge" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let rows = canon::knowledge_rows(&vault, parsed.positional.get(1).map(|s| s.as_str()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else if rows.is_empty() {
                println!("No knowledge recorded yet.");
            } else {
                for row in &rows {
                    println!(
                        "{} {:<14}  {:<16}  {}",
                        row.glyph, row.character_name, row.state, row.subject
                    );
                }
            }
        }

        "ai-status" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let st = ai::status(&vault)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&st)?);
            } else {
                println!("Mode: {}   ({})", st.mode, st.note);
                println!("Indexed: {} files, {} passages ({} waiting)", st.indexed_files, st.indexed_passages, st.pending_files);
            }
        }

        "ai-setup" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let provider = parsed
                .positional
                .get(1)
                .cloned()
                .ok_or_else(|| novelvault_core::VaultError::Other("ai-setup needs a provider: ollama, openai_local or none".into()))?;
            let cfg = LlmConfig {
                port: parsed.num("port", LlmConfig::default_port(&provider) as i64) as u16,
                host: parsed.flag("host").unwrap_or("127.0.0.1").to_string(),
                model: parsed.flag("model").unwrap_or("").to_string(),
                provider,
            };
            ai::save_config(&vault, &cfg)?;
            let st = ai::status(&vault)?;
            println!("Saved. Mode: {}. {}", st.mode, st.note);
        }

        "ai-index" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let r = ai::index_new(&vault)?;
            println!("Indexed {} file(s), {} passage(s); {} unreadable.", r.indexed_files, r.passages, r.skipped_unreadable);
        }

        "ai-ask" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let question = parsed
                .positional
                .get(1)
                .cloned()
                .ok_or_else(|| novelvault_core::VaultError::Other("ai-ask needs a question".into()))?;
            let a = ai::ask(&vault, &question)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&a)?);
            } else {
                println!("{}\n", a.answer);
                for w in &a.warnings {
                    println!("⚠ {w}");
                }
                println!("\n[{} mode{}]", a.mode, a.model.as_ref().map(|m| format!(", {m}")).unwrap_or_default());
                for s in &a.sources {
                    println!("  [{}] {} — {} ({})", s.label, s.title, s.status, s.authority);
                }
            }
        }

        "ai-notice" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let created = ai::scan_suggestions(&vault)?;
            println!("{created} new notice(s). Nothing was added to your story — accept or dismiss each in the app.");
            for s in ai::list_suggestions(&vault, Some("pending"))? {
                println!("  [{}] {} — {}", s.kind, s.title, s.detail);
            }
        }

        "ai-memory" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let m = ai::memory(&vault)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&m)?);
            } else {
                for e in &m.entities {
                    println!("{:<10} {:<24} live: {:<4} older-only: {}", e.kind, e.name, e.passages_live, e.passages_older);
                }
                for f in &m.files {
                    println!("{} v{} [{}] {}", f.rel_path, f.version_number, f.file_status, f.digest);
                }
            }
        }

        "read" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let r = reader::crawl(&vault, parsed.num("max", 50) as usize)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                for s in &r.steps {
                    println!(
                        "read {}  ({} sentences → {} facts, {} events, {} open threads){}",
                        s.rel_path,
                        s.sentences,
                        s.facts,
                        s.events,
                        s.threads,
                        if s.learned.is_empty() { String::new() } else { format!("  learned: {}", s.learned.join(", ")) }
                    );
                }
                println!("{} file(s) read, {} waiting. {} new notice(s).", r.files_read, r.remaining, r.new_notices);
            }
        }

        "brain" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            reader::crawl(&vault, 200)?;
            let u = reader::understanding(&vault)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&u)?);
            } else {
                let c = &u.coverage;
                println!("Read {}/{} files ({} sentences; {} deepened by a model)\n", c.files_read, c.files_total, c.sentences_read, c.files_deepened);
                println!("CAST AND PLACES");
                for e in u.entities.iter().take(25) {
                    println!("  {:<10} {:<24} live {:<4} older {:<4} facts {}", e.kind, e.name, e.mentions_live, e.mentions_older, e.facts);
                }
                println!("\nTHE STORY SO FAR ({})\n{}", u.story_so_far_method, u.story_so_far);
                println!("\nOPEN THREADS");
                for t in u.threads.iter().take(10) {
                    println!("  [{}] {}", t.rel_path, t.excerpt);
                }
                println!("\nCHANGES BETWEEN DRAFTS");
                if u.changes.is_empty() {
                    println!("  none noticed");
                }
                for c in &u.changes {
                    println!("  {} {}: “{}” ({}) → “{}” ({})  [{}]", c.subject, c.predicate, c.old_value, c.old_file, c.new_value, c.new_file, c.kind);
                }
            }
        }

        "brain-entity" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            reader::crawl(&vault, 200)?;
            let name = parsed
                .positional
                .get(1)
                .cloned()
                .ok_or_else(|| novelvault_core::VaultError::Other("brain-entity needs a name".into()))?;
            let p = reader::entity_profile(&vault, &name)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&p)?);
            } else {
                println!("{}\n", p.summary);
                for f in &p.facts {
                    println!("  {} {}  [{}{}]\n      “{}”", f.predicate, f.object, f.rel_path, if f.is_live { "" } else { ", older draft" }, f.excerpt);
                }
                if !p.related.is_empty() {
                    println!("\nOften appears with: {}", p.related.iter().map(|(n, c)| format!("{n} ({c})")).collect::<Vec<_>>().join(", "));
                }
            }
        }

        "brain-deepen" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let n = reader::deepen_with_model(&vault, parsed.num("max", 3) as usize)?;
            println!("Re-read {n} file(s) with the local model. Anything the text did not support was discarded.");
        }

        "probe" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let report = canon::probe(&vault, 50)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report.details)?);
            }
            println!(
                "Checked {} superseded fact(s); raised {} new conflict(s).",
                report.subjects_checked, report.conflicts_raised
            );
            for detail in &report.details {
                println!("  ⚠ {detail}");
            }
            if report.conflicts_raised == 0 {
                println!("No contradictions found. Nothing was changed either way.");
            } else {
                println!("\nNothing was merged or rewritten. Resolve each one with `resolve-conflict`.");
            }
        }

        "conflicts" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let status = if parsed.has("all") { None } else { Some("OPEN") };
            let rows = canon::conflicts(&vault, status)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else if rows.is_empty() {
                println!("No open conflicts.");
            } else {
                for row in &rows {
                    println!(
                        "[{}] {}: canon says \"{}\", {} says \"{}\"",
                        row.status,
                        row.subject,
                        row.current_value.clone().unwrap_or_default(),
                        row.conflicting_source.clone().unwrap_or_default(),
                        row.conflicting_value.clone().unwrap_or_default()
                    );
                    println!("      detector: {}   id: {}", row.detector, row.id);
                    if let Some(resolution) = &row.resolution {
                        println!("      resolved: {resolution}");
                    }
                }
            }
        }

        "resolve-conflict" => {
            let root = root_of(0);
            let vault = Vault::open_or_create(&root, &root_title(&root))?;
            let conflict_id = parsed
                .positional
                .get(1)
                .ok_or_else(|| novelvault_core::VaultError::Other("resolve-conflict needs a conflict id".into()))?;
            let decision = parsed
                .positional
                .get(2)
                .ok_or_else(|| novelvault_core::VaultError::Other("resolve-conflict needs a decision".into()))?;
            canon::resolve_conflict(&vault, conflict_id, decision, parsed.flag("reason").unwrap_or(""))?;
            println!(
                "Recorded: {decision}. Canon itself was not rewritten — change it explicitly if that is the decision."
            );
        }

        other => {
            eprintln!("unknown command '{other}'\n");
            println!("{USAGE}");
            return Err(novelvault_core::VaultError::Other(format!("unknown command '{other}'")));
        }
    }

    Ok(())
}

fn parse_work_status(raw: Option<&str>) -> novelvault_core::Result<Option<WorkStatus>> {
    match raw {
        None => Ok(None),
        Some(value) => WorkStatus::parse(value).map(Some).ok_or_else(|| {
            novelvault_core::VaultError::Other(format!(
                "unknown arc/chapter status '{value}'. Expected one of: Planned, Developing, Drafted, Revised, Locked"
            ))
        }),
    }
}

fn root_title(root: &Path) -> String {
    root.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".to_string())
}

fn parse_status(raw: Option<&str>) -> novelvault_core::Result<Option<FileStatus>> {
    match raw {
        None => Ok(None),
        Some(value) => FileStatus::parse(value).map(Some).ok_or_else(|| {
            novelvault_core::VaultError::Other(format!(
                "unknown file status '{value}'. Expected one of: CURRENT, WORKING_DRAFT, REFERENCE, HISTORICAL, REJECTED, UNKNOWN"
            ))
        }),
    }
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
