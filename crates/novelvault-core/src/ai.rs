//! The local AI librarian.
//!
//! Role: **librarian and editor, never owner.** Everything in this module is read-only
//! with respect to the author's files and canon:
//!
//! * It builds a derived search index (inside `.novelvault/`) over every extracted file.
//! * It answers questions from that index with **numbered sources**, ranked by authority
//!   (the *Canon Shield*): locked canon → canon → current files → drafts → old drafts.
//! * It never decides a conflict, never promotes canon, never touches a file. What it
//!   notices goes into `ai_suggestions`, where a person accepts or dismisses it.
//!
//! The librarian works in two modes:
//!
//! * **model** — a local language model (see `llm`) writes the answer from the sources.
//! * **offline** — no model needed: the same ranked sources are laid out directly. This is
//!   the default, so the app is useful the moment it is installed.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::{self, Vault};
use crate::llm::{self, LlmConfig};
use crate::model::*;
use crate::util::{new_id, now_iso};
use crate::{canon, store, story, Result, VaultError};

const CHUNK_TARGET: usize = 900;
const CONFIG_KEY: &str = "ai.config";

// ---------------------------------------------------------------------------
// Configuration and status
// ---------------------------------------------------------------------------

pub fn load_config(vault: &Vault) -> LlmConfig {
    vault
        .setting_get(CONFIG_KEY)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(vault: &Vault, cfg: &LlmConfig) -> Result<()> {
    llm::validate(cfg)?;
    vault.setting_set(CONFIG_KEY, &serde_json::to_string(cfg)?)?;
    vault.audit("AiConfigure", "Settings", None, None, Some(&format!("{}:{}", cfg.provider, cfg.model)))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct AiStatus {
    pub config: LlmConfig,
    /// "model" when a local model is configured and reachable; otherwise "offline".
    pub mode: String,
    pub reachable: bool,
    pub models: Vec<String>,
    pub note: String,
    pub indexed_files: i64,
    pub indexed_passages: i64,
    pub pending_files: i64,
}

pub fn status(vault: &Vault) -> Result<AiStatus> {
    let config = load_config(vault);
    let (reachable, models, note) = if config.provider == "none" {
        (
            false,
            Vec::new(),
            "No local AI model connected. The librarian still works offline: it finds and ranks passages from your files and canon."
                .to_string(),
        )
    } else {
        match llm::list_models(&config) {
            Ok(models) => {
                let have = config.model.is_empty() || models.iter().any(|m| m == &config.model);
                let note = if models.is_empty() {
                    "The local AI is running but has no models installed yet.".to_string()
                } else if !have {
                    format!("Model '{}' is not installed on the local AI.", config.model)
                } else {
                    "Local AI connected. Your novel stays on this computer.".to_string()
                };
                (have && !models.is_empty(), models, note)
            }
            Err(e) => (false, Vec::new(), e.to_string()),
        }
    };
    let mode = if reachable && config.enabled() { "model" } else { "offline" };
    let (indexed_files, indexed_passages): (i64, i64) = vault.conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(chunk_count),0) FROM ai_indexed",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let pending_files = pending_versions(vault)?.len() as i64;
    Ok(AiStatus {
        config,
        mode: mode.into(),
        reachable,
        models,
        note,
        indexed_files,
        indexed_passages,
        pending_files,
    })
}

// ---------------------------------------------------------------------------
// Indexing — turn extracted text into searchable passages
// ---------------------------------------------------------------------------

/// Split text into passages of roughly `CHUNK_TARGET` characters on paragraph boundaries.
pub fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, chunks: &mut Vec<String>| {
        let t = current.trim();
        if !t.is_empty() {
            chunks.push(t.to_string());
        }
        current.clear();
    };
    for para in text.split('\n').map(str::trim).filter(|p| !p.is_empty()) {
        let pieces: Vec<String> = if para.chars().count() > CHUNK_TARGET * 2 {
            let chars: Vec<char> = para.chars().collect();
            chars.chunks(CHUNK_TARGET).map(|c| c.iter().collect()).collect()
        } else {
            vec![para.to_string()]
        };
        for piece in pieces {
            if !current.is_empty() && current.chars().count() + piece.chars().count() > CHUNK_TARGET {
                flush(&mut current, &mut chunks);
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(&piece);
        }
    }
    flush(&mut current, &mut chunks);
    chunks
}

struct Pending {
    version_id: String,
    sha256: String,
}

fn pending_versions(vault: &Vault) -> Result<Vec<Pending>> {
    let mut stmt = vault.conn.prepare(
        "SELECT v.id, v.sha256 FROM file_versions v
         LEFT JOIN ai_indexed i ON i.version_id = v.id AND i.sha256 = v.sha256
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL AND v.extracted_chars > 0 AND i.version_id IS NULL",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| {
            Ok(Pending { version_id: r.get(0)?, sha256: r.get(1)? })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct IndexReport {
    pub indexed_files: usize,
    pub passages: usize,
    pub skipped_unreadable: usize,
}

/// Incremental: only file versions that are new (or whose hash changed) are read.
/// Safe to call after every scan, import or watcher batch.
pub fn index_new(vault: &Vault) -> Result<IndexReport> {
    let mut report = IndexReport::default();
    for p in pending_versions(vault)? {
        let path = db::index_versions_dir(vault.root.as_path()).join(format!("{}.txt", p.version_id));
        let Ok(text) = std::fs::read_to_string(&path) else {
            report.skipped_unreadable += 1;
            continue;
        };
        let chunks = chunk_text(&text);
        let tx = vault.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM ai_chunks_fts WHERE version_id = ?1", params![p.version_id])?;
        for (i, body) in chunks.iter().enumerate() {
            tx.execute(
                "INSERT INTO ai_chunks_fts (body, version_id, ordinal) VALUES (?1, ?2, ?3)",
                params![body, p.version_id, i as i64],
            )?;
        }
        tx.execute(
            "INSERT INTO ai_indexed (version_id, sha256, chunk_count, indexed_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(version_id) DO UPDATE SET sha256 = excluded.sha256,
                 chunk_count = excluded.chunk_count, indexed_at = excluded.indexed_at",
            params![p.version_id, p.sha256, chunks.len() as i64, now_iso()],
        )?;
        if !chunks.is_empty() {
            let digest = extractive_digest(&chunks[0]);
            tx.execute(
                "INSERT INTO ai_digests (version_id, digest, method, created_at) VALUES (?1, ?2, 'extractive', ?3)
                 ON CONFLICT(version_id) DO UPDATE SET digest = excluded.digest, method = 'extractive',
                     created_at = excluded.created_at",
                params![p.version_id, digest, now_iso()],
            )?;
        }
        tx.commit()?;
        report.indexed_files += 1;
        report.passages += chunks.len();
    }
    if report.indexed_files > 0 {
        vault.audit("AiIndex", "Novel", None, None, Some(&format!("{} files, {} passages", report.indexed_files, report.passages)))?;
    }
    Ok(report)
}

fn extractive_digest(first_chunk: &str) -> String {
    let flat: String = first_chunk.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for sentence in flat.split_inclusive(['.', '!', '?']) {
        if out.chars().count() + sentence.chars().count() > 300 && !out.is_empty() {
            break;
        }
        out.push_str(sentence);
        if out.chars().count() >= 160 {
            break;
        }
    }
    if out.chars().count() > 320 {
        out = out.chars().take(320).collect::<String>() + "…";
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Retrieval — the Canon Shield
// ---------------------------------------------------------------------------

const STOP: &[&str] = &[
    "the", "and", "for", "are", "was", "were", "what", "which", "who", "whom", "when", "where", "why", "how", "does",
    "did", "this", "that", "these", "those", "with", "from", "into", "about", "have", "has", "had", "her", "his",
    "she", "him", "they", "them", "their", "its", "our", "your", "you", "any", "all", "can", "could", "would",
    "should", "tell", "show", "find", "give", "there", "then", "than", "also", "not", "but", "out", "off", "just",
    "current", "latest", "say", "says", "said", "mention", "mentioned", "file", "files", "novel", "story",
];

pub fn tokens(question: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    question
        .split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.chars().count() >= 3 && !STOP.contains(&t.as_str()))
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

fn fts_query(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|t| if t.chars().count() >= 4 { format!("\"{t}\"*") } else { format!("\"{t}\"") })
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[derive(Debug, Clone, Serialize)]
pub struct Source {
    /// "S1", "S2", … — what the answer cites.
    pub label: String,
    /// "canon" or "file".
    pub kind: String,
    pub title: String,
    /// Canon status for canon, file status for files.
    pub status: String,
    /// Plain-language rank in the Canon Shield.
    pub authority: String,
    pub text: String,
    pub detail: Option<String>,
    pub version_id: Option<String>,
    pub canon_id: Option<String>,
    pub is_current: bool,
    pub warning: Option<String>,
    pub score: f64,
}

fn file_weight(status: &str) -> (f64, &'static str) {
    match status {
        "CURRENT" => (1.0, "current file"),
        "WORKING_DRAFT" => (0.85, "working draft"),
        "REFERENCE" => (0.7, "reference"),
        "UNKNOWN" => (0.6, "unclassified"),
        "HISTORICAL" => (0.45, "old draft"),
        "REJECTED" => (0.3, "rejected"),
        _ => (0.5, "file"),
    }
}

fn canon_rank(status: &str) -> (i32, &'static str) {
    match CanonStatus::parse(status).unwrap_or(CanonStatus::Unknown) {
        CanonStatus::LockedCanon => (7, "locked canon"),
        CanonStatus::Canon => (6, "canon"),
        CanonStatus::WorkingCanon => (5, "working canon"),
        CanonStatus::Draft => (3, "draft canon"),
        CanonStatus::Proposed => (2, "proposed"),
        CanonStatus::Unknown => (1, "unconfirmed"),
        CanonStatus::Outdated => (0, "outdated canon"),
        CanonStatus::Rejected => (0, "rejected"),
    }
}

fn excerpt(body: &str, toks: &[String], max: usize) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() <= max {
        return flat;
    }
    let lower: String = flat.to_lowercase();
    let byte_at = toks.iter().filter_map(|t| lower.find(t.as_str())).min().unwrap_or(0);
    let char_at = lower[..byte_at.min(lower.len())].chars().count();
    let start = char_at.saturating_sub(max / 3);
    let end = (start + max).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s = format!("…{s}");
    }
    if end < chars.len() {
        s.push('…');
    }
    s
}

/// Canon first, then files — each carrying the warning it deserves.
pub fn retrieve(vault: &Vault, question: &str, max_files: usize) -> Result<Vec<Source>> {
    let toks = tokens(question);
    if toks.is_empty() {
        return Ok(Vec::new());
    }
    let mut out: Vec<Source> = Vec::new();

    // 1. Canon
    let mut canon_hits: Vec<(i32, usize, CanonItem)> = Vec::new();
    for item in canon::list(vault, None, true)? {
        let hay = format!("{} {}", item.subject, item.value).to_lowercase();
        let overlap = toks.iter().filter(|t| hay.contains(t.as_str())).count();
        if overlap == 0 {
            continue;
        }
        let (rank, _) = canon_rank(&item.status);
        canon_hits.push((rank, overlap, item));
    }
    canon_hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    for (rank, overlap, item) in canon_hits.into_iter().take(6) {
        let (_, authority) = canon_rank(&item.status);
        let sources = item
            .sources
            .iter()
            .map(|s| s.source_label.clone().unwrap_or_else(|| format!("{} {}", s.source_type, s.source_id)))
            .collect::<Vec<_>>();
        let warning = match CanonStatus::parse(&item.status).unwrap_or(CanonStatus::Unknown) {
            CanonStatus::Outdated => {
                let now = item
                    .superseded_by_id
                    .as_deref()
                    .and_then(|id| canon::get(vault, id).ok())
                    .map(|n| format!(" The current value is \"{}\".", n.value))
                    .unwrap_or_default();
                Some(format!("Outdated canon — superseded by a later decision.{now}"))
            }
            CanonStatus::Rejected => Some("Rejected — you decided against this.".into()),
            s if !s.requires_source() && s != CanonStatus::Draft => {
                Some("Not canon yet — still only proposed or unconfirmed.".into())
            }
            _ => None,
        };
        out.push(Source {
            label: String::new(),
            kind: "canon".into(),
            title: item.subject.clone(),
            status: item.status_label.clone(),
            authority: authority.into(),
            text: format!("{} = {}", item.subject, item.value),
            detail: if sources.is_empty() { None } else { Some(format!("evidence: {}", sources.join("; "))) },
            version_id: None,
            canon_id: Some(item.id.clone()),
            is_current: item.status != "OUTDATED" && item.status != "REJECTED",
            warning,
            score: (rank * 10 + overlap as i32) as f64,
        });
    }

    // 2. Files, BM25 then authority-weighted
    let query = fts_query(&toks);
    let mut stmt = vault.conn.prepare(
        "SELECT version_id, ordinal, body, bm25(ai_chunks_fts) FROM ai_chunks_fts
         WHERE ai_chunks_fts MATCH ?1 ORDER BY bm25(ai_chunks_fts) LIMIT 80",
    )?;
    let rows = stmt
        .query_map(params![query], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut cache: HashMap<String, Option<FileVersion>> = HashMap::new();
    let mut scored: Vec<(f64, FileVersion, String, i64)> = Vec::new();
    for (vid, ordinal, body, bm) in rows {
        let version = cache
            .entry(vid.clone())
            .or_insert_with(|| store::get_version(vault, &vid).ok())
            .clone();
        let Some(version) = version else { continue };
        let (w, _) = file_weight(&version.file_status);
        let w = if version.missing { w * 0.8 } else { w };
        let current_bonus = if version.is_current { 1.15 } else { 1.0 };
        scored.push((-bm * w * current_bonus, version, body, ordinal));
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut per_file: HashMap<String, usize> = HashMap::new();
    for (score, version, body, ordinal) in scored {
        if out.iter().filter(|s| s.kind == "file").count() >= max_files {
            break;
        }
        let n = per_file.entry(version.id.clone()).or_insert(0);
        if *n >= 2 {
            continue;
        }
        *n += 1;
        let (_, authority) = file_weight(&version.file_status);
        let current_of_family: Option<i64> = vault
            .conn
            .query_row(
                "SELECT version_number FROM file_versions WHERE file_id = ?1 AND is_current = 1 AND deleted_at IS NULL",
                params![version.file_id],
                |r| r.get(0),
            )
            .optional()?;
        let warning = if version.file_status == "REJECTED" {
            Some("You rejected this file.".to_string())
        } else if version.file_status == "HISTORICAL" {
            Some("Archived draft — may be outdated.".to_string())
        } else if !version.is_current && current_of_family.is_some() {
            Some(format!(
                "Older version — the current version of this file is v{}.",
                current_of_family.unwrap_or_default()
            ))
        } else if version.file_status == "UNKNOWN" {
            Some("Unclassified file — not yet confirmed as a draft or reference.".to_string())
        } else if version.missing {
            Some("This file is no longer on disk; the record was kept.".to_string())
        } else {
            None
        };
        out.push(Source {
            label: String::new(),
            kind: "file".into(),
            title: version.rel_path.clone(),
            status: version.file_status.clone(),
            authority: authority.into(),
            text: excerpt(&body, &toks, 520),
            detail: Some(format!(
                "v{} · passage {}{}{}",
                version.version_number,
                ordinal + 1,
                version.arc_label.as_ref().map(|a| format!(" · {a}")).unwrap_or_default(),
                version.chapter_label.as_ref().map(|c| format!(" · {c}")).unwrap_or_default()
            )),
            version_id: Some(version.id.clone()),
            canon_id: None,
            is_current: version.is_current,
            warning,
            score,
        });
    }

    for (i, s) in out.iter_mut().enumerate() {
        s.label = format!("S{}", i + 1);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Asking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    pub id: String,
    pub question: String,
    pub answer: String,
    /// "model" or "offline"
    pub mode: String,
    pub model: Option<String>,
    pub sources: Vec<Source>,
    /// Labels the answer actually cites (model mode).
    pub cited: Vec<String>,
    pub warnings: Vec<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub mode: Option<String>,
    pub sources: Vec<Source>,
    pub created_at: String,
}

const SYSTEM_PROMPT: &str = "You are the librarian for one author's novel. You help them keep track of their own story.\n\
RULES:\n\
1. Answer ONLY from the numbered SOURCES provided. If they do not contain the answer, say plainly that it is not in the author's files or canon. Never invent story facts.\n\
2. Cite every claim with its source label, like [S2]. Use several labels if several sources agree.\n\
3. Authority order: LOCKED CANON, CANON, WORKING CANON, then CURRENT files, then drafts, then old or archived drafts, then ideas. Prefer the higher authority.\n\
4. If sources disagree, say so openly, state which one has higher authority, and do NOT silently choose. Sources marked OUTDATED, HISTORICAL, REJECTED or 'older version' describe what the story USED to say; present them as history, never as the current truth.\n\
5. You are an assistant, not the author. You never change files, never decide canon, never rewrite the story. You may suggest; the author decides.\n\
6. Sources marked READER'S NOTES are what the author's own files appear to say, read automatically; use them for plot and character questions, but they are never canon and rank below canon and files.\n\
7. Be concise and concrete.";

fn build_context(sources: &[Source]) -> String {
    let mut s = String::new();
    for src in sources {
        s.push_str(&format!(
            "[{}] {} — {} ({}){}\n{}\n\n",
            src.label,
            match src.kind.as_str() {
                "canon" => "CANON FACT",
                "understanding" => "READER'S NOTES (derived from the files, not canon)",
                _ => "FILE",
            },
            src.title,
            src.status,
            src.warning.as_ref().map(|w| format!(" ⚠ {w}")).unwrap_or_default(),
            src.text
        ));
    }
    s
}

fn offline_answer(question: &str, sources: &[Source], note: Option<&str>) -> String {
    if sources.is_empty() {
        return format!(
            "I found nothing about “{}” in your canon or in any indexed file.\n\
             If you expect it to be there, check that the file has been scanned and that its text could be extracted (scanned PDFs have no text).",
            question.trim()
        );
    }
    let mut out = String::new();
    out.push_str(match note {
        Some(n) => format!("{n}\n\n"),
        None => "No language model is connected, so this is read straight from your canon and files, ranked by authority.\n\n".into(),
    }
    .as_str());
    let canon_srcs: Vec<&Source> = sources.iter().filter(|s| s.kind == "canon").collect();
    let file_srcs: Vec<&Source> = sources.iter().filter(|s| s.kind == "file").collect();
    let reader_srcs: Vec<&Source> = sources.iter().filter(|s| s.kind == "understanding").collect();
    if !canon_srcs.is_empty() {
        out.push_str("CANON\n");
        for s in canon_srcs {
            out.push_str(&format!("• [{}] {} — {}", s.label, s.text, s.status));
            if let Some(w) = &s.warning {
                out.push_str(&format!("  ⚠ {w}"));
            }
            out.push('\n');
        }
        out.push('\n');
    }
    if !file_srcs.is_empty() {
        out.push_str("IN YOUR FILES\n");
        for s in file_srcs {
            out.push_str(&format!("• [{}] {} ({})\n  “{}”\n", s.label, s.title, s.status, s.text));
            if let Some(w) = &s.warning {
                out.push_str(&format!("  ⚠ {w}\n"));
            }
        }
        out.push('\n');
    }
    if !reader_srcs.is_empty() {
        out.push_str("WHAT THE READER HAS LEARNED (derived from your files, not canon)\n");
        for s in reader_srcs {
            out.push_str(&format!("• [{}] {}\n", s.label, s.title));
            for line in s.text.lines() {
                out.push_str(&format!("    {line}\n"));
            }
            if let Some(w) = &s.warning {
                out.push_str(&format!("  ⚠ {w}\n"));
            }
        }
    }
    out.trim_end().to_string()
}

fn cited_labels(answer: &str, sources: &[Source]) -> Vec<String> {
    sources
        .iter()
        .filter(|s| answer.contains(&format!("[{}]", s.label)))
        .map(|s| s.label.clone())
        .collect()
}

fn warnings_for(sources: &[Source], cited: &[String], model_mode: bool) -> Vec<String> {
    let mut w: Vec<String> = Vec::new();
    // A change in canon is worth saying out loud wherever it appears.
    for s in sources.iter().filter(|s| s.kind == "canon" && s.status == "OUTDATED") {
        if let Some(t) = &s.warning {
            w.push(format!("“{}” — {}", s.title, t));
        }
    }
    let used: Vec<&Source> = if model_mode && !cited.is_empty() {
        sources.iter().filter(|s| cited.contains(&s.label)).collect()
    } else {
        sources.iter().collect()
    };
    if !used.is_empty() && used.iter().all(|s| !s.is_current || s.warning.is_some()) {
        w.push("Every source behind this answer is old, unconfirmed or outdated. Treat it as history, not current truth.".into());
    }
    if model_mode && cited.is_empty() {
        w.push("The model did not cite any source, so check this answer against the sources listed below.".into());
    }
    let mut seen = HashSet::new();
    w.retain(|x| seen.insert(x.clone()));
    w
}

pub fn ask(vault: &Vault, question: &str) -> Result<Answer> {
    let question = question.trim();
    if question.is_empty() {
        return Err(VaultError::Other("ask a question first".into()));
    }
    index_new(vault)?;
    // The Reader learns from anything new before answering; best-effort, never blocks an answer.
    let _ = crate::reader::crawl(vault, 10);
    let mut sources = retrieve(vault, question, 6)?;
    // What the Reader has understood comes after canon and files: derived, never authoritative.
    if let Ok(extra) = crate::reader::understanding_sources(vault, question, 2) {
        sources.extend(extra);
    }
    for (i, s) in sources.iter_mut().enumerate() {
        s.label = format!("S{}", i + 1);
    }
    let cfg = load_config(vault);

    let mut mode = "offline";
    let mut note: Option<String> = None;
    let mut answer_text = String::new();

    if cfg.enabled() && !sources.is_empty() {
        let mut messages: Vec<(String, String)> = Vec::new();
        for m in history(vault, 6)? {
            if m.role == "user" || m.role == "assistant" {
                messages.push((m.role, m.content.chars().take(1200).collect()));
            }
        }
        messages.push((
            "user".into(),
            format!("SOURCES\n\n{}QUESTION\n{}", build_context(&sources), question),
        ));
        match llm::chat(&cfg, SYSTEM_PROMPT, &messages) {
            Ok(text) => {
                mode = "model";
                answer_text = text;
            }
            Err(e) => {
                note = Some(format!("The local AI could not answer ({e}). Showing your sources directly instead."));
            }
        }
    }
    if mode == "offline" {
        answer_text = offline_answer(question, &sources, note.as_deref());
    }

    let cited = if mode == "model" { cited_labels(&answer_text, &sources) } else { Vec::new() };
    let warnings = warnings_for(&sources, &cited, mode == "model");
    let id = new_id();
    let now = now_iso();
    vault.conn.execute(
        "INSERT INTO ai_messages (id, novel_id, role, content, sources_json, mode, created_at)
         VALUES (?1, ?2, 'user', ?3, NULL, NULL, ?4)",
        params![new_id(), vault.novel_id, question, now],
    )?;
    vault.conn.execute(
        "INSERT INTO ai_messages (id, novel_id, role, content, sources_json, mode, created_at)
         VALUES (?1, ?2, 'assistant', ?3, ?4, ?5, ?6)",
        params![id, vault.novel_id, answer_text, serde_json::to_string(&sources)?, mode, now_iso()],
    )?;
    vault.audit("AiAsk", "Novel", Some(&id), None, Some(question))?;

    Ok(Answer {
        id,
        question: question.to_string(),
        answer: answer_text,
        mode: mode.into(),
        model: if mode == "model" { Some(cfg.model.clone()) } else { None },
        sources,
        cited,
        warnings,
        note,
    })
}

pub fn history(vault: &Vault, limit: i64) -> Result<Vec<ChatMessage>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, role, content, mode, sources_json, created_at FROM ai_messages
         WHERE novel_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2",
    )?;
    let mut rows = stmt
        .query_map(params![vault.novel_id, limit], |r| {
            let sources_json: Option<String> = r.get(4)?;
            Ok(ChatMessage {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
                mode: r.get(3)?,
                sources: sources_json.and_then(|j| serde_json::from_str::<Vec<SourceOwned>>(&j).ok()).map(|v| v.into_iter().map(Into::into).collect()).unwrap_or_default(),
                created_at: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.reverse();
    Ok(rows)
}

pub fn clear_history(vault: &Vault) -> Result<usize> {
    let n = vault.conn.execute("DELETE FROM ai_messages WHERE novel_id = ?1", params![vault.novel_id])?;
    vault.audit("AiClearHistory", "Novel", None, None, Some(&n.to_string()))?;
    Ok(n)
}

// Serde round-trip type for stored sources (Source only derives Serialize).
#[derive(serde::Deserialize)]
struct SourceOwned {
    label: String,
    kind: String,
    title: String,
    status: String,
    authority: String,
    text: String,
    detail: Option<String>,
    version_id: Option<String>,
    canon_id: Option<String>,
    is_current: bool,
    warning: Option<String>,
    score: f64,
}

impl From<SourceOwned> for Source {
    fn from(s: SourceOwned) -> Self {
        Source {
            label: s.label,
            kind: s.kind,
            title: s.title,
            status: s.status,
            authority: s.authority,
            text: s.text,
            detail: s.detail,
            version_id: s.version_id,
            canon_id: s.canon_id,
            is_current: s.is_current,
            warning: s.warning,
            score: s.score,
        }
    }
}

// ---------------------------------------------------------------------------
// Novel memory — what the app has learned about the novel as it grows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FileMemory {
    pub version_id: String,
    pub family_name: String,
    pub rel_path: String,
    pub version_number: i64,
    pub file_status: String,
    pub is_current: bool,
    pub arc_label: Option<String>,
    pub chapter_label: Option<String>,
    pub digest: String,
    pub method: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityMemory {
    pub kind: String,
    pub name: String,
    /// Passages in current / working / reference files.
    pub passages_live: i64,
    /// Passages that only appear in old, rejected or unclassified files.
    pub passages_older: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryReport {
    pub files: Vec<FileMemory>,
    pub entities: Vec<EntityMemory>,
    pub counts: BTreeMap<String, i64>,
    pub pending_suggestions: i64,
}

pub fn memory(vault: &Vault) -> Result<MemoryReport> {
    index_new(vault)?;
    let mut files = Vec::new();
    let mut stmt = vault.conn.prepare(
        "SELECT v.id, f.family_name, v.rel_path, v.version_number, v.file_status, v.is_current,
                v.arc_label, v.chapter_label, d.digest, d.method
         FROM file_versions v JOIN files f ON f.id = v.file_id
         JOIN ai_digests d ON d.version_id = v.id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL
         ORDER BY v.is_current DESC, v.updated_at DESC LIMIT 200",
    )?;
    let rows = stmt.query_map(params![vault.novel_id], |r| {
        Ok(FileMemory {
            version_id: r.get(0)?,
            family_name: r.get(1)?,
            rel_path: r.get(2)?,
            version_number: r.get(3)?,
            file_status: r.get(4)?,
            is_current: r.get::<_, i64>(5)? == 1,
            arc_label: r.get(6)?,
            chapter_label: r.get(7)?,
            digest: r.get(8)?,
            method: r.get(9)?,
        })
    })?;
    for r in rows {
        files.push(r?);
    }

    let live = |status: &str| matches!(status, "CURRENT" | "WORKING_DRAFT" | "REFERENCE");
    let mut status_of: HashMap<String, String> = HashMap::new();
    let mut entity_rows: Vec<(String, String)> = Vec::new();
    for c in story::list_characters(vault)?.into_iter().filter(|c| !c.is_reader_sentinel) {
        entity_rows.push(("character".into(), c.name));
    }
    for l in story::list_locations(vault)? {
        entity_rows.push(("location".into(), l.name));
    }
    for o in story::list_objects(vault)? {
        entity_rows.push(("object".into(), o.name));
    }
    let mut entities = Vec::new();
    for (kind, name) in entity_rows {
        let phrase = name.replace('"', "");
        if phrase.trim().is_empty() {
            continue;
        }
        let mut stmt = vault
            .conn
            .prepare("SELECT version_id FROM ai_chunks_fts WHERE ai_chunks_fts MATCH ?1")?;
        let vids = stmt
            .query_map(params![format!("\"{phrase}\"")], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let (mut a, mut b) = (0i64, 0i64);
        for vid in vids {
            let st = status_of
                .entry(vid.clone())
                .or_insert_with(|| store::get_version(vault, &vid).map(|v| v.file_status).unwrap_or_default())
                .clone();
            if live(&st) {
                a += 1;
            } else {
                b += 1;
            }
        }
        entities.push(EntityMemory { kind, name, passages_live: a, passages_older: b });
    }

    let mut counts = story::counts(vault)?;
    for (k, v) in canon::counts(vault)? {
        counts.insert(format!("canon_{k}"), v);
    }
    let pending_suggestions: i64 = vault.conn.query_row(
        "SELECT COUNT(*) FROM ai_suggestions WHERE novel_id = ?1 AND status = 'pending'",
        params![vault.novel_id],
        |r| r.get(0),
    )?;
    Ok(MemoryReport { files, entities, counts, pending_suggestions })
}

/// Replace the plain extractive digests of live files with model-written ones.
/// Bounded per call so it never ties the app up; safe to call repeatedly.
pub fn summarize_with_model(vault: &Vault, max_files: usize) -> Result<usize> {
    let cfg = load_config(vault);
    if !cfg.enabled() {
        return Err(VaultError::Other("connect a local AI model first".into()));
    }
    let mut stmt = vault.conn.prepare(
        "SELECT v.id FROM file_versions v JOIN ai_digests d ON d.version_id = v.id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL AND d.method = 'extractive'
           AND v.file_status IN ('CURRENT','WORKING_DRAFT')
         ORDER BY v.updated_at DESC LIMIT ?2",
    )?;
    let ids = stmt
        .query_map(params![vault.novel_id, max_files as i64], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut done = 0;
    for id in ids {
        let path = db::index_versions_dir(vault.root.as_path()).join(format!("{id}.txt"));
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let sample: String = text.chars().take(6000).collect();
        let msg = vec![(
            "user".to_string(),
            format!("Summarise what happens in this passage of a novel in 2 or 3 plain sentences. Use only the text. Do not add anything.\n\n{sample}"),
        )];
        let summary = llm::chat(&cfg, "You summarise fiction drafts accurately and briefly for their author.", &msg)?;
        vault.conn.execute(
            "UPDATE ai_digests SET digest = ?2, method = 'model', created_at = ?3 WHERE version_id = ?1",
            params![id, summary, now_iso()],
        )?;
        done += 1;
    }
    if done > 0 {
        vault.audit("AiSummarize", "Novel", None, None, Some(&format!("{done} files")))?;
    }
    Ok(done)
}

// ---------------------------------------------------------------------------
// Suggestions — the assistant notices, the author decides
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub evidence: Vec<String>,
    pub status: String,
    pub decision_note: Option<String>,
    pub created_at: String,
}

const NAME_STOP: &[&str] = &[
    "the", "and", "but", "she", "her", "his", "him", "they", "them", "their", "there", "then", "this", "that",
    "these", "those", "what", "when", "where", "who", "why", "how", "with", "from", "into", "have", "had", "was",
    "were", "not", "now", "one", "two", "all", "any", "for", "you", "your", "our", "its", "yes", "monday",
    "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "january", "february", "march", "april",
    "may", "june", "july", "august", "september", "october", "november", "december", "arc", "chapter", "draft",
    "scene", "part", "note", "notes", "idea", "ideas", "version", "revised", "old", "new", "current", "he",
];

/// Capitalised words that keep turning up mid-sentence — likely proper nouns.
pub(crate) fn name_candidates(text: &str) -> HashMap<String, (usize, usize)> {
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new(); // (mid-sentence, total)
    let mut sentence_start = true;
    for raw in text.split_whitespace() {
        let word: String = raw.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
        let first = word.chars().next();
        let looks_like_name = word.chars().count() >= 3
            && first.map(|c| c.is_uppercase()).unwrap_or(false)
            && word.chars().skip(1).any(|c| c.is_lowercase())
            && word.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '-');
        if looks_like_name {
            let e = counts.entry(word.clone()).or_insert((0, 0));
            e.1 += 1;
            if !sentence_start {
                e.0 += 1;
            }
        }
        let ends = raw.ends_with(['.', '!', '?', ':', '"', '”', '’']);
        sentence_start = ends;
    }
    counts
}

pub fn scan_suggestions(vault: &Vault) -> Result<usize> {
    index_new(vault)?;
    let known: HashSet<String> = {
        let mut k = HashSet::new();
        let mut add = |name: &str| {
            for part in name.split_whitespace() {
                k.insert(part.to_lowercase());
            }
            k.insert(name.to_lowercase());
        };
        for c in story::list_characters(vault)? {
            add(&c.name);
        }
        for l in story::list_locations(vault)? {
            add(&l.name);
        }
        for o in story::list_objects(vault)? {
            add(&o.name);
        }
        k
    };

    let mut stmt = vault.conn.prepare(
        "SELECT c.version_id, c.body FROM ai_chunks_fts c
         JOIN file_versions v ON v.id = c.version_id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL
           AND v.file_status IN ('CURRENT','WORKING_DRAFT','REFERENCE','UNKNOWN')",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut totals: HashMap<String, (usize, usize)> = HashMap::new();
    let mut where_seen: HashMap<String, Vec<String>> = HashMap::new();
    for (vid, body) in &rows {
        for (word, (mid, total)) in name_candidates(body) {
            let e = totals.entry(word.clone()).or_insert((0, 0));
            e.0 += mid;
            e.1 += total;
            let list = where_seen.entry(word).or_default();
            if !list.contains(vid) && list.len() < 3 {
                list.push(vid.clone());
            }
        }
    }
    let mut candidates: Vec<(String, usize, usize)> = totals
        .into_iter()
        .filter(|(w, (mid, total))| {
            let lw = w.to_lowercase();
            *mid >= 2 && *total >= 3 && !NAME_STOP.contains(&lw.as_str()) && !known.contains(&lw)
        })
        .map(|(w, (mid, total))| (w, mid, total))
        .collect();
    candidates.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    candidates.truncate(15);

    let mut created = 0;
    for (word, _mid, total) in candidates {
        let evidence: Vec<String> = where_seen
            .get(&word)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| store::get_version(vault, id).ok().map(|v| v.rel_path))
                    .collect()
            })
            .unwrap_or_default();
        created += insert_suggestion(
            vault,
            "new_entity",
            &word,
            &format!(
                "“{word}” appears {total} times in your live files but is not recorded as a character, location or object."
            ),
            &evidence,
        )?;
    }

    for idea in story::list_ideas(vault, Some(IdeaStatus::Undecided))?.into_iter().take(10) {
        let title: String = idea.text.chars().take(80).collect();
        created += insert_suggestion(
            vault,
            "idea_pending",
            &title,
            "A captured idea that has not been reviewed yet — it is not canon until you decide.",
            &[],
        )?;
    }
    if created > 0 {
        vault.audit("AiSuggest", "Novel", None, None, Some(&format!("{created} new")))?;
    }
    Ok(created)
}

fn insert_suggestion(vault: &Vault, kind: &str, title: &str, detail: &str, evidence: &[String]) -> Result<usize> {
    let n = vault.conn.execute(
        "INSERT OR IGNORE INTO ai_suggestions (id, novel_id, kind, title, detail, evidence_json, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
        params![new_id(), vault.novel_id, kind, title, detail, serde_json::to_string(evidence)?, now_iso()],
    )?;
    Ok(n)
}

pub fn list_suggestions(vault: &Vault, status: Option<&str>) -> Result<Vec<Suggestion>> {
    let mut stmt = vault.conn.prepare(
        "SELECT id, kind, title, detail, evidence_json, status, decision_note, created_at FROM ai_suggestions
         WHERE novel_id = ?1 AND (?2 IS NULL OR status = ?2) ORDER BY created_at DESC, title",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id, status], |r| {
            let ev: Option<String> = r.get(4)?;
            Ok(Suggestion {
                id: r.get(0)?,
                kind: r.get(1)?,
                title: r.get(2)?,
                detail: r.get(3)?,
                evidence: ev.and_then(|e| serde_json::from_str(&e).ok()).unwrap_or_default(),
                status: r.get(5)?,
                decision_note: r.get(6)?,
                created_at: r.get(7)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The author's decision. `as_kind` ("character" | "location" | "object") is only used
/// when accepting a `new_entity` suggestion — and only then is a story record created,
/// at the author's explicit request. It is a story record, not canon (R1).
pub fn decide_suggestion(
    vault: &Vault,
    id: &str,
    decision: &str,
    note: Option<&str>,
    as_kind: Option<&str>,
) -> Result<Suggestion> {
    if decision != "accepted" && decision != "dismissed" {
        return Err(VaultError::Other("decision must be 'accepted' or 'dismissed'".into()));
    }
    let s = list_suggestions(vault, None)?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| VaultError::NotFound(format!("suggestion {id}")))?;
    if s.status != "pending" {
        return Err(VaultError::Other("that suggestion has already been decided".into()));
    }
    if decision == "accepted" && s.kind == "new_entity" {
        match as_kind.unwrap_or("character") {
            "character" => {
                story::create_character(vault, &s.title, None, Some("Added from an AI suggestion — please review."))?;
            }
            "location" => {
                story::create_location(vault, &s.title, Some("Added from an AI suggestion — please review."))?;
            }
            "object" => {
                story::create_object(vault, &s.title, Some("Added from an AI suggestion — please review."))?;
            }
            other => return Err(VaultError::Other(format!("unknown record kind '{other}'"))),
        }
    }
    vault.conn.execute(
        "UPDATE ai_suggestions SET status = ?2, decision_note = ?3, decided_at = ?4 WHERE id = ?1",
        params![id, decision, note, now_iso()],
    )?;
    vault.audit("AiSuggestionDecided", "Suggestion", Some(id), Some("pending"), Some(decision))?;
    list_suggestions(vault, None)?
        .into_iter()
        .find(|x| x.id == id)
        .ok_or_else(|| VaultError::NotFound(format!("suggestion {id}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_respect_paragraphs_and_size() {
        let text = format!("{}\n{}\n\n{}", "a".repeat(500), "b".repeat(500), "c".repeat(100));
        let chunks = chunk_text(&text);
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|c| c.chars().count() <= CHUNK_TARGET * 2));
    }

    #[test]
    fn a_long_unbroken_paragraph_is_split_without_breaking_characters() {
        let text = "é".repeat(5000);
        let chunks = chunk_text(&text);
        assert!(chunks.len() > 1);
        assert_eq!(chunks.iter().map(|c| c.chars().count()).sum::<usize>(), 5000);
    }

    #[test]
    fn question_tokens_drop_filler() {
        assert_eq!(tokens("Where did we establish the wooden bird?"), vec!["establish", "wooden", "bird"]);
    }

    #[test]
    fn names_are_found_mid_sentence_not_at_sentence_starts() {
        let text = "Maheen looked away. Then she saw Maheen again. Azmirah told Maheen nothing. Quietly she left.";
        let found = name_candidates(text);
        let (mid, total) = found["Maheen"];
        assert_eq!(total, 3);
        assert_eq!(mid, 2);
        assert!(!found.contains_key("Quietly") || found["Quietly"].0 == 0);
    }
}
