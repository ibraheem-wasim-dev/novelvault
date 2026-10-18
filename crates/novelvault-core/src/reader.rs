//! The Reader — a crawler that learns the novel as it reads it.
//!
//! The librarian in `ai` answers questions from a search index. The Reader is different:
//! it *reads* every draft in story order, one file at a time, and builds an understanding
//! of the plot that grows with each file — who the characters are, what is said about them,
//! what happens, which questions the story leaves open, and where one draft disagrees with
//! another. Understanding is built **only from the author's files**, never from outside
//! knowledge, and every item points back to the file and sentence it came from.
//!
//! How it learns, side by side with the novel:
//!
//! * **Incremental.** A file version is read once per content hash. A new draft, a new save
//!   from the watcher or a changed file is picked up and folded in; nothing is re-read.
//! * **It gets better as it reads.** Names it has seen often enough become *known*, so on a
//!   later file it can recognise a name at the start of a sentence that it could not
//!   recognise the first time.
//! * **Authority follows the file, not the fact.** Facts are stored against the file version
//!   they came from. When the author marks a draft CURRENT or archives it, the same facts
//!   instantly count as live or as history — nothing needs re-reading.
//! * **Heuristics by default, a local model to go deeper.** With no model, reading is
//!   rule-based and deterministic. With a local model (see `llm`) `deepen_with_model` asks it
//!   for a richer reading of each file, and every claim it makes is checked against the file
//!   text before it is kept.
//!
//! Like everything in `ai`, the Reader is read-only toward the author's work (R1/R2): it
//! writes only derived tables inside `.novelvault/`. What it notices goes to
//! `ai_suggestions` and waits for a person to decide.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::params;
use serde::Serialize;
use serde_json::Value;

use crate::db::{self, Vault};
use crate::llm;
use crate::model::*;
use crate::util::{new_id, now_iso, sha256_hex};
use crate::{ai, story, Result, VaultError};

const LIVE_SQL: &str = "('CURRENT','WORKING_DRAFT','REFERENCE')";
const STORY_SO_FAR_KEY: &str = "reader.story_so_far";
const STORY_SO_FAR_SIG: &str = "reader.story_so_far_sig";
const MAX_EVENTS_PER_FILE: usize = 12;
const MAX_THREADS_PER_FILE: usize = 8;

fn is_live_status(status: &str) -> bool {
    matches!(status, "CURRENT" | "WORKING_DRAFT" | "REFERENCE")
}

// ---------------------------------------------------------------------------
// Word lists (small, deliberate, and easy to read)
// ---------------------------------------------------------------------------

/// Capitalised words that are never names.
const NOT_NAMES: &[&str] = &[
    "the", "and", "but", "she", "her", "his", "him", "they", "them", "their", "there", "then", "this", "that",
    "these", "those", "what", "when", "where", "who", "why", "how", "with", "from", "into", "have", "had", "was",
    "were", "not", "now", "one", "two", "all", "any", "for", "you", "your", "our", "its", "yes", "no", "he", "it",
    "we", "i", "if", "as", "at", "in", "on", "of", "to", "by", "so", "or", "yet", "still", "after", "before",
    "while", "because", "although", "though", "once", "again", "maybe", "perhaps", "some", "every", "each", "no",
    "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "january", "february", "march",
    "april", "may", "june", "july", "august", "september", "october", "november", "december", "arc", "chapter",
    "draft", "scene", "part", "note", "notes", "idea", "ideas", "version", "revised", "old", "new", "current",
    "later", "soon", "suddenly", "finally", "meanwhile", "however", "quietly", "slowly", "nothing", "everything",
    "something", "someone", "everyone", "anyone", "nobody", "here", "just", "even", "only", "also", "too", "very",
    "mr", "mrs", "ms", "dr", "sir", "lady", "lord", "oh", "ah", "well", "okay", "please", "thank", "thanks",
];

const PLACE_PREPS: &[&str] = &[
    "in", "at", "to", "from", "into", "toward", "towards", "near", "inside", "outside", "across", "through",
    "reached", "entered", "left", "beyond", "behind", "above", "below", "over", "under", "around", "within",
];

/// Past or present verbs that very often follow a person's name.
const VERBS: &[&str] = &[
    "said", "asked", "looked", "smiled", "nodded", "turned", "walked", "ran", "went", "came", "saw", "knew",
    "thought", "felt", "took", "gave", "made", "found", "kept", "held", "stood", "sat", "fell", "left", "told",
    "heard", "began", "wanted", "loved", "hated", "feared", "met", "lost", "won", "wrote", "read", "spoke",
    "grew", "brought", "carried", "wore", "hid", "stole", "fought", "died", "lived", "slept", "woke", "cried",
    "laughed", "shouted", "whispered", "replied", "answered", "watched", "waited", "followed", "led", "drew",
    "let", "set", "put", "got", "had", "has", "is", "was", "are", "were", "became", "becomes", "does", "did",
    "will", "would", "could", "can", "must", "should", "might", "never", "always", "also", "then", "still",
    "shook", "pulled", "pushed", "opened", "closed", "burned", "broke", "swore", "promised", "refused",
    "trusted", "betrayed", "killed", "saved", "married", "owned", "owns", "keeps", "holds", "carries", "wears",
    "lives", "goes", "loves", "hates", "fears", "knows", "tells", "says", "asks", "looks", "walks", "runs",
    "returned", "arrived", "escaped", "vanished", "disappeared", "confessed", "revealed", "discovered",
];

const ACTION_CUES: &[&str] = &[
    "died", "killed", "found", "discovered", "arrived", "left", "escaped", "married", "betrayed", "promised",
    "revealed", "confessed", "vanished", "disappeared", "returned", "fought", "saved", "stole", "burned",
    "opened", "swore", "refused", "lost", "won", "fled", "attacked", "hid", "learned", "decided", "broke",
];

const THREAD_CUES: &[&str] = &[
    "secret", "never told", "promised", "would return", "vanished", "disappeared", "missing", "hidden",
    "unknown", "mystery", "refused to say", "owed", "swore", "someday", "one day", "still waiting",
    "unanswered", "unsolved", "wondered", "no one knew", "nobody knew", "had not told", "kept from",
];

// ---------------------------------------------------------------------------
// Pure reading — text in, structured observations out
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct NameStat {
    pub mid: usize,
    pub total: usize,
    pub person_cue: usize,
    pub place_cue: usize,
}

#[derive(Debug, Clone)]
pub struct RawFact {
    pub ordinal: usize,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub excerpt: String,
    pub inferred: bool,
}

#[derive(Debug, Clone)]
pub struct RawEvent {
    pub ordinal: usize,
    pub summary: String,
    pub entities: Vec<String>,
    pub salience: usize,
}

#[derive(Debug, Clone)]
pub struct RawThread {
    pub ordinal: usize,
    pub excerpt: String,
    pub cue: String,
    pub entities: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Reading {
    pub sentences: usize,
    pub names: HashMap<String, NameStat>,
    pub facts: Vec<RawFact>,
    pub events: Vec<RawEvent>,
    pub threads: Vec<RawThread>,
    pub relations: HashMap<(String, String), usize>,
}

/// Split text into sentences. Paragraph breaks always end a sentence.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n').map(str::trim).filter(|p| !p.is_empty()) {
        let chars: Vec<char> = para.chars().collect();
        let mut cur = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            cur.push(c);
            if matches!(c, '.' | '!' | '?' | '…') {
                // swallow closing quotes/brackets and repeated terminators
                while i + 1 < chars.len() && matches!(chars[i + 1], '.' | '!' | '?' | '"' | '”' | '’' | ')' | '\'') {
                    i += 1;
                    cur.push(chars[i]);
                }
                let at_end = i + 1 >= chars.len();
                let next_is_space = !at_end && chars[i + 1].is_whitespace();
                if at_end || next_is_space {
                    let s = cur.trim().to_string();
                    if !s.is_empty() {
                        out.push(s);
                    }
                    cur.clear();
                }
            }
            i += 1;
        }
        let s = cur.trim().to_string();
        if !s.is_empty() {
            out.push(s);
        }
    }
    out
}

#[derive(Debug, Clone)]
struct Word {
    clean: String,
    lower: String,
    cap: bool,
    possessive: bool,
    /// A comma, semicolon, colon or sentence end follows this word.
    break_after: bool,
}

