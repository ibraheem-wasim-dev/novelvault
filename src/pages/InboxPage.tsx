import { useEffect, useState } from "react";
import { api } from "../api";
import { FILE_STATUSES, statusTone, type FileVersion, type InboxItem } from "../types";

interface Props {
  onChanged: () => void;
  onError: (message: string) => void;
}

export default function InboxPage({ onChanged, onError }: Props) {
  const [items, setItems] = useState<InboxItem[]>([]);
  const [versions, setVersions] = useState<Record<string, FileVersion>>({});
  const [showResolved, setShowResolved] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  async function load() {
    try {
      const list = await api.inbox(showResolved);
      setItems(list);
      const all = await api.versions(true);
      const map: Record<string, FileVersion> = {};
      for (const v of all) map[v.id] = v;
      setVersions(map);
    } catch (e) {
      onError(String(e));
    }
  }

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showResolved]);

  async function act(item: InboxItem, action: string, fields: Record<string, string> = {}) {
    setBusy(item.id);
    try {
      await api.resolveInbox(item.id, action, fields);
      await load();
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section>
      <div className="section-head">
        <h2>Inbox</h2>
        <label className="toggle">
          <input
            type="checkbox"
            checked={showResolved}
            onChange={(e) => setShowResolved(e.target.checked)}
          />
          Show resolved
        </label>
      </div>
      <p className="muted small">
        Nothing enters the structured novel directly. Everything the scanner or the watcher finds
        waits here until you decide.
      </p>

      {items.length === 0 && <p className="muted">Nothing waiting.</p>}

      {items.map((item) => {
        const version = item.file_version_id ? versions[item.file_version_id] : undefined;
        const candidate = item.candidate_version_id ? versions[item.candidate_version_id] : undefined;
        const open = item.status === "OPEN";
        return (
          <article className={`card ${open ? "" : "card-resolved"}`} key={item.id}>
            <header className="card-head">
              <span className="badge">{item.kind}</span>
              <strong>{item.title}</strong>
              {!open && <span className="badge">{item.status}</span>}
            </header>

            {item.detail && <p className="muted small">{item.detail}</p>}

            {version && (
              <div className="kv">
                <span>Version</span>
                <span className={`badge ${statusTone(version.file_status)}`}>
                  {version.status_marker} {version.file_status}
                </span>
                <span>Family</span>
                <span>{version.family_name} · v{version.version_number}</span>
                <span>Extraction</span>
                <span>{version.extraction_state}</span>
              </div>
            )}

            {candidate && (
              <div className="kv">
                <span>Relates to</span>
                <span>
                  {candidate.family_name} · v{candidate.version_number}
                  {candidate.is_current ? " (current)" : ""}
                </span>
              </div>
            )}

            {open && (
              <div className="card-actions">
                {version && (
                  <select
                    defaultValue=""
                    disabled={busy !== null}
                    onChange={(e) => {
                      const status = e.target.value;
                      if (!status) return;
                      void act(item, "classify", { status });
                    }}
                  >
                    <option value="">Classify as…</option>
                    {FILE_STATUSES.map((s) => (
                      <option key={s} value={s}>{s}</option>
                    ))}
                  </select>
                )}

                {item.kind === "POSSIBLE_RENAME" && (
                  <button disabled={busy !== null} onClick={() => void act(item, "relink")}>
                    Relink to this path
                  </button>
                )}

                {item.kind === "FAMILY_PROPOSAL" && version && candidate && (
                  <button
                    disabled={busy !== null}
                    onClick={() =>
                      void (async () => {
                        await api.mergeFamily(version.file_id, candidate.file_id);
                        await act(item, "keep_separate");
                      })()
                    }
                  >
                    Merge into one family
                  </button>
                )}

                {(item.kind === "NEW_VERSION" || item.kind === "FAMILY_PROPOSAL") && (
                  <>
                    <button
                      className="primary small-btn"
                      disabled={busy !== null}
                      onClick={() => void act(item, "accept_change")}
                    >
                      Make this CURRENT
                    </button>
                    <button
                      disabled={busy !== null}
                      onClick={() => void act(item, "reject_change")}
                    >
                      Keep previous
                    </button>
                  </>
                )}

                <button disabled={busy !== null} onClick={() => void act(item, "keep_separate")}>
                  Keep separate
                </button>
                <button className="ghost" disabled={busy !== null} onClick={() => void act(item, "ignore")}>
                  Ignore
                </button>
              </div>
            )}

            {!open && item.resolved_action && (
              <p className="muted tiny">Resolved by “{item.resolved_action}” at {item.resolved_at}</p>
            )}
          </article>
        );
      })}
    </section>
  );
}
