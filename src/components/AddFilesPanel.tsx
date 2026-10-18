import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { AddOutcome, AddPlan } from "../types";

/**
 * Bringing drafts in by browsing.
 *
 * The panel never writes anything on its own. Browsing produces a plan, the plan is
 * shown in full, and only the confirm button applies it — the core then writes each
 * file to a temporary name, verifies the sha256, and only then lets it exist under its
 * real name. An existing file is never overwritten.
 */
export default function AddFilesPanel({
  onDone,
  onError,
}: {
  onDone: () => void;
  onError: (message: string) => void;
}) {
  const [plan, setPlan] = useState<AddPlan | null>(null);
  const [paths, setPaths] = useState<string[]>([]);
  const [moveFiles, setMoveFiles] = useState(false);
  const [outcomes, setOutcomes] = useState<AddOutcome[] | null>(null);
  const [busy, setBusy] = useState(false);

  async function planFor(list: string[], moving: boolean) {
    try {
      setBusy(true);
      setOutcomes(null);
      const planned = await api.planAddFiles(list, moving);
      setPaths(list);
      setPlan(planned);
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function browseFiles() {
    const picked = await open({
      multiple: true,
      title: "Choose draft files to bring into the novel",
    });
    if (!picked) return;
    const list = Array.isArray(picked) ? picked : [picked];
    await planFor(list, moveFiles);
  }

  async function browseFolder() {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Choose a folder of drafts",
    });
    if (typeof picked !== "string") return;
    await planFor([picked], moveFiles);
  }

  async function applyNow() {
    try {
      setBusy(true);
      const result = await api.applyAddFiles(paths, moveFiles, true);
      setOutcomes(result);
      setPlan(null);
      onDone();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function toggleMove(next: boolean) {
    setMoveFiles(next);
    if (paths.length) await planFor(paths, next);
  }

  return (
    <section className="add-panel">
      <h3>Bring files in</h3>
      <p className="muted small">
        Pick the drafts with the browser. They are copied or moved into this novel's folder and
        indexed — never overwritten, and never made CURRENT on their own.
      </p>

      <div className="controls">
        <button onClick={browseFiles} disabled={busy}>
          🗂 Browse files…
        </button>
        <button onClick={browseFolder} disabled={busy}>
          📁 Browse a folder…
        </button>
        <label className="toggle">
          <input
            type="checkbox"
            checked={moveFiles}
            onChange={(e) => void toggleMove(e.target.checked)}
          />
          Move instead of copy
        </label>
      </div>

      {busy && <p className="muted small">Working…</p>}

      {plan && (
        <div className="plan">
          <div className="plan-head">
            <strong>{plan.headline}</strong>
            <span className="muted tiny mono">→ {plan.novel_root}</span>
          </div>

          {plan.entries.length > 0 && (
            <table className="table files">
              <thead>
                <tr>
                  <th>From</th>
                  <th>Lands as</th>
                  <th>Size</th>
                  <th>Action</th>
                </tr>
              </thead>
              <tbody>
                {plan.entries.map((entry) => (
                  <tr key={entry.source_path}>
                    <td className="mono small">{entry.source_path}</td>
                    <td className="mono small">{entry.rel_target}</td>
                    <td className="small">{Math.max(1, Math.round(entry.size_bytes / 1024))} KB</td>
                    <td>
                      <span className="badge">{entry.action}</span>
                      {entry.note && <div className="muted tiny">{entry.note}</div>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          {plan.skipped.length > 0 && (
            <>
              <h4>Skipped</h4>
              <ul className="muted small">
                {plan.skipped.map((entry) => (
                  <li key={entry.source_path}>
                    <span className="mono">{entry.source_path}</span> — {entry.note}
                  </li>
                ))}
              </ul>
            </>
          )}

          {plan.entries.length > 0 && (
            <div className="controls">
              <button className="primary" onClick={applyNow} disabled={busy}>
                {moveFiles ? `Move ${plan.entries.length} file(s)` : `Copy ${plan.entries.length} file(s)`}
              </button>
              <button className="ghost" onClick={() => setPlan(null)} disabled={busy}>
                Cancel — change nothing
              </button>
            </div>
          )}
        </div>
      )}

      {outcomes && (
        <div className="plan">
          <strong>Done</strong>
          <ul className="small">
            {outcomes.map((outcome) => (
              <li key={outcome.source_path}>
                {outcome.error ? (
                  <span className="error">
                    {outcome.source_path}: {outcome.error}
                  </span>
                ) : (
                  <>
                    <span className="mono">{outcome.rel_target}</span> — copy verified
                    {outcome.source_removed ? ", original removed" : ", original left in place"}. It is
                    UNKNOWN and waiting in the Inbox.
                  </>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