fn words_of(sentence: &str) -> Vec<Word> {
    sentence
        .split_whitespace()
        .map(|raw| {
            let break_after = raw.ends_with([',', ';', ':', '.', '!', '?', '—', '…']);
            let mut clean: String = raw
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
                .to_string();
            let mut possessive = false;
            for suf in ["'s", "’s"] {
                if let Some(stem) = clean.strip_suffix(suf) {
                    clean = stem.to_string();
                    possessive = true;
                    break;
                }
            }
            let clean = clean.trim_matches(|c: char| c == '\'' || c == '’').to_string();
            let cap = clean.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
            let lower = clean.to_lowercase();
            Word { clean, lower, cap, possessive, break_after }
        })
        .filter(|w| !w.clean.is_empty())
        .collect()
}

fn is_verb(w: &str) -> bool {
    VERBS.contains(&w) || (w.len() >= 5 && w.ends_with("ed"))
}

fn plausible_name_word(w: &Word) -> bool {
    w.cap
        && w.clean.chars().count() >= 2
        && w.clean.chars().skip(1).any(|c| c.is_lowercase())
        && w.clean.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '’' || c == '-')
        && !NOT_NAMES.contains(&w.lower.as_str())
}

#[derive(Debug, Clone)]
struct Mention {
    phrase: String,
    start: usize,
    end: usize,
    place: bool,
    person: bool,
}

fn find_mentions(words: &[Word], known: &HashSet<String>) -> Vec<Mention> {
    let mut out: Vec<Mention> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = &words[i];
        let initial = i == 0;
        let accepted = plausible_name_word(w)
            && (!initial
                || known.contains(&w.lower)
                || w.possessive
                || words.get(i + 1).map(|n| is_verb(&n.lower)).unwrap_or(false));
        if !accepted {
            i += 1;
            continue;
        }
        // Merge a run of capitalised words ("Lucifer Morningstar") unless punctuation breaks it.
        let mut j = i;
        while !words[j].break_after
            && !words[j].possessive
            && j + 1 < words.len()
            && plausible_name_word(&words[j + 1])
        {
            j += 1;
        }
        let phrase = words[i..=j].iter().map(|w| w.clean.as_str()).collect::<Vec<_>>().join(" ");
        let prev = if i > 0 { Some(words[i - 1].lower.as_str()) } else { None };
        let next = words.get(j + 1).map(|n| n.lower.as_str());
        let after_verb = next.map(is_verb).unwrap_or(false) || words[j].possessive;
        let place = prev.map(|p| PLACE_PREPS.contains(&p)).unwrap_or(false) && !after_verb;
        let person = after_verb || matches!(prev, Some("said") | Some("asked") | Some("told") | Some("called"));
        out.push(Mention { phrase, start: i, end: j + 1, place, person });
        i = j + 1;
    }
    out
}

/// (predicate shown to the reader, class)
#[derive(Clone, Copy, PartialEq)]
enum Class {
    Copula,
    Possess,
    Move,
    Relate,
}

fn lemma(verb: &str) -> Option<(&'static str, Class)> {
    Some(match verb {
        "is" | "was" | "are" | "were" => ("is", Class::Copula),
        "became" | "becomes" => ("became", Class::Copula),
        "had" | "has" => ("has", Class::Possess),
        "kept" | "keeps" => ("keeps", Class::Possess),
        "carried" | "carries" => ("carries", Class::Possess),
        "held" | "holds" => ("holds", Class::Possess),
        "wore" | "wears" => ("wears", Class::Possess),
        "owned" | "owns" => ("owns", Class::Possess),
        "hid" | "hides" => ("hides", Class::Possess),
        "found" | "finds" => ("finds", Class::Possess),
        "lost" | "loses" => ("loses", Class::Possess),
        "lived" | "lives" => ("lives in", Class::Move),
        "went" | "goes" => ("goes to", Class::Move),
        "walked" | "walks" => ("walks to", Class::Move),
        "ran" | "runs" => ("runs to", Class::Move),
        "returned" | "returns" => ("returns to", Class::Move),
        "arrived" | "arrives" => ("arrives at", Class::Move),
        "travelled" | "traveled" | "travels" => ("travels to", Class::Move),
        "fled" | "flees" => ("flees to", Class::Move),
        "loved" | "loves" => ("loves", Class::Relate),
        "hated" | "hates" => ("hates", Class::Relate),
        "feared" | "fears" => ("fears", Class::Relate),
        "trusted" | "trusts" => ("trusts", Class::Relate),
        "killed" | "kills" => ("kills", Class::Relate),
        "saved" | "saves" => ("saves", Class::Relate),
        "married" | "marries" => ("marries", Class::Relate),
        "betrayed" | "betrays" => ("betrays", Class::Relate),
        "followed" | "follows" => ("follows", Class::Relate),
        "watched" | "watches" => ("watches", Class::Relate),
        "met" | "meets" => ("meets", Class::Relate),
        "knew" | "knows" => ("knows", Class::Relate),
        "told" | "tells" => ("tells", Class::Relate),
        _ => return None,
    })
}

const CLAUSE_STOP: &[&str] = &["but", "and", "because", "while", "when", "as", "until", "so", "though", "although"];
const PREPS_FOR_OBJECT: &[&str] = &["to", "in", "at", "into", "toward", "towards", "from", "near", "inside"];

fn take_object(words: &[Word], from: usize) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for (k, w) in words.iter().enumerate().skip(from) {
        if k > from && CLAUSE_STOP.contains(&w.lower.as_str()) {
            break;
        }
        parts.push(w.clean.as_str());
        if w.break_after || parts.len() >= 9 {
            break;
        }
    }
    parts.join(" ")
}

fn extract_fact(
    words: &[Word],
    mentions: &[Mention],
    last_person: &Option<String>,
    sentence: &str,
    ordinal: usize,
) -> Option<RawFact> {
    // Subject: a name opening the sentence, or he/she/they resolved to the last person named.
    let (subject, mut idx, inferred) = if let Some(m) = mentions.first().filter(|m| m.start == 0) {
        (m.phrase.clone(), m.end, false)
    } else if matches!(words.first().map(|w| w.lower.as_str()), Some("he") | Some("she") | Some("they")) {
        (last_person.clone()?, 1, true)
    } else {
        return None;
    };
    // skip "had been" and an -ly adverb
    if matches!(words.get(idx).map(|w| w.lower.as_str()), Some("had") | Some("has") | Some("have"))
        && words.get(idx + 1).map(|w| w.lower.as_str()) == Some("been")
    {
        idx += 1;
    }
    if words.get(idx).map(|w| w.lower.ends_with("ly") && w.lower.len() > 4).unwrap_or(false) {
        idx += 1;
    }
    let verb = words.get(idx)?;
    if verb.lower.contains("n't") || verb.lower.contains("n’t") {
        return None; // negations are not facts about what is
    }
    let was_been = verb.lower == "been";
    let (pred, class) = if was_been { ("is", Class::Copula) } else { lemma(&verb.lower)? };
    if matches!(words.get(idx + 1).map(|w| w.lower.as_str()), Some("not") | Some("never")) {
        return None;
    }
    let object = match class {
        Class::Move => {
            let prep = words.get(idx + 1)?;
            if !PREPS_FOR_OBJECT.contains(&prep.lower.as_str()) {
                return None;
            }
            take_object(words, idx + 2)
        }
        _ => take_object(words, idx + 1),
    };
    if object.trim().is_empty() {
        return None;
    }
    // "Azmirah kept" with nothing to say about it is not a fact
    if class == Class::Copula && object.split_whitespace().count() < 1 {
        return None;
    }
    Some(RawFact {
        ordinal,
        subject,
        predicate: pred.to_string(),
        object,
        excerpt: clip(sentence, 240),
        inferred,
    })
}

fn clip(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        flat.chars().take(max).collect::<String>() + "…"
    }
}

