import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import AddFilesPanel from "../components/AddFilesPanel";
import type { NovelSummary, ScanSummary } from "../types";

/**
 * The first screen. Two ways in, both by browsing:
 *
 *  * create a novel — choose where it lives, name it, and the folder plus its
 *    `.novelvault/` index are made;
 *  * open one that already exists — point at the folder, and the index is rebuilt by
 *    scanning, because the files are the source of truth.
 */
export default function SetupPage({ onOpened }: { onOpened: () => void }) {
  const [tab, setTab] = useState<"create" | "open">("create");
  const [parent, setParent] = useState<string | null>(null);
  const [name, setName] = useState("Saqar");
  const [created, setCreated] = useState<string | null>(null);
  const [progress, setProgress] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [library, setLibrary] = useState<NovelSummary[]>([]);

  const loadLibrary = useCallback(() => {
    api.libraryList().then(setLibrary).catch(() => setLibrary([]));
  }, []);

  useEffect(loadLibrary, [loadLibrary]);

  async function openFromLibrary(path: string) {
    try {
      setBusy(true);
      setError(null);
      setProgress("Opening…");
      await api.openNovel(path);
      onOpened();
    } catch (e) {
      setError(String(e));
      setProgress(null);
    } finally {
      setBusy(false);
    }
  }

  async function forget(path: string) {
    try {
      await api.libraryForget(path);
      loadLibrary();
    } catch (e) {
      setError(String(e));
    }
  }

  async function chooseParent() {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Where should the novel folder live?",
    });
    if (typeof picked === "string") setParent(picked);
  }

  async function createNow() {
    if (!parent) {
      setError("Choose the folder the novel folder should be created in.");
      return;
    }
    if (!name.trim()) {
      setError("Give the novel a name.");
      return;
    }
    try {
      setBusy(true);
      setError(null);
      const result = await api.createNovel(parent, name.trim(), name.trim());
      setCreated(result.root);
      setProgress(`Folder ready at ${result.root}. Now bring your drafts in.`);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function openExisting() {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Select the novel folder",
    });
    if (typeof picked !== "string") return;
    try {
      setBusy(true);
      setError(null);
      setProgress("Opening and scanning the folder…");
      await api.openNovel(picked);
      const summary: ScanSummary = await api.scan();
      setProgress(summary.headline);
      onOpened();
    } catch (e) {
      setError(String(e));
      setProgress(null);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="welcome">
      <div className="welcome-card wide">
        <h1>NovelVault</h1>
        <p className="muted">
          A local-first vault for a novel that lives as files. It watches the folder, keeps every
          version, and holds the canon you have decided on — it never rewrites your drafts.
        </p>

        {library.length > 0 && (
          <div className="library">
            <h3>Your novels</h3>
            {library.map((n) => (
              <div key={n.path} className={n.exists ? "library-row" : "library-row missing"}>
                <div>
                  <strong>{n.title}</strong>
                  <div className="muted tiny mono" title={n.path}>{n.path}</div>
                  {n.exists ? (
                    <div className="muted small">
                      {n.files} files · {n.current_files} current · {n.chapters} chapters · {n.characters} characters ·{" "}
                      {n.canon_items} canon
                      {n.open_conflicts > 0 ? ` · ⚠ ${n.open_conflicts} conflicts` : ""}
                      {n.pending_ai_suggestions > 0 ? ` · 🔔 ${n.pending_ai_suggestions} notices` : ""}
                    </div>
                  ) : (
                    <div className="ai-warn">Folder not found — it may have been moved. Use “Open an existing folder” to re-find it.</div>
                  )}
                </div>
                <div className="card-actions">
                  <button className="primary" disabled={busy || !n.exists} onClick={() => void openFromLibrary(n.path)}>
                    Open
                  </button>
                  <button className="ghost" title="Removes it from this list only. The folder is not touched." onClick={() => void forget(n.path)}>
                    Remove from list
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}

        <div className="tabs">
          <button className={tab === "create" ? "nav active" : "nav"} onClick={() => setTab("create")}>
            ＋ Create a novel
          </button>
          <button className={tab === "open" ? "nav active" : "nav"} onClick={() => setTab("open")}>
            📂 Open an existing folder
          </button>
        </div>

        {tab === "create" && (
          <div className="setup-body">
            <ol className="welcome-list">
              <li>Choose where the novel folder should live.</li>
              <li>Name it — the folder and its <code>.novelvault/</code> index are created.</li>
              <li>Browse your drafts in. They are indexed; nothing becomes canon by itself.</li>
            </ol>

            <div className="controls">
              <button onClick={chooseParent} disabled={busy}>
                {parent ? "Change location…" : "Choose location…"}
              </button>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Novel name, e.g. Saqar"
                aria-label="Novel name"
              />
              <button className="primary" onClick={createNow} disabled={busy || !parent}>
                Create novel folder
              </button>
            </div>

            {parent && (
              <p className="muted small mono">
                will create {parent.endsWith("/") ? parent : `${parent}/`}
                {name.trim() || "…"}
              </p>
            )}

            {created && (
              <AddFilesPanel onDone={() => onOpened()} onError={setError} />
            )}
          </div>
        )}

        {tab === "open" && (
          <div className="setup-body">
            <ul className="welcome-list">
              <li>Point at a folder you already have. Nothing in it is moved or renamed.</li>
              <li>The index is rebuilt by scanning, so the folder is portable between machines.</li>
            </ul>
            <button className="primary" onClick={openExisting} disabled={busy}>
              Browse for the folder…
            </button>
          </div>
        )}

        {progress && <p className="muted small">{progress}</p>}
        {error && <p className="error">{error}</p>}
      </div>
    </div>
  );
}
