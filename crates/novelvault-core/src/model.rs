//! Domain vocabulary for V1. The status taxonomies are the ones fixed in the
//! specification; parsing is deliberately strict so a typo cannot quietly pick the
//! wrong status.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// File status — six values (spec §2.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatus {
    Current,
    WorkingDraft,
    Reference,
    Historical,
    Rejected,
    Unknown,
}

impl FileStatus {
    pub const ALL: [FileStatus; 6] = [
        FileStatus::Current,
        FileStatus::WorkingDraft,
        FileStatus::Reference,
        FileStatus::Historical,
        FileStatus::Rejected,
        FileStatus::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FileStatus::Current => "CURRENT",
            FileStatus::WorkingDraft => "WORKING_DRAFT",
            FileStatus::Reference => "REFERENCE",
            FileStatus::Historical => "HISTORICAL",
            FileStatus::Rejected => "REJECTED",
            FileStatus::Unknown => "UNKNOWN",
        }
    }

    pub fn marker(self) -> &'static str {
        match self {
            FileStatus::Current => "🟢",
            FileStatus::WorkingDraft => "🔵",
            FileStatus::Reference => "🟡",
            FileStatus::Historical => "🟠",
            FileStatus::Rejected => "🔴",
            FileStatus::Unknown => "⚫",
        }
    }

    /// Strict parse: `Unknown` must be spelled out; an empty value is *not* accepted.
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_uppercase().replace([' ', '-'], "_");
        Self::ALL.iter().copied().find(|s| s.as_str() == v)
    }
}

// ---------------------------------------------------------------------------
// Extraction state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtractionState {
    Pending,
    Ok,
    Failed,
    Unsupported,
}

impl ExtractionState {
    pub fn as_str(self) -> &'static str {
        match self {
            ExtractionState::Pending => "pending",
            ExtractionState::Ok => "ok",
            ExtractionState::Failed => "failed",
            ExtractionState::Unsupported => "unsupported",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pending" => Some(Self::Pending),
            "ok" => Some(Self::Ok),
            "failed" => Some(Self::Failed),
            "unsupported" => Some(Self::Unsupported),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Inbox — the only door into the structured index (spec §9)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InboxKind {
    NewFile,
    NewVersion,
    PossibleRename,
    PossibleDuplicate,
    Unclassified,
    ExtractionFailed,
    MissingFile,
    FamilyProposal,
}

impl InboxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            InboxKind::NewFile => "NEW_FILE",
            InboxKind::NewVersion => "NEW_VERSION",
            InboxKind::PossibleRename => "POSSIBLE_RENAME",
            InboxKind::PossibleDuplicate => "POSSIBLE_DUPLICATE",
            InboxKind::Unclassified => "UNCLASSIFIED",
            InboxKind::ExtractionFailed => "EXTRACTION_FAILED",
            InboxKind::MissingFile => "MISSING_FILE",
            InboxKind::FamilyProposal => "FAMILY_PROPOSAL",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_uppercase().replace([' ', '-'], "_");
        [
            Self::NewFile,
            Self::NewVersion,
            Self::PossibleRename,
            Self::PossibleDuplicate,
            Self::Unclassified,
            Self::ExtractionFailed,
            Self::MissingFile,
            Self::FamilyProposal,
        ]
        .into_iter()
        .find(|k| k.as_str() == v)
    }
    pub fn label(self) -> &'static str {
        match self {
            InboxKind::NewFile => "New file detected",
            InboxKind::NewVersion => "New version detected",
            InboxKind::PossibleRename => "Possible rename",
            InboxKind::PossibleDuplicate => "Possible duplicate",
            InboxKind::Unclassified => "Needs classification",
            InboxKind::ExtractionFailed => "Text extraction failed",
            InboxKind::MissingFile => "File no longer on disk",
            InboxKind::FamilyProposal => "Version family proposed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InboxStatus {
    Open,
    Resolved,
    Ignored,
}

impl InboxStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            InboxStatus::Open => "OPEN",
            InboxStatus::Resolved => "RESOLVED",
            InboxStatus::Ignored => "IGNORED",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "OPEN" => Some(Self::Open),
            "RESOLVED" => Some(Self::Resolved),
            "IGNORED" => Some(Self::Ignored),
            _ => None,
        }
    }
}

/// Actions an author can take on an Inbox item. None of them is ever applied
/// automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InboxAction {
    Classify,
    KeepSeparate,
    Relink,
    Ignore,
    AcceptChange,
    RejectChange,
}

