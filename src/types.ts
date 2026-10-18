// Typed mirror of the Rust read models (novelvault-core::model).
// Keeping these in one file means a schema change shows up in one place.

export interface Novel {
  id: string;
  title: string;
  created_at: string;
}

export interface FileVersion {
  id: string;
  file_id: string;
  family_name: string;
  family_confirmed: boolean;
  version_number: number;
  rel_path: string;
  sha256: string;
  sha256_short: string;
  size_bytes: number;
  mtime: string | null;
  extraction_state: string;
  extraction_note: string | null;
  extracted_chars: number;
  file_status: string;
  status_marker: string;
  arc_label: string | null;
  chapter_label: string | null;
  version_label: string | null;
  is_current: boolean;
  missing: boolean;
  last_seen_at: string | null;
  updated_at: string;
  abs_path: string;
}

export interface InboxItem {
  id: string;
  kind: string;
  kind_label: string;
  title: string;
  detail: string | null;
  rel_path: string | null;
  file_version_id: string | null;
  candidate_version_id: string | null;
  status: string;
  created_at: string;
  resolved_action: string | null;
  resolved_at: string | null;
}

export interface ScanSummary {
  root: string;
  files_seen: number;
  by_kind: Record<string, number>;
  imported: number;
  unchanged: number;
  new_versions: number;
  rename_candidates: number;
  duplicates: number;
  marked_missing: number;
  extraction_failed: number;
  extraction_unsupported: number;
  duration_ms: number;
  headline: string;
}

export interface SearchHit {
  file_version_id: string;
  family_name: string;
  rel_path: string;
  version_number: number;
  file_status: string;
  status_marker: string;
  is_current: boolean;
  matched_in: string;
  snippet: string;
}

export interface Dashboard {
  novel: Novel;
  files: number;
  versions: number;
  current_versions: number;
  unclassified: number;
  missing: number;
  extraction_failed: number;
  inbox_open: number;
  inbox_by_kind: Record<string, number>;
  snapshots: number;
  conflicts: number;
  ideas: number;
  counts_note: string;
}

export interface OpenReport {
  novel: Novel;
  root: string;
  versions: number;
  resolved: number;
  missing: number;
  hash_mismatches: string[];
  portable: boolean;
}

export interface SnapshotRecord {
  id: string;
  name: string;
  created_at: string;
  rel_path: string;
  includes_files: boolean;
  note: string | null;
}

export interface AuditRow {
  at: string;
  action: string;
  entityType: string;
  entityId: string;
}

export const FILE_STATUSES = [
  "CURRENT",
  "WORKING_DRAFT",
  "REFERENCE",
  "HISTORICAL",
  "REJECTED",
  "UNKNOWN",
] as const;

export type FileStatusValue = (typeof FILE_STATUSES)[number];

export function statusTone(status: string): string {
  switch (status) {
    case "CURRENT":
      return "tone-current";
    case "WORKING_DRAFT":
      return "tone-draft";
    case "REFERENCE":
      return "tone-reference";
    case "HISTORICAL":
      return "tone-historical";
    case "REJECTED":
      return "tone-rejected";
    default:
      return "tone-unknown";
  }
}

// ---------------------------------------------------------------------------
// Phase 2 — story records
// ---------------------------------------------------------------------------

export const WORK_STATUSES = ["Planned", "Developing", "Drafted", "Revised", "Locked"] as const;

export const NOTE_CATEGORIES = [
  "Quick",
  "Character",
  "Plot",
  "DialogueIdea",
  "SceneIdea",
  "Research",
  "Continuity",
  "Question",
  "Problem",
  "Decision",
] as const;

export const IDEA_STATUSES = ["UNDECIDED", "UNDER_REVIEW", "ACCEPTED", "REJECTED"] as const;

export interface Arc {
  id: string;
  novel_id: string;
  number: number;
  title: string;
  purpose: string | null;
  summary: string | null;
  status: string;
  chapters: number;
  words: number;
}

