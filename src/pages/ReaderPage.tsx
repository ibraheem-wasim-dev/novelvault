import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { ReaderProfile, ReaderStep, ReaderUnderstanding } from "../types";

interface Props {
  onChanged: () => void;
  onError: (message: string) => void;
}

type Tab = "story" | "cast" | "timeline" | "threads" | "changes";

const KIND_MARK: Record<string, string> = {
  character: "🧑",
  location: "📍",
  object: "🔹",
  unknown: "❔",
};

/**
 * Story Brain — what the Reader has understood by reading the drafts in story order.
 * Everything here is derived from the author's files and points back to them. None of it
 * is canon: it is a reading, and the author decides what it means.
 */
export default function ReaderPage({ onChanged, onError }: Props) {
  const [tab, setTab] = useState<Tab>("story");
  const [data, setData] = useState<ReaderUnderstanding | null>(null);
  const [log, setLog] = useState<ReaderStep[]>([]);
  const [reading, setReading] = useState(false);
  const [modelOn, setModelOn] = useState(false);
  const [deepening, setDeepening] = useState(false);
  const [profile, setProfile] = useState<ReaderProfile | null>(null);
  const alive = useRef(true);

  const load = useCallback(async () => {
    try {
      setData(await api.readerUnderstanding());
    } catch (e) {
      onError(String(e));
    }
  }, [onError]);

  /** Read everything unread, a few files at a time, so the page fills in as the Reader learns. */
  const readAll = useCallback(async () => {
    setReading(true);
    try {
      for (let guard = 0; guard < 200 && alive.current; guard++) {
        const r = await api.readerCrawl(5);
        if (r.steps.length) setLog((old) => [...r.steps.slice().reverse(), ...old].slice(0, 40));
        await load();
        if (r.remaining === 0 || r.files_read === 0) {
          if (r.new_notices > 0) onChanged();
          break;
        }
      }
    } catch (e) {
      onError(String(e));
    } finally {
      if (alive.current) setReading(false);
    }
  }, [load, onChanged, onError]);

  useEffect(() => {
    alive.current = true;
    void (async () => {
      await load();
      try {
        setModelOn((await api.aiStatus()).mode === "model");
      } catch {
        /* the model is optional */
      }
      await readAll();
    })();
    return () => {
      alive.current = false;
    };
  }, [load, readAll]);

  async function deepen() {
    setDeepening(true);
    try {
      await api.readerDeepen(2);
      await load();
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setDeepening(false);
    }
  }

  async function open(name: string) {
    try {
      setProfile(await api.readerEntity(name));
    } catch (e) {
      onError(String(e));
    }
  }

  const cov = data?.coverage;
  const pct = cov && cov.files_total > 0 ? Math.round((cov.files_read / cov.files_total) * 100) : 0;

  return (
    <section>
      <h2>Story Brain</h2>
      <p className="muted small">
        The Reader reads your drafts one by one, in story order, and learns the plot as it goes — who is
        in it, what is said about them, what happens, and what is left open. It learns only from your own
        files, shows where each thing came from, and never changes a file or decides canon.
      </p>

      <div className="reader-progress card">
        <div className="reader-bar" aria-label="reading progress">
          <div className="reader-bar-fill" style={{ width: `${pct}%` }} />
        </div>
        <div className="reader-stats small">
          {cov ? (
            <>
              <strong>
                {cov.files_read} of {cov.files_total}
              </strong>{" "}
              files read · {cov.sentences_read.toLocaleString()} sentences
              {cov.files_deepened > 0 && <> · {cov.files_deepened} read deeply by your local model</>}
            </>
          ) : (
            "Opening…"
          )}
        </div>
        <div className="reader-actions">
          <button onClick={() => void readAll()} disabled={reading}>
            {reading ? "Reading…" : "Read new drafts now"}
          </button>
          <button
            onClick={() => void deepen()}
            disabled={!modelOn || deepening}
            title={modelOn ? "Re-read live files with your local model; claims not found in the text are discarded" : "Connect a local model on the Librarian page first"}
          >
            {deepening ? "Reading deeply…" : "🧠 Read deeper with local model"}
          </button>
        </div>
        {log.length > 0 && (
          <details className="reader-log" open={reading}>
            <summary className="muted small">Reading log</summary>
            {log.map((s) => (
              <div key={s.version_id + s.rel_path} className="small">
                📖 {s.rel_path} — {s.facts} facts, {s.events} events, {s.threads} open questions
                {s.learned.length > 0 && <span className="reader-learned"> · learned {s.learned.join(", ")}</span>}
              </div>
            ))}
          </details>
        )}
      </div>

      <div className="tabs">
        {(
          [
            ["story", "Story so far"],
            ["cast", `Cast & places${data ? ` (${data.entities.length})` : ""}`],
            ["timeline", "Timeline"],
            ["threads", `Open threads${data ? ` (${data.threads.length})` : ""}`],
            ["changes", `Drafts disagree${data && data.changes.length ? ` (${data.changes.length})` : ""}`],
          ] as Array<[Tab, string]>
        ).map(([key, label]) => (
          <button key={key} className={tab === key ? "nav active" : "nav"} onClick={() => setTab(key)}>
            {label}
          </button>
        ))}
      </div>

      {!data && <p className="muted">Loading…</p>}
      {data && data.coverage.files_total === 0 && (
        <p className="muted">No readable files yet. Bring drafts in and the Reader will start learning.</p>
      )}

      {data && tab === "story" && (
        <div>
          {data.story_so_far ? (
            <>
              <p className="muted tiny">
                {data.story_so_far_method === "model"
                  ? "Written by your local model from its reading of your current files."
                  : "Assembled from the key moments the Reader found, chapter by chapter."}
              </p>
              <div className="reader-story">{data.story_so_far}</div>
            </>
          ) : (
            <p className="muted">Mark a draft as Current or Working draft and the Reader will tell its story.</p>
          )}
          <h3>Chapter by chapter</h3>
          {data.chapters.map((c) => (
            <div key={c.version_id} className={c.is_live ? "card" : "card reader-old"}>
              <div className="card-head">
                <strong>{c.label}</strong>
                <span className="badge">{c.is_live ? "live" : "older draft"}</span>
                {c.method === "model" && <span className="badge">model</span>}
              </div>
              <div className="muted tiny">{c.rel_path}</div>
              <p className="small">{c.summary || "Nothing eventful found yet."}</p>
            </div>
          ))}
        </div>
      )}

      {data && tab === "cast" && (
        <div className="reader-cast">
          <div className="reader-cast-list">
            {data.entities.length === 0 && <p className="muted">No names learned yet.</p>}
            {data.entities.map((e) => (
              <button key={e.name} className={profile?.name === e.name ? "reader-ent on" : "reader-ent"} onClick={() => void open(e.name)}>
                <span>{KIND_MARK[e.kind] ?? "❔"}</span>
                <span className="reader-ent-name">{e.name}</span>
                <span className="muted tiny">
                  {e.mentions_live} live{e.mentions_older > 0 ? ` · ${e.mentions_older} older` : ""}
                  {!e.recorded && " · not in your story records"}
                </span>
              </button>
            ))}
          </div>
          <div className="reader-profile">
            {!profile && <p className="muted">Pick a name to see everything the Reader has learned about it.</p>}
            {profile && (
              <>
                <h3 style={{ marginTop: 0 }}>
                  {KIND_MARK[profile.kind] ?? "❔"} {profile.name}
                </h3>
                <p className="small">{profile.summary}</p>
                {profile.related.length > 0 && (
                  <p className="muted small">
                    Often appears with:{" "}
                    {profile.related.map(([n, c]) => (
                      <button key={n} className="ghost tiny-btn" onClick={() => void open(n)}>
                        {n} ({c})
                      </button>
                    ))}
                  </p>
                )}
                {profile.facts.map((f, i) => (
                  <div key={i} className={f.is_live ? "ai-source" : "ai-source has-warning"}>
                    <div>
                      <strong>{f.predicate}</strong> {f.object}
                      {f.inferred && <span className="muted tiny"> (“he/she” guessed)</span>}
                      {f.source === "model" && <span className="badge"> model</span>}
                    </div>
                    <div className="muted tiny">“{f.excerpt}”</div>
                    <div className="muted tiny">
                      {f.rel_path} {f.is_live ? "" : "— older draft, treat as history"}
                    </div>
                  </div>
                ))}
                {profile.facts.length === 0 && <p className="muted small">No stated facts yet — only appearances.</p>}
              </>
            )}
          </div>
        </div>
      )}

      {data && tab === "timeline" && (
        <div>
          <p className="muted tiny">Key moments in the order your files tell them. Older drafts are shown dimmed.</p>
          {data.timeline.length === 0 && <p className="muted">No events found yet.</p>}
          {data.timeline.map((e, i) => (
            <div key={i} className={e.is_live ? "reader-event" : "reader-event reader-old"}>
              <div className="muted tiny">
                {e.label} · {e.rel_path}
                {e.source === "model" && " · model"}
              </div>
              <div className="small">{e.summary}</div>
            </div>
          ))}
        </div>
      )}

      {data && tab === "threads" && (
        <div>
          <p className="muted tiny">
            Places where the narration points at something unresolved: a secret, a promise, a question. These are
            possible threads, not a verdict — you know which ones you meant.
          </p>
          {data.threads.length === 0 && <p className="muted">Nothing left open that the Reader could spot.</p>}
          {data.threads.map((t, i) => (
            <div key={i} className={t.is_live ? "card" : "card reader-old"}>
              <div className="small">“{t.excerpt}”</div>
              <div className="muted tiny">
                {t.rel_path} · cue: {t.cue}
                {t.entities.length > 0 && ` · ${t.entities.join(", ")}`}
              </div>
            </div>
          ))}
        </div>
      )}

      {data && tab === "changes" && (
        <div>
          <p className="muted tiny">
            Where one draft says something different from another. The Reader only points this out — it never picks a
            side. Each one also waits as a notice on the Librarian page.
          </p>
          {data.changes.length === 0 && <p className="muted">No disagreements noticed.</p>}
          {data.changes.map((c, i) => (
            <div key={i} className="card">
              <div className="card-head">
                <span className="badge">{c.kind === "revision" ? "revised since an older draft" : "two live files disagree"}</span>
                <strong>
                  {c.subject} {c.predicate}
                </strong>
              </div>
              <div className="small">
                <span className="muted">then</span> “{c.old_value}” <span className="muted tiny">({c.old_file})</span>
              </div>
              <div className="small">
                <span className="muted">now</span> “{c.new_value}” <span className="muted tiny">({c.new_file})</span>
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