impl InboxAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "classify" => Some(Self::Classify),
            "keep_separate" => Some(Self::KeepSeparate),
            "relink" => Some(Self::Relink),
            "ignore" => Some(Self::Ignore),
            "accept_change" | "accept" => Some(Self::AcceptChange),
            "reject_change" | "reject" => Some(Self::RejectChange),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classify => "classify",
            Self::KeepSeparate => "keep_separate",
            Self::Relink => "relink",
            Self::Ignore => "ignore",
            Self::AcceptChange => "accept_change",
            Self::RejectChange => "reject_change",
        }
    }
}

// ---------------------------------------------------------------------------
// Watcher events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Created,
    Modified,
    Removed,
    Renamed,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Created => "CREATED",
            EventKind::Modified => "MODIFIED",
            EventKind::Removed => "REMOVED",
            EventKind::Renamed => "RENAMED",
        }
    }
}

/// One coalesced filesystem event, already reduced to a novel-relative path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEvent {
    pub kind: EventKind,
    pub rel_path: String,
    pub at: String,
}

// ---------------------------------------------------------------------------
// Read models used by the CLI and the UI
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Novel {
    pub id: String,
    pub title: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileVersion {
    pub id: String,
    pub file_id: String,
    pub family_name: String,
    pub family_confirmed: bool,
    pub version_number: i64,
    pub rel_path: String,
    pub sha256: String,
    pub sha256_short: String,
    pub size_bytes: i64,
    pub mtime: Option<String>,
    pub extraction_state: String,
    pub extraction_note: Option<String>,
    pub extracted_chars: i64,
    pub file_status: String,
    pub status_marker: String,
    pub arc_label: Option<String>,
    pub chapter_label: Option<String>,
    pub version_label: Option<String>,
    pub is_current: bool,
    pub missing: bool,
    pub last_seen_at: Option<String>,
    pub updated_at: String,
    /// Resolved absolute path — computed for display only; never stored.
    pub abs_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InboxItem {
    pub id: String,
    pub kind: String,
    pub kind_label: String,
    pub title: String,
    pub detail: Option<String>,
    pub rel_path: Option<String>,
    pub file_version_id: Option<String>,
    pub candidate_version_id: Option<String>,
    pub status: String,
    pub created_at: String,
    pub resolved_action: Option<String>,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanSummary {
    pub root: String,
    pub files_seen: usize,
    pub by_kind: BTreeMap<String, usize>,
    pub imported: usize,
    pub unchanged: usize,
    pub new_versions: usize,
    pub rename_candidates: usize,
    pub duplicates: usize,
    pub marked_missing: usize,
    pub extraction_failed: usize,
    pub extraction_unsupported: usize,
    pub duration_ms: u128,
    /// A sentence a human can read, e.g. "17 files found — 8 DOCX, 5 PDF, 2 Markdown, 1 TXT, 1 image."
    pub headline: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub file_version_id: String,
    pub family_name: String,
    pub rel_path: String,
    pub version_number: i64,
    pub file_status: String,
    pub status_marker: String,
    pub is_current: bool,
    pub matched_in: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotRecord {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub rel_path: String,
    pub includes_files: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Dashboard {
    pub novel: Novel,
    pub files: i64,
    pub versions: i64,
    pub current_versions: i64,
    pub unclassified: i64,
    pub missing: i64,
    pub extraction_failed: i64,
    pub inbox_open: i64,
    pub inbox_by_kind: BTreeMap<String, i64>,
    pub snapshots: i64,
    /// Counters that belong to later versions report 0 honestly rather than guessing.
    pub conflicts: i64,
    pub ideas: i64,
    pub counts_note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenReport {
    pub novel: Novel,
    pub root: String,
    pub versions: i64,
    pub resolved: i64,
    pub missing: i64,
    pub hash_mismatches: Vec<String>,
    pub portable: bool,
}

// ---------------------------------------------------------------------------
// V2 — story records
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkStatus {
    Planned,
    Developing,
    Drafted,
    Revised,
    Locked,
}

impl WorkStatus {
    pub const ALL: [WorkStatus; 5] = [
        WorkStatus::Planned,
        WorkStatus::Developing,
        WorkStatus::Drafted,
        WorkStatus::Revised,
        WorkStatus::Locked,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            WorkStatus::Planned => "Planned",
            WorkStatus::Developing => "Developing",
            WorkStatus::Drafted => "Drafted",
            WorkStatus::Revised => "Revised",
            WorkStatus::Locked => "Locked",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_lowercase();
        Self::ALL.iter().copied().find(|s| s.as_str().to_ascii_lowercase() == v)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdeaStatus {
    Undecided,
    UnderReview,
    Accepted,
    Rejected,
}

impl IdeaStatus {
    pub const ALL: [IdeaStatus; 4] = [
        IdeaStatus::Undecided,
        IdeaStatus::UnderReview,
        IdeaStatus::Accepted,
        IdeaStatus::Rejected,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            IdeaStatus::Undecided => "UNDECIDED",
            IdeaStatus::UnderReview => "UNDER_REVIEW",
            IdeaStatus::Accepted => "ACCEPTED",
            IdeaStatus::Rejected => "REJECTED",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_uppercase().replace([' ', '-'], "_");
        Self::ALL.iter().copied().find(|s| s.as_str() == v)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteCategory {
    Quick,
    Character,
    Plot,
    DialogueIdea,
    SceneIdea,
    Research,
    Continuity,
    Question,
    Problem,
    Decision,
}

impl NoteCategory {
    pub const ALL: [NoteCategory; 10] = [
        NoteCategory::Quick,
        NoteCategory::Character,
        NoteCategory::Plot,
        NoteCategory::DialogueIdea,
        NoteCategory::SceneIdea,
        NoteCategory::Research,
        NoteCategory::Continuity,
        NoteCategory::Question,
        NoteCategory::Problem,
        NoteCategory::Decision,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            NoteCategory::Quick => "Quick",
            NoteCategory::Character => "Character",
            NoteCategory::Plot => "Plot",
            NoteCategory::DialogueIdea => "DialogueIdea",
            NoteCategory::SceneIdea => "SceneIdea",
            NoteCategory::Research => "Research",
            NoteCategory::Continuity => "Continuity",
            NoteCategory::Question => "Question",
            NoteCategory::Problem => "Problem",
            NoteCategory::Decision => "Decision",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_lowercase().replace([' ', '_', '-'], "");
        Self::ALL
            .iter()
            .copied()
            .find(|s| s.as_str().to_ascii_lowercase() == v)
    }
}

/// Entity types a note, a canon source or a knowledge row may point at.
pub mod entity_kinds {
    pub const NOVEL: &str = "Novel";
    pub const ARC: &str = "Arc";
    pub const CHAPTER: &str = "Chapter";
    pub const SCENE: &str = "Scene";
    pub const CHARACTER: &str = "Character";
    pub const LOCATION: &str = "Location";
    pub const OBJECT: &str = "Object";
    pub const NOTE: &str = "Note";
    pub const IDEA: &str = "Idea";
    pub const FILE_VERSION: &str = "FileVersion";
    pub const CANON_ITEM: &str = "CanonItem";

    pub const ALL: [&str; 11] = [
        NOVEL,
        ARC,
        CHAPTER,
        SCENE,
        CHARACTER,
        LOCATION,
        OBJECT,
        NOTE,
        IDEA,
        FILE_VERSION,
        CANON_ITEM,
    ];

    pub fn normalize(value: &str) -> Option<&'static str> {
        ALL.iter()
            .copied()
            .find(|k| k.eq_ignore_ascii_case(value.trim().replace([' ', '_', '-'], "").as_str()))
            .or_else(|| {
                ALL.iter()
                    .copied()
                    .find(|k| k.eq_ignore_ascii_case(value.trim()))
            })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Arc {
    pub id: String,
    pub novel_id: String,
    pub number: i64,
    pub title: String,
    pub purpose: Option<String>,
    pub summary: Option<String>,
    pub status: String,
    pub chapters: i64,
    pub words: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Chapter {
    pub id: String,
    pub arc_id: String,
    pub arc_number: i64,
    pub number: i64,
    pub title: String,
    pub purpose: Option<String>,
    pub summary: Option<String>,
    pub emotional_movement: Option<String>,
    pub status: String,
    pub word_count: i64,
    pub scenes: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scene {
    pub id: String,
    pub chapter_id: String,
    pub number: i64,
    pub title: String,
    pub purpose: Option<String>,
    pub pov_character_id: Option<String>,
    pub location: Option<String>,
    pub summary: Option<String>,
    pub word_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    pub description: Option<String>,
    /// The single row that stands for "what the reader knows" (R4).
    pub is_reader_sentinel: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub id: String,
    pub category: String,
    pub body: String,
    pub is_archived: bool,
    pub created_at: String,
    pub links: Vec<NoteLink>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteLink {
    pub entity_type: String,
    pub entity_id: String,
    pub link_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Idea {
    pub id: String,
    pub text: String,
    pub status: String,
    pub decision_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoryObject {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// V2 — browsing files into the novel folder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AddAction {
    Copy,
    Move,
    Skip,
}

impl AddAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AddAction::Copy => "COPY",
            AddAction::Move => "MOVE",
            AddAction::Skip => "SKIP",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AddEntry {
    pub source_path: String,
    pub file_name: String,
    pub rel_target: String,
    pub abs_target: String,
    pub size_bytes: i64,
    pub action: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddPlan {
    pub novel_root: String,
    pub move_files: bool,
    pub entries: Vec<AddEntry>,
    pub skipped: Vec<AddEntry>,
    pub total_bytes: i64,
    pub headline: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddOutcome {
    pub source_path: String,
    pub rel_target: String,
    pub action: String,
    pub sha256: String,
    pub verified: bool,
    pub version_id: Option<String>,
    pub source_removed: bool,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// V3 — canon
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanonStatus {
    Unknown,
    Draft,
    Proposed,
    WorkingCanon,
    Canon,
    LockedCanon,
    Outdated,
    Rejected,
}

impl CanonStatus {
    pub const ALL: [CanonStatus; 8] = [
        CanonStatus::Unknown,
        CanonStatus::Draft,
        CanonStatus::Proposed,
        CanonStatus::WorkingCanon,
        CanonStatus::Canon,
        CanonStatus::LockedCanon,
        CanonStatus::Outdated,
        CanonStatus::Rejected,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CanonStatus::Unknown => "UNKNOWN",
            CanonStatus::Draft => "DRAFT",
            CanonStatus::Proposed => "PROPOSED",
            CanonStatus::WorkingCanon => "WORKING_CANON",
            CanonStatus::Canon => "CANON",
            CanonStatus::LockedCanon => "LOCKED_CANON",
            CanonStatus::Outdated => "OUTDATED",
            CanonStatus::Rejected => "REJECTED",
        }
    }

    /// Display form: the specification writes these with a space.
    pub fn label(self) -> &'static str {
        match self {
            CanonStatus::WorkingCanon => "WORKING CANON",
            CanonStatus::LockedCanon => "LOCKED CANON",
            other => other.as_str(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_uppercase().replace([' ', '-'], "_");
        Self::ALL.iter().copied().find(|s| s.as_str() == v)
    }

    /// R3 applies to these: a canon fact needs at least one source.
    pub fn requires_source(self) -> bool {
        matches!(self, CanonStatus::WorkingCanon | CanonStatus::Canon | CanonStatus::LockedCanon)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KnowledgeState {
    Knows,
    DoesNotKnow,
    Partial,
    BelievesFalsely,
    Unknown,
}

impl KnowledgeState {
    pub const ALL: [KnowledgeState; 5] = [
        KnowledgeState::Knows,
        KnowledgeState::DoesNotKnow,
        KnowledgeState::Partial,
        KnowledgeState::BelievesFalsely,
        KnowledgeState::Unknown,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            KnowledgeState::Knows => "KNOWS",
            KnowledgeState::DoesNotKnow => "DOES_NOT_KNOW",
            KnowledgeState::Partial => "PARTIAL",
            KnowledgeState::BelievesFalsely => "BELIEVES_FALSELY",
            KnowledgeState::Unknown => "UNKNOWN",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            KnowledgeState::Knows => "✓",
            KnowledgeState::DoesNotKnow => "✗",
            KnowledgeState::Partial => "〜",
            KnowledgeState::BelievesFalsely => "✗!",
            KnowledgeState::Unknown => "?",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_uppercase().replace([' ', '-'], "_");
        Self::ALL.iter().copied().find(|s| s.as_str() == v)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CanonItem {
    pub id: String,
    pub subject: String,
    pub value: String,
    pub category: Option<String>,
    pub status: String,
    pub status_label: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub reason: Option<String>,
    pub superseded_by_id: Option<String>,
    pub sources: Vec<CanonSource>,
    pub is_superseded: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CanonSource {
    pub id: String,
    pub source_type: String,
    pub source_id: String,
    /// A NULL label means "the file name is the label"; it is not an error.
    pub source_label: Option<String>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CanonHistoryRow {
    pub id: String,
    pub canon_item_id: String,
    pub action: String,
    pub reason: Option<String>,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CanonConflict {
    pub id: String,
    pub subject: String,
    pub current_value: Option<String>,
    pub conflicting_value: Option<String>,
    pub current_source: Option<String>,
    pub conflicting_source: Option<String>,
    pub detector: String,
    pub status: String,
    pub resolution: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeRow {
    pub id: String,
    pub character_id: String,
    pub character_name: String,
    pub canon_item_id: String,
    pub subject: String,
    pub state: String,
    pub glyph: String,
    pub note: Option<String>,
}

/// §29 — what a change to this canon fact would touch.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    pub canon_item_id: String,
    pub subject: String,
    pub value: String,
    pub status: String,
    pub is_locked: bool,
    pub affected_notes: Vec<String>,
    pub affected_characters: Vec<String>,
    pub affected_conflicts: Vec<String>,
    pub files_mentioning_old_value: Vec<String>,
    pub files_mentioning_subject: Vec<String>,
    pub chapters_mentioning_subject: Vec<String>,
    pub summary: String,
}
