import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { Arc, Chapter, Idea, NoteRow } from "../types";
import { IDEA_STATUSES, NOTE_CATEGORIES, WORK_STATUSES } from "../types";

/**
 * Story structure — the planning record, not the documents.
 *
 * Reordering here moves chapter numbers in the database; the author's files stay
 * exactly where they are, untouched.
 */
export default function StoryPage({
  onChanged,
  onError,
}: {
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  const [arcs, setArcs] = useState<Arc[]>([]);
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [ideas, setIdeas] = useState<Idea[]>([]);
  const [notes, setNotes] = useState<NoteRow[]>([]);
  const [arcNumber, setArcNumber] = useState("1");
  const [arcTitle, setArcTitle] = useState("");
  const [chapterArc, setChapterArc] = useState("");
  const [chapterNumber, setChapterNumber] = useState("1");
  const [chapterTitle, setChapterTitle] = useState("");
  const [ideaText, setIdeaText] = useState("");
  const [noteCategory, setNoteCategory] = useState<string>(NOTE_CATEGORIES[0]);
  const [noteBody, setNoteBody] = useState("");

  const load = useCallback(async () => {
    try {
      const [arcRows, chapterRows, ideaRows, noteRows] = await Promise.all([
        api.arcs(),
        api.chapters(),
        api.ideas(),
        api.notes(undefined, false),
      ]);
      setArcs(arcRows);
      setChapters(chapterRows);
      setIdeas(ideaRows);
      setNotes(noteRows);
      if (arcRows.length && !chapterArc) setChapterArc(arcRows[0].id);
    } catch (e) {
      onError(String(e));
    }
  }, [chapterArc, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  async function addArc() {
    try {
      await api.createArc(Number(arcNumber), arcTitle.trim() || `Arc ${arcNumber}`);
      setArcTitle("");
      await load();
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  async function addChapter() {
    if (!chapterArc) {
      onError("Add an arc first.");
      return;
    }
    try {
      await api.createChapter(
        chapterArc,
        Number(chapterNumber),
        chapterTitle.trim() || `Chapter ${chapterNumber}`
      );
      setChapterTitle("");
      await load();
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  async function move(arc: Arc, index: number, delta: number) {
    const list = chapters.filter((c) => c.arc_id === arc.id);
    const target = index + delta;
    if (target < 0 || target >= list.length) return;
    const ids = list.map((c) => c.id);
    [ids[index], ids[target]] = [ids[target], ids[index]];
    try {
      await api.reorderChapters(arc.id, ids);
      await load();
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  async function setStatus(chapter: Chapter, status: string) {
    try {
      await api.updateChapter(chapter.id, { status });
      await load();
    } catch (e) {
      onError(String(e));
    }
  }

  async function addIdea() {
    if (!ideaText.trim()) return;
    try {
      await api.createIdea(ideaText.trim());
      setIdeaText("");
      await load();
    } catch (e) {
      onError(String(e));
    }
  }

  async function decide(idea: Idea, status: string) {
    try {
      await api.updateIdea(idea.id, status, "decided in the Story view");
      await load();
    } catch (e) {
      onError(String(e));
    }
  }

  async function addNote() {
    if (!noteBody.trim()) return;
    try {
      await api.createNote(noteCategory, noteBody.trim());
      setNoteBody("");
      await load();
    } catch (e) {
      onError(String(e));
    }
  }

  return (
    <section>
      <div className="section-head">
        <h2>Story structure</h2>
        <span className="muted small">
          An idea is not canon, and a chapter record is not a file.
        </span>
      </div>

      <h3>Arcs and chapters</h3>
      <div className="controls">
        <input
          className="narrow"
          value={arcNumber}
          onChange={(e) => setArcNumber(e.target.value)}
          aria-label="Arc number"
        />
        <input
          value={arcTitle}
          onChange={(e) => setArcTitle(e.target.value)}
          placeholder="Arc title"
          aria-label="Arc title"
        />
        <button onClick={addArc}>Add arc</button>
      </div>

      {arcs.length === 0 && <p className="muted">No arcs yet.</p>}

      {arcs.map((arc) => {
        const list = chapters.filter((c) => c.arc_id === arc.id);
        return (
          <div className="arc-card" key={arc.id}>
            <div className="arc-head">
              <strong>
                Arc {arc.number} — {arc.title}
              </strong>
              <span className="badge">{arc.status}</span>
              <span className="muted small">
                {arc.chapters} chapter(s) · {arc.words} word(s)
              </span>
            </div>

            {list.length === 0 ? (
              <p className="muted small">No chapters recorded.</p>
            ) : (
              <table className="table">
                <thead>
                  <tr>
                    <th>#</th>
                    <th>Chapter</th>
                    <th>Status</th>
                    <th>Words</th>
                    <th>Order</th>
                  </tr>
                </thead>
                <tbody>
                  {list.map((chapter, index) => (
                    <tr key={chapter.id}>
                      <td>{chapter.number}</td>
                      <td>{chapter.title}</td>
                      <td>
                        <select
                          value={chapter.status}
                          onChange={(e) => void setStatus(chapter, e.target.value)}
                        >
                          {WORK_STATUSES.map((s) => (
                            <option key={s} value={s}>
                              {s}
                            </option>
                          ))}
                        </select>
                      </td>
                      <td>{chapter.word_count}</td>
                      <td className="actions">
                        <button
                          className="tiny-btn"
                          onClick={() => void move(arc, index, -1)}
                          disabled={index === 0}
                        >
                          ↑
                        </button>
                        <button
                          className="tiny-btn"
                          onClick={() => void move(arc, index, 1)}
                          disabled={index === list.length - 1}
                        >
                          ↓
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}

            <div className="controls">
              <select value={chapterArc} onChange={(e) => setChapterArc(e.target.value)}>
                {arcs.map((a) => (
                  <option key={a.id} value={a.id}>
                    Arc {a.number}
                  </option>
                ))}
              </select>
              <input
                className="narrow"
                value={chapterNumber}
                onChange={(e) => setChapterNumber(e.target.value)}
                aria-label="Chapter number"
              />
              <input
                value={chapterTitle}
                onChange={(e) => setChapterTitle(e.target.value)}
                placeholder="Chapter title"
                aria-label="Chapter title"
              />
              <button onClick={addChapter}>Add chapter</button>
            </div>
          </div>
        );
      })}

      <h3>Ideas — not canon</h3>
      <div className="controls">
        <input
          value={ideaText}
          onChange={(e) => setIdeaText(e.target.value)}
          placeholder="What if…"
          aria-label="Idea"
        />
        <button onClick={addIdea}>Capture idea</button>
      </div>
      <table className="table">
        <thead>
          <tr>
            <th>Idea</th>
            <th>Status</th>
            <th>Decide</th>
          </tr>
        </thead>
        <tbody>
          {ideas.map((idea) => (
            <tr key={idea.id}>
              <td>
                {idea.text}
                {idea.decision_reason && (
                  <div className="muted tiny">{idea.decision_reason}</div>
                )}
              </td>
              <td>
                <span className="badge">{idea.status}</span>
              </td>
              <td className="actions">
                {IDEA_STATUSES.map((status) => (
                  <button
                    key={status}
                    className="tiny-btn"
                    onClick={() => void decide(idea, status)}
                    disabled={idea.status === status}
                  >
                    {status.replace("_", " ").toLowerCase()}
                  </button>
                ))}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <h3>Notes</h3>
      <div className="controls">
        <select value={noteCategory} onChange={(e) => setNoteCategory(e.target.value)}>
          {NOTE_CATEGORIES.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        <input
          value={noteBody}
          onChange={(e) => setNoteBody(e.target.value)}
          placeholder="Note text"
          aria-label="Note text"
        />
        <button onClick={addNote}>Add note</button>
      </div>
      <table className="table">
        <thead>
          <tr>
            <th>Category</th>
            <th>Note</th>
            <th>Linked to</th>
          </tr>
        </thead>
        <tbody>
          {notes.map((note) => (
            <tr key={note.id}>
              <td>
                <span className="badge">{note.category}</span>
              </td>
              <td>{note.body}</td>
              <td className="mono tiny">
                {note.links.length === 0
                  ? "—"
                  : note.links.map((l) => `${l.entity_type}:${l.entity_id.slice(0, 6)}`).join(", ")}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