export interface Chapter {
  id: string;
  arc_id: string;
  arc_number: number;
  number: number;
  title: string;
  purpose: string | null;
  summary: string | null;
  emotional_movement: string | null;
  status: string;
  word_count: number;
  scenes: number;
}

export interface Character {
  id: string;
  name: string;
  role: string | null;
  description: string | null;
  is_reader_sentinel: boolean;
}

export interface NoteLink {
  entity_type: string;
  entity_id: string;
  link_type: string | null;
}

export interface NoteRow {
  id: string;
  category: string;
  body: string;
  is_archived: boolean;
  created_at: string;
  links: NoteLink[];
}

export interface Idea {
  id: string;
  text: string;
  status: string;
  decision_reason: string | null;
}

// ---------------------------------------------------------------------------
// Phase 2 — browsing files into the novel folder
// ---------------------------------------------------------------------------

export interface AddEntry {
  source_path: string;
  file_name: string;
  rel_target: string;
  abs_target: string;
  size_bytes: number;
  action: string;
  note: string | null;
}

export interface AddPlan {
  novel_root: string;
  move_files: boolean;
  entries: AddEntry[];
  skipped: AddEntry[];
  total_bytes: number;
  headline: string;
}

export interface AddOutcome {
  source_path: string;
  rel_target: string;
  action: string;
  sha256: string;
  verified: boolean;
  version_id: string | null;
  source_removed: boolean;
  error: string | null;
}

// ---------------------------------------------------------------------------
// Phase 3 — canon engine
// ---------------------------------------------------------------------------

export const CANON_STATUSES = [
  "PROPOSED",
  "WORKING_CANON",
  "CANON",
  "LOCKED_CANON",
  "REJECTED",
] as const;

export const KNOWLEDGE_STATES = [
  "KNOWS",
  "DOES_NOT_KNOW",
  "PARTIAL",
  "BELIEVES_FALSELY",
  "UNKNOWN",
] as const;

export interface CanonSource {
  id: string;
  source_type: string;
  source_id: string;
  source_label: string | null;
  location: string | null;
}

export interface CanonItem {
  id: string;
  subject: string;
  value: string;
  category: string | null;
  status: string;
  status_label: string;
  entity_type: string | null;
  entity_id: string | null;
  reason: string | null;
  superseded_by_id: string | null;
  sources: CanonSource[];
  is_superseded: boolean;
}

export interface CanonConflict {
  id: string;
  subject: string;
  current_value: string | null;
  conflicting_value: string | null;
  current_source: string | null;
  conflicting_source: string | null;
  detector: string;
  status: string;
  resolution: string | null;
  created_at: string;
}

export interface KnowledgeRow {
  id: string;
  character_id: string;
  character_name: string;
  canon_item_id: string;
  subject: string;
  state: string;
  glyph: string;
  note: string | null;
}

export interface ImpactReport {
  canon_item_id: string;
  subject: string;
  value: string;
  status: string;
  is_locked: boolean;
  affected_notes: string[];
  affected_characters: string[];
  affected_conflicts: string[];
  files_mentioning_old_value: string[];
  files_mentioning_subject: string[];
  chapters_mentioning_subject: string[];
  summary: string;
}

export interface CanonHistoryRow {
  id: string;
  canon_item_id: string;
  action: string;
  reason: string | null;
  before_json: string | null;
  after_json: string | null;
  at: string;
}

export function canonTone(status: string): string {
  switch (status) {
    case "LOCKED_CANON":
      return "canon-locked";
    case "CANON":
      return "canon-canon";
    case "WORKING_CANON":
      return "canon-working";
    case "PROPOSED":
      return "canon-proposed";
    case "OUTDATED":
      return "canon-outdated";
    case "REJECTED":
      return "canon-rejected";
    default:
      return "canon-unknown";
  }
}

// ---- Phase 4: local AI librarian and the library of novels ------------------------

export interface LlmConfig {
  provider: "none" | "ollama" | "openai_local";
  host: string;
  port: number;
  model: string;
}

export interface AiStatus {
  config: LlmConfig;
  mode: "model" | "offline";
  reachable: boolean;
  models: string[];
  note: string;
  indexed_files: number;
  indexed_passages: number;
  pending_files: number;
}