/// Read a whole text. `known` holds lowercase names the Reader already recognises.
pub fn read_text(text: &str, known: &HashSet<String>) -> Reading {
    let mut reading = Reading::default();
    let mut last_person: Option<String> = None;
    let mut events: Vec<RawEvent> = Vec::new();
    let mut threads: Vec<RawThread> = Vec::new();

    let sentences = split_sentences(text);
    reading.sentences = sentences.len();

    for (ordinal, sentence) in sentences.iter().enumerate() {
        let words = words_of(sentence);
        if words.is_empty() {
            continue;
        }
        let mentions = find_mentions(&words, known);

        for (n, m) in mentions.iter().enumerate() {
            let stat = reading.names.entry(m.phrase.clone()).or_default();
            stat.total += 1;
            if m.start > 0 {
                stat.mid += 1;
            }
            if m.person {
                stat.person_cue += 1;
            }
            if m.place {
                stat.place_cue += 1;
            }
            // pair up with later names in the same sentence
            for other in mentions.iter().skip(n + 1).take(4) {
                if other.phrase != m.phrase {
                    let key = if m.phrase < other.phrase {
                        (m.phrase.clone(), other.phrase.clone())
                    } else {
                        (other.phrase.clone(), m.phrase.clone())
                    };
                    *reading.relations.entry(key).or_insert(0) += 1;
                }
            }
        }

        if let Some(fact) = extract_fact(&words, &mentions, &last_person, sentence, ordinal) {
            reading.facts.push(fact);
        }

        // Events: a named party doing something.
        let names_here: Vec<String> = {
            let mut seen = HashSet::new();
            mentions.iter().filter(|m| seen.insert(m.phrase.clone())).map(|m| m.phrase.clone()).collect()
        };
        let has_action = words.iter().any(|w| is_verb(&w.lower) && !matches!(w.lower.as_str(), "was" | "were" | "is" | "are" | "had" | "has" | "the"));
        if !names_here.is_empty() && has_action {
            let lower = sentence.to_lowercase();
            let cue_hits = ACTION_CUES.iter().filter(|c| lower.split(|ch: char| !ch.is_alphabetic()).any(|t| t == **c)).count();
            let salience = names_here.len() * 2 + cue_hits * 3 + mentions.iter().filter(|m| m.place).count();
            events.push(RawEvent { ordinal, summary: clip(sentence, 220), entities: names_here.clone(), salience });
        }

        // Possible open threads: the narration points at something unresolved.
        let lower = sentence.to_lowercase();
        let in_dialogue = sentence.trim_start().starts_with(['"', '“']);
        let cue = THREAD_CUES
            .iter()
            .find(|c| lower.contains(**c))
            .map(|c| c.to_string())
            .or_else(|| if sentence.trim_end().ends_with('?') && !in_dialogue { Some("question".to_string()) } else { None });
        if let Some(cue) = cue {
            threads.push(RawThread { ordinal, excerpt: clip(sentence, 240), cue, entities: names_here.clone() });
        }

        // Who "she/he" will mean in the next sentence.
        if let Some(m) = mentions.iter().find(|m| m.start == 0 && !m.place).or_else(|| mentions.iter().find(|m| m.person)) {
            last_person = Some(m.phrase.clone());
        }
    }

    // Keep the most important events, but in the order they happen.
    if events.len() > MAX_EVENTS_PER_FILE {
        let mut ranked: Vec<usize> = (0..events.len()).collect();
        ranked.sort_by(|a, b| events[*b].salience.cmp(&events[*a].salience).then(a.cmp(b)));
        let keep: HashSet<usize> = ranked.into_iter().take(MAX_EVENTS_PER_FILE).collect();
        events = events.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, e)| e).collect();
    }
    threads.truncate(MAX_THREADS_PER_FILE);
    reading.events = events;
    reading.threads = threads;
    reading
}

// ---------------------------------------------------------------------------
// Reading order
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Nat {
    Num(u64),
    Text(String),
}

fn natural_key(s: &str) -> Vec<Nat> {
    let mut out = Vec::new();
    let mut digits = String::new();
    let mut text = String::new();
    let flush_text = |text: &mut String, out: &mut Vec<Nat>| {
        if !text.is_empty() {
            out.push(Nat::Text(std::mem::take(text)));
        }
    };
    for c in s.to_lowercase().chars() {
        if c.is_ascii_digit() {
            flush_text(&mut text, &mut out);
            digits.push(c);
        } else {
            if !digits.is_empty() {
                out.push(Nat::Num(digits.parse().unwrap_or(0)));
                digits.clear();
            }
            text.push(c);
        }
    }
    if !digits.is_empty() {
        out.push(Nat::Num(digits.parse().unwrap_or(0)));
    }
    flush_text(&mut text, &mut out);
    out
}

/// Story order: arc, then chapter, then file name, then version.
fn reading_order(a: &FileVersion, b: &FileVersion) -> Ordering {
    let key = |v: &FileVersion| {
        (
            natural_key(v.arc_label.as_deref().unwrap_or("~")),
            natural_key(v.chapter_label.as_deref().unwrap_or("~")),
            natural_key(&v.rel_path),
            v.version_number,
        )
    };
    key(a).cmp(&key(b))
}

