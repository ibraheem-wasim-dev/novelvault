import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import type {
  AiChatMessage,
  AiMemory,
  AiSource,
  AiStatus,
  AiSuggestion,
} from "../types";

interface Props {
  onChanged: () => void;
  onError: (message: string) => void;
}

type Tab = "ask" | "memory" | "notices" | "setup";

const STATUS_MARK: Record<string, string> = {
  CURRENT: "🟢",
  WORKING_DRAFT: "🔵",
  REFERENCE: "🟡",
  HISTORICAL: "🟠",
  REJECTED: "🔴",
  UNKNOWN: "⚫",
};

/**
 * The local librarian. It reads the novel, answers with numbered sources ranked by
 * authority, and notices things — but it never edits a file and never decides canon.
 * Everything it notices waits on the Notices tab for a person to accept or dismiss.
 */
export default function AiPage({ onChanged, onError }: Props) {
  const [tab, setTab] = useState<Tab>("ask");
  const [status, setStatus] = useState<AiStatus | null>(null);

  const loadStatus = useCallback(async () => {
    try {
      setStatus(await api.aiStatus());
    } catch (e) {
      onError(String(e));
    }
  }, [onError]);

  useEffect(() => {
    void loadStatus();
  }, [loadStatus]);

  return (
    <section>
      <h2>Librarian</h2>
      <p className="muted small">
        A local assistant that knows your novel. It answers from your canon and files, shows its
        sources, and warns when they are old or outdated. It only suggests — you decide.
      </p>

      <div className="ai-mode">
        {status?.mode === "model" ? (
          <span className="chip ai-on">🧠 Local model: {status.config.model}</span>
        ) : (
          <span className="chip">📚 Offline librarian (no model connected)</span>
        )}
        {status && (
          <span className="muted small">
            {status.indexed_files} files · {status.indexed_passages} passages indexed
            {status.pending_files > 0 ? ` · ${status.pending_files} waiting` : ""}
          </span>
        )}
      </div>

      <div className="tabs">
        {(
          [
            ["ask", "💬 Ask"],
            ["memory", "🧠 Novel memory"],
            ["notices", "🔔 Notices"],
            ["setup", "⚙ Local AI"],
          ] as Array<[Tab, string]>
        ).map(([key, label]) => (
          <button key={key} className={tab === key ? "nav active" : "nav"} onClick={() => setTab(key)}>
            {label}
          </button>
        ))}
      </div>

      {tab === "ask" && <AskTab onError={onError} onAsked={loadStatus} />}
      {tab === "memory" && <MemoryTab status={status} onError={onError} />}
      {tab === "notices" && <NoticesTab onChanged={onChanged} onError={onError} />}
      {tab === "setup" && <SetupTab status={status} onChanged={loadStatus} onError={onError} />}
    </section>
  );
}

// ---------------------------------------------------------------------------

function SourceCard({ source, cited }: { source: AiSource; cited: boolean }) {
  const mark = source.kind === "canon" ? "⚖️" : source.kind === "understanding" ? "🧠" : STATUS_MARK[source.status] ?? "⚫";
  return (
    <div className={`ai-source ${source.warning ? "has-warning" : ""} ${cited ? "cited" : ""}`}>
      <div className="ai-source-head">
        <span className="badge">{source.label}</span>
        <span>
          {mark} <strong>{source.title}</strong>
        </span>
        <span className="muted small">
          {source.status} · {source.authority}
        </span>
        {cited && <span className="chip">cited</span>}
      </div>
      <div className="ai-source-text">{source.text}</div>
      {source.detail && <div className="muted tiny">{source.detail}</div>}
      {source.warning && <div className="ai-warn">⚠ {source.warning}</div>}
    </div>
  );
}

