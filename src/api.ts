import { invoke } from "@tauri-apps/api/core";
import type {
  AiAnswer,
  AiChatMessage,
  AiMemory,
  AiStatus,
  AiSuggestion,
  NovelSummary,
  AddOutcome,
  AddPlan,
  Arc,
  AuditRow,
  CanonConflict,
  CanonHistoryRow,
  CanonItem,
  CanonSource,
  Chapter,
  Character,
  Dashboard,
  FileVersion,
  Idea,
  ImpactReport,
  InboxItem,
  KnowledgeRow,
  NoteRow,
  OpenReport,
  ReaderCrawl,
  ReaderProfile,
  ReaderUnderstanding,
  ScanSummary,
  SearchHit,
  SnapshotRecord,
} from "./types";

// Every call below maps to one Tauri command in src-tauri/src/main.rs, which in turn
// calls novelvault-core. The UI has no other way to touch the vault.

export const api = {
  // ---- novel folder -------------------------------------------------------
  openNovel: (path: string, title?: string) =>
    invoke<OpenReport>("open_novel", { path, title }),

  createNovel: (parent: string, name: string, title?: string) =>
    invoke<{ report: OpenReport; root: string; folder_created: boolean }>("create_novel", {
      parent,
      name,
      title: title ?? null,
    }),

  closeNovel: () => invoke<{ closed: boolean }>("close_novel"),

  currentNovel: () =>
    invoke<{ title: string; root: string; vault_dir: string }>("current_novel"),

  scan: () => invoke<ScanSummary>("scan_folder"),

  watchStart: (quietMs?: number) => invoke<{ watching: boolean }>("watch_start", { quietMs }),

  watchStop: () => invoke<{ watching: boolean }>("watch_stop"),

  // ---- browsing files into the novel folder -------------------------------
  planAddFiles: (paths: string[], moveFiles = false) =>
    invoke<AddPlan>("plan_add_files", { paths, moveFiles }),

  applyAddFiles: (paths: string[], moveFiles = false, confirmed = true) =>
    invoke<AddOutcome[]>("apply_add_files", { paths, moveFiles, confirmed }),

  // ---- files and versions -------------------------------------------------
  versions: (includeMissing = false) =>
    invoke<FileVersion[]>("list_versions", { includeMissing }),

  inbox: (includeResolved = false) =>
    invoke<InboxItem[]>("list_inbox", { includeResolved }),

  dashboard: () => invoke<Dashboard>("dashboard"),

  search: (query: string, limit = 30) => invoke<SearchHit[]>("search_vault", { query, limit }),

  audit: (limit = 50) => invoke<AuditRow[]>("audit_tail", { limit }),

  snapshots: () => invoke<SnapshotRecord[]>("list_snapshots"),

  classify: (
    versionId: string,
    fields: { status?: string; arc?: string; chapter?: string; versionLabel?: string }
  ) =>
    invoke<FileVersion>("classify", {
      versionId,
      status: fields.status ?? null,
      arc: fields.arc ?? null,
      chapter: fields.chapter ?? null,
      versionLabel: fields.versionLabel ?? null,
    }),

  setCurrent: (versionId: string) => invoke<FileVersion>("set_current", { versionId }),

  archive: (versionId: string) => invoke<FileVersion>("archive", { versionId }),

  resolveInbox: (
    itemId: string,
    action: string,
    fields: { status?: string; arc?: string; chapter?: string; versionLabel?: string } = {}
  ) =>
    invoke<InboxItem>("resolve_inbox", {
      itemId,
      action,
      status: fields.status ?? null,
      arc: fields.arc ?? null,
      chapter: fields.chapter ?? null,
      versionLabel: fields.versionLabel ?? null,
    }),

  mergeFamily: (fromFileId: string, intoFileId: string) =>
    invoke<{ merged: boolean }>("merge_family", { fromFileId, intoFileId }),

  snapshot: (name: string, withFiles = false) =>
    invoke<SnapshotRecord>("create_snapshot", { name, withFiles }),

  // ---- story records ------------------------------------------------------
  arcs: () => invoke<Arc[]>("story_arcs"),

  createArc: (number: number, title: string, purpose?: string, status?: string) =>
    invoke<Arc>("story_arc_create", {
      number,
      title,
      purpose: purpose ?? null,
      status: status ?? null,
    }),

  chapters: (arcId?: string) =>
    invoke<Chapter[]>("story_chapters", { arcId: arcId ?? null }),

  createChapter: (arcId: string, number: number, title: string, purpose?: string, status?: string) =>
    invoke<Chapter>("story_chapter_create", {
      arcId,
      number,
      title,
      purpose: purpose ?? null,
      status: status ?? null,
    }),

  updateChapter: (
    chapterId: string,
    fields: { title?: string; summary?: string; status?: string }
  ) =>
    invoke<Chapter>("story_chapter_update", {
      chapterId,
      title: fields.title ?? null,
      summary: fields.summary ?? null,
      status: fields.status ?? null,
    }),

  reorderChapters: (arcId: string, orderedIds: string[]) =>
    invoke<{ reordered: boolean }>("story_reorder_chapters", { arcId, orderedIds }),

  characters: () => invoke<Character[]>("story_characters"),

  createCharacter: (name: string, role?: string, description?: string) =>
    invoke<Character>("story_character_create", {
      name,
      role: role ?? null,
      description: description ?? null,
    }),

  notes: (category?: string, includeArchived = false) =>
    invoke<NoteRow[]>("notes_list", {
      category: category ?? null,
      includeArchived,
    }),

  createNote: (category: string, body: string, linkTo?: string) =>
    invoke<NoteRow>("note_create", { category, body, linkTo: linkTo ?? null }),

  linkNote: (noteId: string, entityType: string, entityId: string) =>
    invoke<{ linked: boolean }>("note_link", { noteId, entityType, entityId }),

  ideas: (status?: string) => invoke<Idea[]>("ideas_list", { status: status ?? null }),

  createIdea: (text: string) => invoke<Idea>("idea_create", { text }),

  updateIdea: (ideaId: string, status: string, reason?: string) =>
    invoke<Idea>("idea_update", { ideaId, status, reason: reason ?? null }),

  // ---- canon engine -------------------------------------------------------
  canon: (status?: string, includeSuperseded = false) =>
    invoke<CanonItem[]>("canon_list", {
      status: status ?? null,
      includeSuperseded,
    }),

  proposeCanon: (
    subject: string,
    value: string,
    fields: {
      category?: string;
      entityType?: string;
      entityId?: string;
      reason?: string;
    } = {}
  ) =>
    invoke<CanonItem>("canon_propose", {
      subject,
      value,
      category: fields.category ?? null,
      entityType: fields.entityType ?? null,
      entityId: fields.entityId ?? null,
      reason: fields.reason ?? null,
    }),

  addCanonSource: (
    canonId: string,
    sourceType: string,
    sourceId: string,
    sourceLabel?: string,
    location?: string
  ) =>
    invoke<CanonSource>("canon_add_source", {
      canonId,
      sourceType,
      sourceId,
      sourceLabel: sourceLabel ?? null,
      location: location ?? null,
    }),

  promoteCanon: (canonId: string, status: string, reason: string) =>
    invoke<CanonItem>("canon_promote", { canonId, status, reason }),

  changeCanonValue: (canonId: string, newValue: string, reason: string) =>
    invoke<CanonItem>("canon_change_value", { canonId, newValue, reason }),

  canonImpact: (canonId: string) => invoke<ImpactReport>("canon_impact", { canonId }),

  canonHistory: (canonId: string) => invoke<CanonHistoryRow[]>("canon_history", { canonId }),

  knowledge: (canonId?: string) =>
    invoke<KnowledgeRow[]>("canon_knowledge", { canonId: canonId ?? null }),

  setKnowledge: (characterId: string, canonId: string, stateValue: string, note?: string) =>
    invoke<KnowledgeRow>("canon_set_knowledge", {
      characterId,
      canonId,
      stateValue,
      note: note ?? null,
    }),

  probe: () =>
    invoke<{ conflicts_raised: number; subjects_checked: number; details: string[] }>(
      "canon_probe"
    ),

  conflicts: (includeResolved = false) =>
    invoke<CanonConflict[]>("canon_conflicts", { includeResolved }),

  resolveConflict: (conflictId: string, decision: string, reason: string) =>
    invoke<{ resolved: boolean }>("canon_resolve_conflict", { conflictId, decision, reason }),

  canonCounts: () => invoke<Record<string, number>>("canon_counts"),

  // ---- library of novels --------------------------------------------------
  libraryList: () => invoke<NovelSummary[]>("library_list"),
  libraryForget: (path: string) => invoke<{ removed: boolean }>("library_forget", { path }),

  // ---- local AI librarian -------------------------------------------------
  aiStatus: () => invoke<AiStatus>("ai_status"),
  aiConfigure: (provider: string, model?: string, host?: string, port?: number) =>
    invoke<AiStatus>("ai_configure", {
      provider,
      model: model ?? null,
      host: host ?? null,
      port: port ?? null,
    }),
  aiIndex: () =>
    invoke<{ indexed_files: number; passages: number; skipped_unreadable: number }>("ai_index"),
  aiAsk: (question: string) => invoke<AiAnswer>("ai_ask", { question }),
  aiHistory: (limit = 50) => invoke<AiChatMessage[]>("ai_history", { limit }),
  aiClearHistory: () => invoke<{ cleared: number }>("ai_clear_history"),
  aiMemory: () => invoke<AiMemory>("ai_memory"),
  aiSummarize: (maxFiles = 3) => invoke<{ summarized: number }>("ai_summarize", { maxFiles }),
  aiScanSuggestions: () => invoke<{ created: number }>("ai_scan_suggestions"),
  aiSuggestions: (status?: string) =>
    invoke<AiSuggestion[]>("ai_suggestions", { status: status ?? null }),
  aiDecideSuggestion: (id: string, decision: "accepted" | "dismissed", note?: string, asKind?: string) =>
    invoke<AiSuggestion>("ai_decide_suggestion", {
      id,
      decision,
      note: note ?? null,
      asKind: asKind ?? null,
    }),

  // The Reader — learns the plot from the drafts, in story order
  readerCrawl: (maxFiles = 10) => invoke<ReaderCrawl>("reader_crawl", { maxFiles }),
  readerUnderstanding: () => invoke<ReaderUnderstanding>("reader_understanding"),
  readerEntity: (name: string) => invoke<ReaderProfile>("reader_entity", { name }),
  readerDeepen: (maxFiles = 2) => invoke<{ deepened: number }>("reader_deepen", { maxFiles }),
};