// ---------------------------------------------------------------------------
// The crawl
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ReadStep {
    pub version_id: String,
    pub rel_path: String,
    pub sentences: usize,
    pub facts: usize,
    pub events: usize,
    pub threads: usize,
    /// Names that became known to the Reader because of this file.
    pub learned: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CrawlReport {
    pub files_read: usize,
    pub steps: Vec<ReadStep>,
    pub remaining: usize,
    pub new_notices: usize,
}

struct PendingRead {
    version: FileVersion,
}

fn pending_reads(vault: &Vault) -> Result<Vec<PendingRead>> {
    let mut stmt = vault.conn.prepare(
        "SELECT v.id FROM file_versions v
         LEFT JOIN reader_files r ON r.version_id = v.id AND r.sha256 = v.sha256
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL AND v.extracted_chars > 0 AND r.version_id IS NULL",
    )?;
    let ids = stmt
        .query_map(params![vault.novel_id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut out: Vec<FileVersion> = Vec::new();
    for id in ids {
        if let Ok(v) = crate::store::get_version(vault, &id) {
            out.push(v);
        }
    }
    out.sort_by(reading_order);
    Ok(out.into_iter().map(|version| PendingRead { version }).collect())
}

fn registry(vault: &Vault) -> Result<Vec<(String, &'static str)>> {
    let mut out: Vec<(String, &'static str)> = Vec::new();
    for c in story::list_characters(vault)?.into_iter().filter(|c| !c.is_reader_sentinel) {
        out.push((c.name, "character"));
    }
    for l in story::list_locations(vault)? {
        out.push((l.name, "location"));
    }
    for o in story::list_objects(vault)? {
        out.push((o.name, "object"));
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct EntityAgg {
    name: String,
    kind: String,
    aliases: Vec<String>,
    mid: i64,
    total: i64,
    live_total: i64,
    files: i64,
    in_registry: bool,
}

/// A capitalised word becomes an entity once it keeps turning up mid-sentence, or keeps
/// being the one who acts ("Azmirah kept…", "Azmirah smiled…") even if it always opens a sentence.
fn qualifies(mid: i64, total: i64, person_cue: i64, name: &str) -> bool {
    total >= 3 && (mid >= 2 || person_cue >= 3) && !NOT_NAMES.contains(&name.to_lowercase().as_str())
}

fn aggregate_entities(vault: &Vault) -> Result<Vec<EntityAgg>> {
    let reg = registry(vault)?;
    let reg_kind: HashMap<String, &'static str> = reg.iter().map(|(n, k)| (n.to_lowercase(), *k)).collect();

    let mut stmt = vault.conn.prepare(&format!(
        "SELECT n.name, SUM(n.mid), SUM(n.total), SUM(n.person_cue), SUM(n.place_cue),
                SUM(CASE WHEN v.file_status IN {LIVE_SQL} THEN n.total ELSE 0 END),
                COUNT(DISTINCT n.version_id)
         FROM reader_names n JOIN file_versions v ON v.id = n.version_id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL GROUP BY n.name"
    ))?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut map: BTreeMap<String, EntityAgg> = BTreeMap::new();
    for (name, mid, total, person, place, live, files) in rows {
        let in_reg = reg_kind.contains_key(&name.to_lowercase());
        if !in_reg && !qualifies(mid, total, person, &name) {
            continue;
        }
        let kind = reg_kind
            .get(&name.to_lowercase())
            .map(|k| k.to_string())
            .unwrap_or_else(|| match person.cmp(&place) {
                Ordering::Greater => "character".into(),
                Ordering::Less => "location".into(),
                Ordering::Equal => "unknown".into(),
            });
        map.insert(
            name.clone(),
            EntityAgg { name, kind, aliases: Vec::new(), mid, total, live_total: live, files, in_registry: in_reg },
        );
    }
    // Registry entries the text never capitalised still belong to the cast.
    for (name, kind) in &reg {
        if !map.keys().any(|k| k.eq_ignore_ascii_case(name)) {
            map.insert(
                name.clone(),
                EntityAgg { name: name.clone(), kind: kind.to_string(), aliases: vec![], mid: 0, total: 0, live_total: 0, files: 0, in_registry: true },
            );
        }
    }

    // "Azmirah" and "Azmirah Khan" are one person: fold a single word into the full name.
    let names: Vec<String> = map.keys().cloned().collect();
    for short in names.iter().filter(|n| !n.contains(' ')) {
        let owners: Vec<&String> = names
            .iter()
            .filter(|n| n.contains(' ') && n.split_whitespace().any(|p| p.eq_ignore_ascii_case(short)))
            .collect();
        if owners.len() == 1 {
            if let Some(short_agg) = map.remove(short) {
                if let Some(owner) = map.get_mut(owners[0]) {
                    owner.mid += short_agg.mid;
                    owner.total += short_agg.total;
                    owner.live_total += short_agg.live_total;
                    owner.files = owner.files.max(short_agg.files);
                    owner.in_registry |= short_agg.in_registry;
                    owner.aliases.push(short_agg.name);
                }
            }
        }
    }
    let mut out: Vec<EntityAgg> = map.into_values().collect();
    out.sort_by(|a, b| b.live_total.cmp(&a.live_total).then(b.total.cmp(&a.total)).then(a.name.cmp(&b.name)));
    Ok(out)
}

fn known_names(vault: &Vault) -> Result<HashSet<String>> {
    let mut known = HashSet::new();
    for (name, _) in registry(vault)? {
        known.insert(name.to_lowercase());
        for part in name.split_whitespace() {
            known.insert(part.to_lowercase());
        }
    }
    for e in aggregate_entities(vault)? {
        known.insert(e.name.to_lowercase());
        for part in e.name.split_whitespace() {
            known.insert(part.to_lowercase());
        }
        for a in e.aliases {
            known.insert(a.to_lowercase());
        }
    }
    Ok(known)
}

fn store_reading(vault: &Vault, version_id: &str, sha: &str, reading: &Reading, summary: &str) -> Result<()> {
    let tx = vault.conn.unchecked_transaction()?;
    for table in ["reader_names", "reader_facts", "reader_events", "reader_threads", "reader_relations"] {
        tx.execute(&format!("DELETE FROM {table} WHERE version_id = ?1"), params![version_id])?;
    }
    for (name, s) in &reading.names {
        tx.execute(
            "INSERT INTO reader_names (version_id, name, mid, total, person_cue, place_cue) VALUES (?1,?2,?3,?4,?5,?6)",
            params![version_id, name, s.mid as i64, s.total as i64, s.person_cue as i64, s.place_cue as i64],
        )?;
    }
    for f in &reading.facts {
        tx.execute(
            "INSERT INTO reader_facts (id, version_id, ordinal, subject, predicate, object, excerpt, source, inferred)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'heuristic',?8)",
            params![new_id(), version_id, f.ordinal as i64, f.subject, f.predicate, f.object, f.excerpt, f.inferred as i64],
        )?;
    }
    for (seq, e) in reading.events.iter().enumerate() {
        tx.execute(
            "INSERT INTO reader_events (id, version_id, ordinal, seq, summary, entities_json, salience, source)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'heuristic')",
            params![new_id(), version_id, e.ordinal as i64, seq as i64, e.summary, serde_json::to_string(&e.entities)?, e.salience as i64],
        )?;
    }
    for t in &reading.threads {
        tx.execute(
            "INSERT INTO reader_threads (id, version_id, ordinal, excerpt, cue, entities_json) VALUES (?1,?2,?3,?4,?5,?6)",
            params![new_id(), version_id, t.ordinal as i64, t.excerpt, t.cue, serde_json::to_string(&t.entities)?],
        )?;
    }
    for ((a, b), n) in &reading.relations {
        tx.execute(
            "INSERT INTO reader_relations (version_id, a, b, n) VALUES (?1,?2,?3,?4)",
            params![version_id, a, b, *n as i64],
        )?;
    }
    tx.execute(
        "INSERT INTO reader_files (version_id, sha256, sentences, method, summary, read_at)
         VALUES (?1,?2,?3,'heuristic',?4,?5)
         ON CONFLICT(version_id) DO UPDATE SET sha256 = excluded.sha256, sentences = excluded.sentences,
             method = 'heuristic', summary = excluded.summary, read_at = excluded.read_at",
        params![version_id, sha, reading.sentences as i64, summary, now_iso()],
    )?;
    tx.commit()?;
    Ok(())
}

fn heuristic_summary(reading: &Reading) -> String {
    let parts: Vec<String> = reading.events.iter().take(2).map(|e| e.summary.clone()).collect();
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" ")
    }
}

/// Read up to `max_files` files nobody has read yet, in story order, folding each into the
/// Reader's understanding. Cheap enough to call after every scan, save or question.
pub fn crawl(vault: &Vault, max_files: usize) -> Result<CrawlReport> {
    let pending = pending_reads(vault)?;
    let total_pending = pending.len();
    let mut report = CrawlReport::default();
    if total_pending == 0 {
        return Ok(report);
    }
    let mut known = known_names(vault)?;
    for p in pending.into_iter().take(max_files.max(1)) {
        let path = db::index_versions_dir(vault.root.as_path()).join(format!("{}.txt", p.version.id));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let reading = read_text(&text, &known);
        store_reading(vault, &p.version.id, &p.version.sha256, &reading, &heuristic_summary(&reading))?;

        // What did this file teach the Reader that it did not already know?
        let after = known_names(vault)?;
        let mut learned: Vec<String> = after.difference(&known).filter(|n| !n.contains(' ')).cloned().collect();
        learned.sort();
        known = after;

        report.steps.push(ReadStep {
            version_id: p.version.id.clone(),
            rel_path: p.version.rel_path.clone(),
            sentences: reading.sentences,
            facts: reading.facts.len(),
            events: reading.events.len(),
            threads: reading.threads.len(),
            learned: learned.into_iter().map(|n| title_case(&n)).collect(),
        });
        report.files_read += 1;
    }
    report.remaining = total_pending.saturating_sub(report.files_read);
    if report.files_read > 0 {
        report.new_notices = sync_change_notices(vault)?;
        vault.audit("ReaderCrawl", "Novel", None, None, Some(&format!("{} files read", report.files_read)))?;
    }
    Ok(report)
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// What the Reader knows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Coverage {
    pub files_total: i64,
    pub files_read: i64,
    pub files_pending: i64,
    pub files_deepened: i64,
    pub sentences_read: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityCard {
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub mentions_live: i64,
    pub mentions_older: i64,
    pub files: i64,
    pub facts: i64,
    /// True when the author has recorded this as a character/location/object.
    pub recorded: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineEvent {
    pub version_id: String,
    pub rel_path: String,
    pub label: String,
    pub summary: String,
    pub entities: Vec<String>,
    pub is_live: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadCard {
    pub rel_path: String,
    pub excerpt: String,
    pub cue: String,
    pub entities: Vec<String>,
    pub is_live: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChapterBrief {
    pub version_id: String,
    pub rel_path: String,
    pub label: String,
    pub summary: String,
    pub method: String,
    pub is_live: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoryChange {
    pub subject: String,
    pub predicate: String,
    pub old_value: String,
    pub old_file: String,
    pub new_value: String,
    pub new_file: String,
    /// "revision" (an older draft says something different) or "contradiction" (two live files differ).
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Understanding {
    pub coverage: Coverage,
    pub entities: Vec<EntityCard>,
    pub chapters: Vec<ChapterBrief>,
    pub timeline: Vec<TimelineEvent>,
    pub threads: Vec<ThreadCard>,
    pub changes: Vec<StoryChange>,
    pub story_so_far: String,
    pub story_so_far_method: String,
}

fn label_of(v: &FileVersion) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(a) = &v.arc_label {
        parts.push(a.clone());
    }
    if let Some(c) = &v.chapter_label {
        parts.push(c.clone());
    }
    if parts.is_empty() {
        v.family_name.clone()
    } else {
        parts.join(" · ")
    }
}

/// All read file versions, in story order, with their status.
fn read_versions(vault: &Vault) -> Result<Vec<(FileVersion, String, Option<String>)>> {
    let mut stmt = vault.conn.prepare(
        "SELECT r.version_id, r.method, r.summary FROM reader_files r JOIN file_versions v ON v.id = r.version_id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL AND r.sha256 = v.sha256",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for (id, method, summary) in rows {
        if let Ok(v) = crate::store::get_version(vault, &id) {
            out.push((v, method, summary));
        }
    }
    out.sort_by(|a, b| reading_order(&a.0, &b.0));
    Ok(out)
}

pub fn coverage(vault: &Vault) -> Result<Coverage> {
    let files_total: i64 = vault.conn.query_row(
        "SELECT COUNT(*) FROM file_versions WHERE novel_id = ?1 AND deleted_at IS NULL AND extracted_chars > 0",
        params![vault.novel_id],
        |r| r.get(0),
    )?;
    let (files_read, files_deepened, sentences_read): (i64, i64, i64) = vault.conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN r.method = 'model' THEN 1 ELSE 0 END),0), COALESCE(SUM(r.sentences),0)
         FROM reader_files r JOIN file_versions v ON v.id = r.version_id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL AND r.sha256 = v.sha256",
        params![vault.novel_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(Coverage {
        files_total,
        files_read,
        files_pending: (files_total - files_read).max(0),
        files_deepened,
        sentences_read,
    })
}

fn fact_counts(vault: &Vault) -> Result<HashMap<String, i64>> {
    let mut stmt = vault.conn.prepare(
        "SELECT lower(f.subject), COUNT(*) FROM reader_facts f JOIN file_versions v ON v.id = f.version_id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL GROUP BY lower(f.subject)",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().collect())
}

pub fn understanding(vault: &Vault) -> Result<Understanding> {
    let versions = read_versions(vault)?;
    let coverage = coverage(vault)?;
    let fc = fact_counts(vault)?;

    let entities: Vec<EntityCard> = aggregate_entities(vault)?
        .into_iter()
        .take(80)
        .map(|e| {
            let mut facts = fc.get(&e.name.to_lowercase()).copied().unwrap_or(0);
            for a in &e.aliases {
                facts += fc.get(&a.to_lowercase()).copied().unwrap_or(0);
            }
            EntityCard {
                mentions_older: e.total - e.live_total,
                mentions_live: e.live_total,
                name: e.name,
                kind: e.kind,
                aliases: e.aliases,
                files: e.files,
                facts,
                recorded: e.in_registry,
            }
        })
        .collect();

    let chapters: Vec<ChapterBrief> = versions
        .iter()
        .map(|(v, method, summary)| ChapterBrief {
            version_id: v.id.clone(),
            rel_path: v.rel_path.clone(),
            label: label_of(v),
            summary: summary.clone().unwrap_or_default(),
            method: method.clone(),
            is_live: is_live_status(&v.file_status),
        })
        .collect();

    let mut timeline = Vec::new();
    let mut threads = Vec::new();
    for (v, _, _) in &versions {
        let live = is_live_status(&v.file_status);
        let mut stmt = vault.conn.prepare(
            "SELECT summary, entities_json, source FROM reader_events WHERE version_id = ?1 ORDER BY seq",
        )?;
        let evs = stmt
            .query_map(params![v.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (summary, ents, source) in evs {
            timeline.push(TimelineEvent {
                version_id: v.id.clone(),
                rel_path: v.rel_path.clone(),
                label: label_of(v),
                summary,
                entities: ents.and_then(|e| serde_json::from_str(&e).ok()).unwrap_or_default(),
                is_live: live,
                source,
            });
        }
        let mut stmt = vault.conn.prepare("SELECT excerpt, cue, entities_json FROM reader_threads WHERE version_id = ?1 ORDER BY ordinal")?;
        let ths = stmt
            .query_map(params![v.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (excerpt, cue, ents) in ths {
            threads.push(ThreadCard {
                rel_path: v.rel_path.clone(),
                excerpt,
                cue,
                entities: ents.and_then(|e| serde_json::from_str(&e).ok()).unwrap_or_default(),
                is_live: live,
            });
        }
    }
    // Live material first within each list; story order is kept inside each group.
    timeline.sort_by_key(|e| !e.is_live);
    threads.sort_by_key(|t| !t.is_live);
    timeline.truncate(120);
    threads.truncate(60);

    let (story_so_far, story_so_far_method) = story_so_far(vault, &versions);
    Ok(Understanding {
        coverage,
        entities,
        chapters,
        timeline,
        threads,
        changes: story_changes(vault)?,
        story_so_far,
        story_so_far_method,
    })
}

fn live_signature(versions: &[(FileVersion, String, Option<String>)]) -> String {
    let joined: String = versions
        .iter()
        .filter(|(v, _, _)| is_live_status(&v.file_status))
        .map(|(v, _, _)| v.sha256.clone())
        .collect::<Vec<_>>()
        .join("|");
    sha256_hex(joined.as_bytes())
}

fn story_so_far(vault: &Vault, versions: &[(FileVersion, String, Option<String>)]) -> (String, String) {
    if let (Some(text), Some(sig)) = (vault.setting_get(STORY_SO_FAR_KEY), vault.setting_get(STORY_SO_FAR_SIG)) {
        if sig == live_signature(versions) && !text.trim().is_empty() {
            return (text, "model".into());
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for (v, _, summary) in versions.iter().filter(|(v, _, _)| is_live_status(&v.file_status)) {
        if let Some(s) = summary.as_ref().filter(|s| !s.trim().is_empty()) {
            lines.push(format!("{} — {}", label_of(v), s.trim()));
        }
    }
    (lines.join("\n"), "reading".into())
}

// ---------------------------------------------------------------------------
// Changes the Reader noticed between drafts
// ---------------------------------------------------------------------------

const SINGLE_VALUED: &[&str] = &["is", "keeps", "carries", "holds", "wears", "owns", "lives in", "has"];

fn strip_articles(s: &str) -> String {
    let mut words: Vec<&str> = s.split_whitespace().collect();
    while words
        .first()
        .map(|w| matches!(w.to_lowercase().as_str(), "the" | "a" | "an" | "her" | "his" | "their" | "its" | "my" | "your" | "our"))
        .unwrap_or(false)
    {
        words.remove(0);
    }
    words.join(" ").to_lowercase()
}

/// The core of an object phrase: what comes before the first preposition.
fn object_head(s: &str) -> String {
    const CUT: &[&str] = &["on", "in", "at", "by", "near", "under", "beside", "with", "from", "to", "for", "of", "over", "behind"];
    let stripped = strip_articles(s);
    let mut head: Vec<&str> = Vec::new();
    for w in stripped.split_whitespace() {
        if !head.is_empty() && CUT.contains(&w) {
            break;
        }
        head.push(w);
    }
    head.join(" ").trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

fn differs(a: &str, b: &str) -> bool {
    let (ha, hb) = (object_head(a), object_head(b));
    !ha.is_empty() && !hb.is_empty() && ha != hb && !ha.contains(&hb) && !hb.contains(&ha)
}

struct FactRow {
    subject: String,
    predicate: String,
    object: String,
    rel_path: String,
    file_id: String,
    live: bool,
    version_number: i64,
}

fn fact_rows(vault: &Vault) -> Result<Vec<FactRow>> {
    let mut stmt = vault.conn.prepare(
        "SELECT f.subject, f.predicate, f.object, v.rel_path, v.file_id, v.file_status, v.version_number
         FROM reader_facts f JOIN file_versions v ON v.id = f.version_id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL AND f.inferred = 0
         ORDER BY v.rel_path, f.ordinal",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| {
            Ok(FactRow {
                subject: r.get(0)?,
                predicate: r.get(1)?,
                object: r.get(2)?,
                rel_path: r.get(3)?,
                file_id: r.get(4)?,
                live: is_live_status(&r.get::<_, String>(5)?),
                version_number: r.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Where one draft says something different from another about the same subject.
/// This is computed from the facts as they stand now, so it follows the author's
/// CURRENT / archive decisions without re-reading anything.
pub fn story_changes(vault: &Vault) -> Result<Vec<StoryChange>> {
    let rows = fact_rows(vault)?;
    let mut by_key: HashMap<(String, String), Vec<&FactRow>> = HashMap::new();
    for r in &rows {
        if SINGLE_VALUED.contains(&r.predicate.as_str()) {
            by_key.entry((r.subject.to_lowercase(), r.predicate.clone())).or_default().push(r);
        }
    }
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut keys: Vec<_> = by_key.keys().cloned().collect();
    keys.sort();
    for key in keys {
        let group = &by_key[&key];
        for old in group.iter().filter(|r| !r.live) {
            for new in group.iter().filter(|r| r.live) {
                // "is" is too loose across different documents; only trust it within one family.
                if key.1 == "is" && old.file_id != new.file_id {
                    continue;
                }
                if differs(&old.object, &new.object) {
                    let id = format!("rev|{}|{}|{}|{}", key.0, key.1, object_head(&old.object), object_head(&new.object));
                    if seen.insert(id) {
                        out.push(StoryChange {
                            subject: new.subject.clone(),
                            predicate: key.1.clone(),
                            old_value: old.object.clone(),
                            old_file: format!("{} (v{})", old.rel_path, old.version_number),
                            new_value: new.object.clone(),
                            new_file: format!("{} (v{})", new.rel_path, new.version_number),
                            kind: "revision".into(),
                        });
                    }
                }
            }
        }
        if key.1 != "is" {
            let live: Vec<&&FactRow> = group.iter().filter(|r| r.live).collect();
            for (i, a) in live.iter().enumerate() {
                for b in live.iter().skip(i + 1) {
                    if a.file_id != b.file_id && differs(&a.object, &b.object) {
                        let (x, y) = if object_head(&a.object) < object_head(&b.object) { (a, b) } else { (b, a) };
                        let id = format!("con|{}|{}|{}|{}", key.0, key.1, object_head(&x.object), object_head(&y.object));
                        if seen.insert(id) {
                            out.push(StoryChange {
                                subject: x.subject.clone(),
                                predicate: key.1.clone(),
                                old_value: x.object.clone(),
                                old_file: x.rel_path.clone(),
                                new_value: y.object.clone(),
                                new_file: y.rel_path.clone(),
                                kind: "contradiction".into(),
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Turn the changes into notices that wait for the author. Never decides anything.
pub fn sync_change_notices(vault: &Vault) -> Result<usize> {
    let mut created = 0;
    for c in story_changes(vault)? {
        let title = format!("{} {}: “{}” vs “{}”", c.subject, c.predicate, clip(&object_head(&c.old_value), 40), clip(&object_head(&c.new_value), 40));
        let detail = if c.kind == "revision" {
            format!(
                "An older draft ({}) says {} {} “{}”, but a live file ({}) says “{}”. If this is a deliberate revision, you may want to update canon; if not, one of them is a mistake.",
                c.old_file, c.subject, c.predicate, c.old_value, c.new_file, c.new_value
            )
        } else {
            format!(
                "Two live files disagree: {} says {} {} “{}”, while {} says “{}”. This may be a continuity slip.",
                c.old_file, c.subject, c.predicate, c.old_value, c.new_file, c.new_value
            )
        };
        let n = vault.conn.execute(
            "INSERT OR IGNORE INTO ai_suggestions (id, novel_id, kind, title, detail, evidence_json, status, created_at)
             VALUES (?1, ?2, 'story_change', ?3, ?4, ?5, 'pending', ?6)",
            params![new_id(), vault.novel_id, title, detail, serde_json::to_string(&[c.old_file.clone(), c.new_file.clone()])?, now_iso()],
        )?;
        created += n;
    }
    if created > 0 {
        vault.audit("ReaderNotice", "Novel", None, None, Some(&format!("{created} story changes")))?;
    }
    Ok(created)
}

// ---------------------------------------------------------------------------
// One entity, in depth
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FactView {
    pub predicate: String,
    pub object: String,
    pub excerpt: String,
    pub rel_path: String,
    pub file_status: String,
    pub is_live: bool,
    pub inferred: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityProfile {
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub facts: Vec<FactView>,
    pub events: Vec<TimelineEvent>,
    pub related: Vec<(String, i64)>,
    pub files: Vec<String>,
    pub summary: String,
}

fn find_entity(vault: &Vault, name: &str) -> Result<Option<EntityAgg>> {
    let want = name.trim().to_lowercase();
    if want.is_empty() {
        return Ok(None);
    }
    Ok(aggregate_entities(vault)?.into_iter().find(|e| {
        e.name.to_lowercase() == want || e.aliases.iter().any(|a| a.to_lowercase() == want)
    }))
}

pub fn entity_profile(vault: &Vault, name: &str) -> Result<EntityProfile> {
    let ent = find_entity(vault, name)?.ok_or_else(|| VaultError::NotFound(format!("the Reader has not met “{}” yet", name.trim())))?;
    let mut names: Vec<String> = vec![ent.name.to_lowercase()];
    names.extend(ent.aliases.iter().map(|a| a.to_lowercase()));

    let mut stmt = vault.conn.prepare(
        "SELECT f.subject, f.predicate, f.object, f.excerpt, v.rel_path, v.file_status, f.inferred, f.source, f.ordinal
         FROM reader_facts f JOIN file_versions v ON v.id = f.version_id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL ORDER BY v.rel_path, f.ordinal",
    )?;
    let rows = stmt
        .query_map(params![vault.novel_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut facts: Vec<FactView> = Vec::new();
    for (subject, predicate, object, excerpt, rel_path, status, inferred, source) in rows {
        if names.contains(&subject.to_lowercase()) {
            facts.push(FactView {
                predicate,
                object,
                excerpt,
                rel_path,
                is_live: is_live_status(&status),
                file_status: status,
                inferred: inferred == 1,
                source,
            });
        }
    }
    facts.sort_by_key(|f| (!f.is_live, f.inferred));

    let mut related_map: HashMap<String, i64> = HashMap::new();
    let mut stmt = vault.conn.prepare(
        "SELECT a, b, SUM(n) FROM reader_relations r JOIN file_versions v ON v.id = r.version_id
         WHERE v.novel_id = ?1 AND v.deleted_at IS NULL GROUP BY a, b",
    )?;
    let rels = stmt
        .query_map(params![vault.novel_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (a, b, n) in rels {
        if names.contains(&a.to_lowercase()) {
            *related_map.entry(b).or_insert(0) += n;
        } else if names.contains(&b.to_lowercase()) {
            *related_map.entry(a).or_insert(0) += n;
        }
    }
    let mut related: Vec<(String, i64)> = related_map.into_iter().filter(|(n, _)| !names.contains(&n.to_lowercase())).collect();
    related.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    related.truncate(8);

    let u_events: Vec<TimelineEvent> = understanding_events(vault)?
        .into_iter()
        .filter(|e| e.entities.iter().any(|n| names.contains(&n.to_lowercase())))
        .take(10)
        .collect();

    let mut files: Vec<String> = Vec::new();
    for f in &facts {
        if !files.contains(&f.rel_path) {
            files.push(f.rel_path.clone());
        }
    }
    for e in &u_events {
        if !files.contains(&e.rel_path) {
            files.push(e.rel_path.clone());
        }
    }

    let summary = profile_summary(&ent, &facts);
    Ok(EntityProfile { name: ent.name, kind: ent.kind, aliases: ent.aliases, facts, events: u_events, related, files, summary })
}

fn understanding_events(vault: &Vault) -> Result<Vec<TimelineEvent>> {
    let mut out = Vec::new();
    for (v, _, _) in read_versions(vault)? {
        let mut stmt = vault.conn.prepare("SELECT summary, entities_json, source FROM reader_events WHERE version_id = ?1 ORDER BY seq")?;
        let rows = stmt
            .query_map(params![v.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (summary, ents, source) in rows {
            out.push(TimelineEvent {
                version_id: v.id.clone(),
                rel_path: v.rel_path.clone(),
                label: label_of(&v),
                summary,
                entities: ents.and_then(|e| serde_json::from_str(&e).ok()).unwrap_or_default(),
                is_live: is_live_status(&v.file_status),
                source,
            });
        }
    }
    out.sort_by_key(|e| !e.is_live);
    Ok(out)
}

fn profile_summary(ent: &EntityAgg, facts: &[FactView]) -> String {
    let live: Vec<String> = facts
        .iter()
        .filter(|f| f.is_live && !f.inferred)
        .take(4)
        .map(|f| format!("{} {}", f.predicate, clip(&f.object, 60)))
        .collect();
    let older: Vec<String> = facts
        .iter()
        .filter(|f| !f.is_live)
        .take(2)
        .map(|f| format!("{} {}", f.predicate, clip(&f.object, 60)))
        .collect();
    let mut s = format!("{} ({}).", ent.name, ent.kind);
    if !live.is_empty() {
        s.push_str(&format!(" In current files: {}.", live.join("; ")));
    }
    if !older.is_empty() {
        s.push_str(&format!(" Older drafts said: {}.", older.join("; ")));
    }
    s
}

// ---------------------------------------------------------------------------
// Handing what the Reader knows to the librarian
// ---------------------------------------------------------------------------

/// Sources for `ai::ask` drawn from the Reader's understanding. They are marked as derived:
/// the author's canon and files always outrank them.
pub fn understanding_sources(vault: &Vault, question: &str, max: usize) -> Result<Vec<ai::Source>> {
    let q = question.to_lowercase();
    let mut out: Vec<ai::Source> = Vec::new();

    let plot_question = ["plot", "summar", "what happens", "so far", "overview", "recap"].iter().any(|k| q.contains(k));
    if plot_question {
        let versions = read_versions(vault)?;
        let (text, method) = story_so_far(vault, &versions);
        if !text.trim().is_empty() {
            out.push(ai::Source {
                label: String::new(),
                kind: "understanding".into(),
                title: "The story so far".into(),
                status: "DERIVED".into(),
                authority: "reader's understanding (derived, not canon)".into(),
                text: clip(&text, 1400),
                detail: Some(format!("read from your current files in story order ({method})")),
                version_id: None,
                canon_id: None,
                is_current: true,
                warning: None,
                score: 0.0,
            });
        }
    }

    for e in aggregate_entities(vault)? {
        if out.len() >= max {
            break;
        }
        let mut forms = vec![e.name.to_lowercase()];
        forms.extend(e.aliases.iter().map(|a| a.to_lowercase()));
        let mentioned = forms.iter().any(|f| {
            let padded = format!(" {} ", q.replace(|c: char| !c.is_alphanumeric() && c != '\'', " "));
            padded.contains(&format!(" {f} "))
        });
        if !mentioned {
            continue;
        }
        let Ok(profile) = entity_profile(vault, &e.name) else { continue };
        if profile.facts.is_empty() && profile.events.is_empty() {
            continue;
        }
        let mut lines: Vec<String> = Vec::new();
        for f in profile.facts.iter().filter(|f| !f.inferred).take(5) {
            lines.push(format!(
                "{} {} {} [{}{}]",
                profile.name,
                f.predicate,
                clip(&f.object, 80),
                f.rel_path,
                if f.is_live { "" } else { ", older draft" }
            ));
        }
        for ev in profile.events.iter().take(2) {
            lines.push(format!("event: {} [{}]", clip(&ev.summary, 140), ev.rel_path));
        }
        if !profile.related.is_empty() {
            lines.push(format!(
                "often appears with {}",
                profile.related.iter().take(4).map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        let live_any = profile.facts.iter().any(|f| f.is_live) || profile.events.iter().any(|e| e.is_live);
        out.push(ai::Source {
            label: String::new(),
            kind: "understanding".into(),
            title: format!("{} — what the Reader has learned", profile.name),
            status: "DERIVED".into(),
            authority: "reader's understanding (derived, not canon)".into(),
            text: lines.join("\n"),
            detail: Some(format!("{} · read from {} file(s)", profile.kind, profile.files.len())),
            version_id: None,
            canon_id: None,
            is_current: live_any,
            warning: if live_any { None } else { Some("Only older drafts mention this — treat as history.".into()) },
            score: 0.0,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Going deeper with a local model
// ---------------------------------------------------------------------------

const READ_PROMPT: &str = "You are reading one passage of an author's unpublished novel, to learn the story. \
Use ONLY the text given. Do not add anything you cannot find in it. Reply with a single JSON object and nothing else:\n\
{\"summary\": \"2-3 plain sentences of what happens\",\n \"characters\": [{\"name\": \"...\", \"role\": \"...\"}],\n \"facts\": [{\"subject\": \"...\", \"predicate\": \"...\", \"object\": \"...\"}],\n \"events\": [\"one sentence per important event\"]}\n\
Facts are short statements the text makes outright (e.g. subject \"Azmirah\", predicate \"keeps\", object \"a wooden bird\"). \
Every name must be spelled as it is in the text.";

#[derive(Debug, Default, Clone)]
pub struct ModelReading {
    pub summary: String,
    pub characters: Vec<String>,
    pub facts: Vec<(String, String, String)>,
    pub events: Vec<String>,
    pub dropped: usize,
}

/// Fraction of the longer words of `claim` that appear in `source` (both lowercased).
fn grounded_ratio(claim: &str, source_lower: &str) -> f64 {
    let toks: Vec<String> = claim
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 4)
        .map(String::from)
        .collect();
    if toks.is_empty() {
        return 0.0;
    }
    toks.iter().filter(|t| source_lower.contains(t.as_str())).count() as f64 / toks.len() as f64
}

/// Parse a model's reply and keep only what the source text supports.
pub fn parse_model_reading(reply: &str, source: &str) -> Result<ModelReading> {
    let (Some(start), Some(end)) = (reply.find('{'), reply.rfind('}')) else {
        return Err(VaultError::Other("the model did not return a reading".into()));
    };
    let v: Value = serde_json::from_str(&reply[start..=end])?;
    let src = source.to_lowercase();
    let mut out = ModelReading::default();
    out.summary = v["summary"].as_str().unwrap_or("").trim().to_string();
    if !out.summary.is_empty() && grounded_ratio(&out.summary, &src) < 0.5 {
        out.summary.clear();
        out.dropped += 1;
    }
    for c in v["characters"].as_array().into_iter().flatten() {
        let name = c["name"].as_str().or_else(|| c.as_str()).unwrap_or("").trim().to_string();
        if !name.is_empty() && src.contains(&name.to_lowercase()) {
            out.characters.push(name);
        } else if !name.is_empty() {
            out.dropped += 1;
        }
    }
    for f in v["facts"].as_array().into_iter().flatten().take(40) {
        let (s, p, o) = (
            f["subject"].as_str().unwrap_or("").trim(),
            f["predicate"].as_str().unwrap_or("").trim(),
            f["object"].as_str().unwrap_or("").trim(),
        );
        if s.is_empty() || p.is_empty() || o.is_empty() {
            continue;
        }
        if src.contains(&s.to_lowercase()) && grounded_ratio(o, &src) >= 0.5 {
            out.facts.push((s.to_string(), p.to_lowercase(), o.to_string()));
        } else {
            out.dropped += 1;
        }
    }
    for e in v["events"].as_array().into_iter().flatten().take(MAX_EVENTS_PER_FILE) {
        let s = e.as_str().unwrap_or("").trim();
        if s.is_empty() {
            continue;
        }
        if grounded_ratio(s, &src) >= 0.6 {
            out.events.push(s.to_string());
        } else {
            out.dropped += 1;
        }
    }
    Ok(out)
}

/// Re-read up to `max_files` live files with the local model, replacing their rule-based
/// summary and adding model-read facts and events (each checked against the file text).
/// Then refresh the rolling "story so far". Returns the number of files deepened.
pub fn deepen_with_model(vault: &Vault, max_files: usize) -> Result<usize> {
    let cfg = ai::load_config(vault);
    if !cfg.enabled() {
        return Err(VaultError::Other("connect a local AI model first".into()));
    }
    crawl(vault, 50)?;
    let todo: Vec<FileVersion> = read_versions(vault)?
        .into_iter()
        .filter(|(v, method, _)| method != "model" && matches!(v.file_status.as_str(), "CURRENT" | "WORKING_DRAFT"))
        .map(|(v, _, _)| v)
        .take(max_files.max(1))
        .collect();

    let mut done = 0;
    for v in todo {
        let path = db::index_versions_dir(vault.root.as_path()).join(format!("{}.txt", v.id));
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let sample: String = text.chars().take(9000).collect();
        let reply = match llm::chat(&cfg, READ_PROMPT, &[("user".to_string(), sample.clone())]) {
            Ok(r) => r,
            Err(e) if done == 0 => return Err(e),
            Err(_) => break,
        };
        let Ok(reading) = parse_model_reading(&reply, &sample) else {
            continue; // an unusable reply is skipped, never guessed at
        };
        let tx = vault.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM reader_facts WHERE version_id = ?1 AND source = 'model'", params![v.id])?;
        tx.execute("DELETE FROM reader_events WHERE version_id = ?1 AND source = 'model'", params![v.id])?;
        for (s, p, o) in &reading.facts {
            tx.execute(
                "INSERT INTO reader_facts (id, version_id, ordinal, subject, predicate, object, excerpt, source, inferred)
                 VALUES (?1,?2,0,?3,?4,?5,?6,'model',0)",
                params![new_id(), v.id, s, p, o, format!("{s} {p} {o}")],
            )?;
        }
        for (seq, e) in reading.events.iter().enumerate() {
            let ents: Vec<String> = reading.characters.iter().filter(|c| e.to_lowercase().contains(&c.to_lowercase())).cloned().collect();
            tx.execute(
                "INSERT INTO reader_events (id, version_id, ordinal, seq, summary, entities_json, salience, source)
                 VALUES (?1,?2,0,?3,?4,?5,5,'model')",
                params![new_id(), v.id, 1000 + seq as i64, e, serde_json::to_string(&ents)?],
            )?;
        }
        for c in &reading.characters {
            tx.execute(
                "INSERT INTO reader_names (version_id, name, mid, total, person_cue, place_cue) VALUES (?1,?2,2,3,2,0)
                 ON CONFLICT(version_id, name) DO UPDATE SET person_cue = person_cue + 2,
                     mid = MAX(mid, 2), total = MAX(total, 3)",
                params![v.id, c],
            )?;
        }
        if !reading.summary.is_empty() {
            tx.execute("UPDATE reader_files SET summary = ?2, method = 'model' WHERE version_id = ?1", params![v.id, reading.summary])?;
        } else {
            tx.execute("UPDATE reader_files SET method = 'model' WHERE version_id = ?1", params![v.id])?;
        }
        tx.commit()?;
        done += 1;
    }
    if done > 0 {
        refresh_story_so_far(vault, &cfg)?;
        sync_change_notices(vault)?;
        vault.audit("ReaderDeepen", "Novel", None, None, Some(&format!("{done} files")))?;
    }
    Ok(done)
}

fn refresh_story_so_far(vault: &Vault, cfg: &llm::LlmConfig) -> Result<()> {
    let versions = read_versions(vault)?;
    let live: Vec<&(FileVersion, String, Option<String>)> = versions.iter().filter(|(v, _, _)| is_live_status(&v.file_status)).collect();
    if live.is_empty() {
        return Ok(());
    }
    let mut so_far = String::new();
    for (v, _, summary) in live {
        let Some(s) = summary.as_ref().filter(|s| !s.trim().is_empty()) else { continue };
        let prompt = format!(
            "STORY SO FAR:\n{}\n\nNEXT PART ({}):\n{}\n\nRewrite the story so far in at most 150 words, adding the next part. Use only what is written above.",
            if so_far.is_empty() { "(nothing yet)" } else { &so_far },
            label_of(v),
            s
        );
        match llm::chat(cfg, "You keep a running summary of an author's novel, accurately and briefly.", &[("user".to_string(), prompt)]) {
            Ok(t) => so_far = t,
            Err(_) => return Ok(()), // keep the previous summary rather than a half-built one
        }
    }
    if !so_far.trim().is_empty() {
        vault.setting_set(STORY_SO_FAR_KEY, so_far.trim())?;
        vault.setting_set(STORY_SO_FAR_SIG, &live_signature(&versions))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn sentences_split_on_terminators_and_paragraphs() {
        let s = split_sentences("Azmirah ran. She did not look back!\n\n“Wait,” said Lucifer. Nothing moved");
        assert_eq!(s.len(), 4);
        assert!(s[2].contains("Lucifer"));
    }

    #[test]
    fn a_name_at_the_start_of_a_sentence_needs_a_verb_or_prior_knowledge() {
        let words = words_of("Azmirah kept the bird.");
        let m = find_mentions(&words, &none());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].phrase, "Azmirah");
        let words = words_of("Quietly the room emptied.");
        assert!(find_mentions(&words, &none()).is_empty());
        let words = words_of("Zorblax the elder stood.");
        assert!(find_mentions(&words, &none()).is_empty(), "unknown, no verb next");
        let known: HashSet<String> = ["zorblax".to_string()].into();
        assert_eq!(find_mentions(&words, &known)[0].phrase, "Zorblax");
    }

    #[test]
    fn multi_word_names_merge_and_punctuation_breaks_them() {
        let m = find_mentions(&words_of("She met Lucifer Morningstar at dawn."), &none());
        assert_eq!(m[0].phrase, "Lucifer Morningstar");
        let m = find_mentions(&words_of("He saw Maheen, Azmirah and the king."), &none());
        let phrases: Vec<&str> = m.iter().map(|x| x.phrase.as_str()).collect();
        assert_eq!(phrases, vec!["Maheen", "Azmirah"]);
    }

    #[test]
    fn facts_are_extracted_with_their_source_sentence() {
        let r = read_text("Azmirah kept the wooden bird on her windowsill. The wooden bird had been her brother's.", &none());
        let f = r.facts.iter().find(|f| f.predicate == "keeps").expect("a keeps fact");
        assert_eq!(f.subject, "Azmirah");
        assert_eq!(f.object, "the wooden bird on her windowsill");
        assert!(f.excerpt.starts_with("Azmirah kept"));
        assert!(!f.inferred);
    }

    #[test]
    fn pronouns_resolve_to_the_last_named_person_and_are_marked_inferred() {
        let r = read_text("Azmirah crossed the courtyard. She lived in Zanzibar.", &none());
        let f = r.facts.iter().find(|f| f.predicate == "lives in").expect("lives in");
        assert_eq!(f.subject, "Azmirah");
        assert_eq!(f.object, "Zanzibar");
        assert!(f.inferred, "a guessed 'she' must say so");
    }

    #[test]
    fn negations_are_not_stated_as_facts() {
        let r = read_text("Azmirah never kept the bird. Lucifer did not love her.", &none());
        assert!(r.facts.is_empty());
    }

    #[test]
    fn events_keep_the_important_ones_in_story_order() {
        let mut text = String::new();
        for i in 0..30 {
            text.push_str(&format!("Azmirah walked to the market number {i}.\n"));
        }
        text.push_str("Lucifer killed the guard and escaped.\n");
        let r = read_text(&text, &none());
        assert!(r.events.len() <= MAX_EVENTS_PER_FILE);
        assert!(r.events.iter().any(|e| e.summary.contains("killed")), "the dramatic one is kept");
        let ords: Vec<usize> = r.events.iter().map(|e| e.ordinal).collect();
        let mut sorted = ords.clone();
        sorted.sort();
        assert_eq!(ords, sorted);
    }

    #[test]
    fn open_threads_are_spotted_but_not_in_dialogue() {
        let r = read_text("Nobody knew where the key was hidden. “Where is it?” she asked. Why had he gone?", &none());
        assert_eq!(r.threads.len(), 2);
    }

    #[test]
    fn story_order_is_natural_not_alphabetical() {
        let mut a = natural_key("Arc 2 Chapter 10.docx");
        let mut b = natural_key("Arc 2 Chapter 9.docx");
        assert!(a > b, "chapter 10 comes after chapter 9");
        a.clear();
        b.clear();
    }

    #[test]
    fn a_changed_object_is_a_change_but_extra_detail_is_not() {
        assert!(differs("the wooden bird on her windowsill", "a picture frame on her windowsill"));
        assert!(!differs("the wooden bird on her windowsill", "the wooden bird"));
        assert!(!differs("her wooden bird", "the wooden bird by the door"));
    }

    #[test]
    fn model_claims_are_kept_only_when_the_text_supports_them() {
        let text = "Azmirah kept the wooden bird on her windowsill. Lucifer watched from the doorway.";
        let reply = r#"Sure! {"summary":"Azmirah keeps a wooden bird while Lucifer watches.",
            "characters":[{"name":"Azmirah"},{"name":"Gandalf"}],
            "facts":[{"subject":"Azmirah","predicate":"keeps","object":"a wooden bird"},
                     {"subject":"Azmirah","predicate":"rules","object":"the kingdom of Narnia"},
                     {"subject":"Frodo","predicate":"has","object":"a ring"}],
            "events":["Lucifer watches from the doorway","A dragon burns the city of Atlantis"]}"#;
        let r = parse_model_reading(reply, text).unwrap();
        assert_eq!(r.characters, vec!["Azmirah"]);
        assert_eq!(r.facts.len(), 1);
        assert_eq!(r.events, vec!["Lucifer watches from the doorway"]);
        assert!(r.dropped >= 4, "invented material is discarded and counted");
        assert!(parse_model_reading("no json here", text).is_err());
    }
}