function AskTab({ onError, onAsked }: { onError: (m: string) => void; onAsked: () => void }) {
  const [messages, setMessages] = useState<AiChatMessage[]>([]);
  const [question, setQuestion] = useState("");
  const [busy, setBusy] = useState(false);
  const [warnings, setWarnings] = useState<string[]>([]);
  const [cited, setCited] = useState<string[]>([]);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    api.aiHistory(40).then(setMessages).catch((e) => onError(String(e)));
  }, [onError]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, busy]);

  async function send() {
    const q = question.trim();
    if (!q || busy) return;
    setBusy(true);
    setQuestion("");
    setMessages((m) => [
      ...m,
      { id: `tmp-${Date.now()}`, role: "user", content: q, mode: null, sources: [], created_at: "" },
    ]);
    try {
      const a = await api.aiAsk(q);
      setWarnings(a.warnings);
      setCited(a.cited);
      setMessages(await api.aiHistory(40));
      onAsked();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const lastAssistant = [...messages].reverse().find((m) => m.role === "assistant");

  return (
    <div className="ai-chat">
      <div className="ai-thread">
        {messages.length === 0 && (
          <div className="muted">
            <p>Try asking:</p>
            <ul>
              <li>Where did we establish the wooden bird?</li>
              <li>What does the current canon say about Maheen?</li>
              <li>Which file contains the old brother storyline?</li>
            </ul>
          </div>
        )}
        {messages.map((m) => (
          <div key={m.id} className={m.role === "user" ? "ai-msg user" : "ai-msg assistant"}>
            <div className="ai-msg-text">{m.content}</div>
          </div>
        ))}
        {busy && <div className="muted small">Reading your novel…</div>}
        <div ref={endRef} />
      </div>

      {warnings.length > 0 && (
        <div className="ai-warnings">
          {warnings.map((w) => (
            <div key={w}>⚠ {w}</div>
          ))}
        </div>
      )}

      {lastAssistant && lastAssistant.sources.length > 0 && (
        <details open className="ai-sources">
          <summary>
            Sources ({lastAssistant.sources.length}) — canon first, then current files, then older drafts
          </summary>
          {lastAssistant.sources.map((s) => (
            <SourceCard key={s.label} source={s} cited={cited.includes(s.label)} />
          ))}
        </details>
      )}

      <div className="ai-input">
        <input
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void send()}
          placeholder="Ask about your novel…"
          aria-label="Ask the librarian"
          disabled={busy}
        />
        <button className="primary" onClick={() => void send()} disabled={busy || !question.trim()}>
          Ask
        </button>
        <button
          className="ghost"
          disabled={busy || messages.length === 0}
          onClick={() =>
            void api
              .aiClearHistory()
              .then(() => {
                setMessages([]);
                setWarnings([]);
              })
              .catch((e) => onError(String(e)))
          }
        >
          Clear chat
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------

function MemoryTab({ status, onError }: { status: AiStatus | null; onError: (m: string) => void }) {
  const [memory, setMemory] = useState<AiMemory | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(() => {
    api.aiMemory().then(setMemory).catch((e) => onError(String(e)));
  }, [onError]);

  useEffect(load, [load]);

  async function summarize() {
    setBusy(true);
    setNote(null);
    try {
      const r = await api.aiSummarize(3);
      setNote(r.summarized ? `Summarised ${r.summarized} file(s) with the local model.` : "Nothing left to summarise.");
      load();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  if (!memory) return <p className="muted">Reading the novel…</p>;
  return (
    <div>
      <p className="muted small">
        What the librarian has learned from your files as the novel grows. Re-built from your
        folder at any time — it is never the source of truth.
      </p>
      <div className="controls">
        <button onClick={summarize} disabled={busy || status?.mode !== "model"}>
          {busy ? "Summarising…" : "Summarise live files with local model"}
        </button>
        <button className="ghost" onClick={load}>Refresh</button>
        {status?.mode !== "model" && (
          <span className="muted small">Connect a local model (Local AI tab) for written summaries.</span>
        )}
      </div>
      {note && <p className="muted small">{note}</p>}

      <h3>Characters, places and objects</h3>
      {memory.entities.length === 0 ? (
        <p className="muted">None recorded yet. Add them on the Story page, or accept a suggestion from Notices.</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Kind</th>
              <th>In live files</th>
              <th>Only in older files</th>
            </tr>
          </thead>
          <tbody>
            {memory.entities.map((e) => (
              <tr key={`${e.kind}-${e.name}`}>
                <td>{e.name}</td>
                <td className="muted">{e.kind}</td>
                <td>{e.passages_live}</td>
                <td>
                  {e.passages_older}
                  {e.passages_live === 0 && e.passages_older > 0 && <span className="ai-warn"> ⚠ not in any live file</span>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <h3>Files</h3>
      <div className="ai-files">
        {memory.files.map((f) => (
          <div key={f.version_id} className="ai-source">
            <div className="ai-source-head">
              <span>{STATUS_MARK[f.file_status] ?? "⚫"}</span>
              <strong>{f.rel_path}</strong>
              <span className="muted small">
                v{f.version_number} · {f.file_status}
                {f.arc_label ? ` · ${f.arc_label}` : ""}
                {f.chapter_label ? ` · ${f.chapter_label}` : ""}
              </span>
              <span className="chip">{f.method === "model" ? "model summary" : "opening lines"}</span>
            </div>
            <div className="ai-source-text">{f.digest}</div>
          </div>
        ))}
        {memory.files.length === 0 && <p className="muted">No readable files yet.</p>}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------

function NoticesTab({ onChanged, onError }: { onChanged: () => void; onError: (m: string) => void }) {
  const [items, setItems] = useState<AiSuggestion[]>([]);
  const [busy, setBusy] = useState(false);
  const [kinds, setKinds] = useState<Record<string, string>>({});

  const load = useCallback(() => {
    api.aiSuggestions("pending").then(setItems).catch((e) => onError(String(e)));
  }, [onError]);

  useEffect(load, [load]);

  async function scan() {
    setBusy(true);
    try {
      await api.aiScanSuggestions();
      load();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function decide(s: AiSuggestion, decision: "accepted" | "dismissed") {
    try {
      await api.aiDecideSuggestion(s.id, decision, undefined, kinds[s.id] ?? "character");
      load();
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  return (
    <div>
      <p className="muted small">
        Things the librarian noticed. Nothing here is applied automatically — accepting creates a
        story record (never canon); dismissing is remembered so it does not come back.
      </p>
      <div className="controls">
        <button className="primary" onClick={scan} disabled={busy}>
          {busy ? "Looking…" : "Look for new things"}
        </button>
      </div>
      {items.length === 0 && <p className="muted">Nothing waiting. Run “Look for new things” after adding drafts.</p>}
      {items.map((s) => (
        <div key={s.id} className="card">
          <div className="card-head">
            <span className="badge">{s.kind === "new_entity" ? "possible new name" : s.kind === "story_change" ? "drafts disagree" : "unreviewed idea"}</span>
            <strong>{s.title}</strong>
          </div>
          <p className="small">{s.detail}</p>
          {s.evidence.length > 0 && <p className="muted tiny">Seen in: {s.evidence.join(", ")}</p>}
          <div className="card-actions">
            {s.kind === "new_entity" && (
              <select
                value={kinds[s.id] ?? "character"}
                onChange={(e) => setKinds({ ...kinds, [s.id]: e.target.value })}
                aria-label="Record as"
              >
                <option value="character">as a character</option>
                <option value="location">as a location</option>
                <option value="object">as an object</option>
              </select>
            )}
            <button className="primary" onClick={() => void decide(s, "accepted")}>
              {s.kind === "new_entity" ? "Add to story" : "Mark reviewed"}
            </button>
            <button onClick={() => void decide(s, "dismissed")}>Dismiss</button>
          </div>
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------

function SetupTab({
  status,
  onChanged,
  onError,
}: {
  status: AiStatus | null;
  onChanged: () => void;
  onError: (m: string) => void;
}) {
  const [provider, setProvider] = useState("ollama");
  const [model, setModel] = useState("");
  const [port, setPort] = useState(11434);
  const [saved, setSaved] = useState<string | null>(null);

  useEffect(() => {
    if (status) {
      setProvider(status.config.provider === "none" ? "ollama" : status.config.provider);
      setModel(status.config.model);
      setPort(status.config.port);
    }
  }, [status]);

  async function save(p: string = provider) {
    try {
      const s = await api.aiConfigure(p, p === "none" ? "" : model, "127.0.0.1", port);
      setSaved(s.note);
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  return (
    <div>
      <p className="muted small">
        Your novel never leaves this computer. NovelVault only connects to an AI running on{" "}
        <code>localhost</code> — any other address is refused.
      </p>

      <div className="card">
        <div className="card-head">
          <strong>Status</strong>
        </div>
        <p className="small">{status?.note ?? "…"}</p>
      </div>

      <div className="card">
        <div className="card-head">
          <strong>Connect a local model</strong>
        </div>
        <ol className="small">
          <li>
            Install <a href="https://ollama.com" target="_blank" rel="noreferrer">Ollama</a> (free, runs on your PC).
          </li>
          <li>
            Open a terminal and download a model, e.g. <code>ollama pull llama3.1:8b</code>. Larger
            models write better answers; smaller ones are faster.
          </li>
          <li>Come back here, pick it below and press Save.</li>
        </ol>
        <div className="controls">
          <select
            value={provider}
            onChange={(e) => {
              setProvider(e.target.value);
              setPort(e.target.value === "openai_local" ? 8080 : 11434);
            }}
            aria-label="Local AI program"
          >
            <option value="ollama">Ollama</option>
            <option value="openai_local">llama.cpp / LM Studio (OpenAI-compatible)</option>
          </select>
          <input value={port} type="number" onChange={(e) => setPort(Number(e.target.value))} aria-label="Port" style={{ width: 90 }} />
          {status && status.models.length > 0 ? (
            <select value={model} onChange={(e) => setModel(e.target.value)} aria-label="Model">
              <option value="">Choose a model…</option>
              {status.models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          ) : (
            <input value={model} onChange={(e) => setModel(e.target.value)} placeholder="model name, e.g. llama3.1:8b" aria-label="Model name" />
          )}
          <button className="primary" onClick={() => void save()} disabled={!model.trim()}>
            Save
          </button>
          <button className="ghost" onClick={() => void save("none")}>
            Use offline librarian only
          </button>
        </div>
        {saved && <p className="muted small">{saved}</p>}
      </div>
    </div>
  );
}