export interface AiSource {
  label: string;
  kind: "canon" | "file" | "understanding";
  title: string;
  status: string;
  authority: string;
  text: string;
  detail: string | null;
  version_id: string | null;
  canon_id: string | null;
  is_current: boolean;
  warning: string | null;
  score: number;
}

export interface AiAnswer {
  id: string;
  question: string;
  answer: string;
  mode: "model" | "offline";
  model: string | null;
  sources: AiSource[];
  cited: string[];
  warnings: string[];
  note: string | null;
}

export interface AiChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  mode: string | null;
  sources: AiSource[];
  created_at: string;
}

export interface AiFileMemory {
  version_id: string;
  family_name: string;
  rel_path: string;
  version_number: number;
  file_status: string;
  is_current: boolean;
  arc_label: string | null;
  chapter_label: string | null;
  digest: string;
  method: "extractive" | "model";
}

export interface AiEntityMemory {
  kind: "character" | "location" | "object";
  name: string;
  passages_live: number;
  passages_older: number;
}

export interface AiMemory {
  files: AiFileMemory[];
  entities: AiEntityMemory[];
  counts: Record<string, number>;
  pending_suggestions: number;
}

export interface AiSuggestion {
  id: string;
  kind: "new_entity" | "idea_pending" | "story_change" | string;
  title: string;
  detail: string;
  evidence: string[];
  status: "pending" | "accepted" | "dismissed";
  decision_note: string | null;
  created_at: string;
}

export interface NovelSummary {
  path: string;
  title: string;
  last_opened_at: string;
  exists: boolean;
  has_vault: boolean;
  files: number;
  current_files: number;
  arcs: number;
  chapters: number;
  characters: number;
  canon_items: number;
  open_conflicts: number;
  pending_ai_suggestions: number;
  last_activity: string | null;
}

// --- The Reader (Phase 5) ---------------------------------------------------

export interface ReaderStep {
  version_id: string;
  rel_path: string;
  sentences: number;
  facts: number;
  events: number;
  threads: number;
  learned: string[];
}

export interface ReaderCrawl {
  files_read: number;
  steps: ReaderStep[];
  remaining: number;
  new_notices: number;
}

export interface ReaderCoverage {
  files_total: number;
  files_read: number;
  files_pending: number;
  files_deepened: number;
  sentences_read: number;
}

export interface ReaderEntity {
  name: string;
  kind: "character" | "location" | "object" | "unknown";
  aliases: string[];
  mentions_live: number;
  mentions_older: number;
  files: number;
  facts: number;
  recorded: boolean;
}

export interface ReaderChapter {
  version_id: string;
  rel_path: string;
  label: string;
  summary: string;
  method: "heuristic" | "model";
  is_live: boolean;
}

export interface ReaderEvent {
  version_id: string;
  rel_path: string;
  label: string;
  summary: string;
  entities: string[];
  is_live: boolean;
  source: "heuristic" | "model";
}

export interface ReaderThread {
  rel_path: string;
  excerpt: string;
  cue: string;
  entities: string[];
  is_live: boolean;
}

export interface ReaderChange {
  subject: string;
  predicate: string;
  old_value: string;
  old_file: string;
  new_value: string;
  new_file: string;
  kind: "revision" | "contradiction";
}

export interface ReaderUnderstanding {
  coverage: ReaderCoverage;
  entities: ReaderEntity[];
  chapters: ReaderChapter[];
  timeline: ReaderEvent[];
  threads: ReaderThread[];
  changes: ReaderChange[];
  story_so_far: string;
  story_so_far_method: "reading" | "model";
}

export interface ReaderFact {
  predicate: string;
  object: string;
  excerpt: string;
  rel_path: string;
  file_status: string;
  is_live: boolean;
  inferred: boolean;
  source: "heuristic" | "model";
}

export interface ReaderProfile {
  name: string;
  kind: string;
  aliases: string[];
  facts: ReaderFact[];
  events: ReaderEvent[];
  related: Array<[string, number]>;
  files: string[];
  summary: string;
}
