import { useEffect, useState } from "react";
import { api } from "../api";
import { FILE_STATUSES, statusTone, type FileVersion } from "../types";

interface Props {
  includeMissing: boolean;
  showHistory?: boolean;
  onChanged: () => void;
  onError: (message: string) => void;
}

export default function FilesPage({ includeMissing, showHistory, onChanged, onError }: Props) {
  const [versions, setVersions] = useState<FileVersion[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState("ALL");

  async function load() {
    try {
      setVersions(await api.versions(includeMissing));
    } catch (e) {
      onError(String(e));
    }
  }

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [includeMissing]);

  async function act(label: string, fn: () => Promise<unknown>) {
    setBusy(label);
    try {
      await fn();
      await load();
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(null);
    }
  }

  const shown = versions.filter((v) => {
    const matchesText =
      filter.trim() === "" ||
      v.rel_path.toLowerCase().includes(filter.toLowerCase()) ||
      v.family_name.toLowerCase().includes(filter.toLowerCase());
    const matchesStatus = statusFilter === "ALL" || v.file_status === statusFilter;
    return matchesText && matchesStatus;
  });

  return (
    <section>
      <div className="section-head">
        <h2>{showHistory ? "Versions" : "Files"}</h2>
        <div className="controls">
          <input
            placeholder="Filter by name…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          <select value={statusFilter} onChange={(e) => setStatusFilter(e.target.value)}>
            <option value="ALL">All statuses</option>
            {FILE_STATUSES.map((s) => (
              <option key={s} value={s}>{s}</option>
            ))}
          </select>
        </div>
      </div>

      {versions.length === 0 && (
        <p className="muted">
          Nothing indexed yet. Use <strong>Rescan folder</strong> to read this folder.
        </p>
      )}

      <table className="table files">
        <thead>
          <tr>
            <th>Family</th>
            <th>V</th>
            <th>Status</th>
            <th>Arc / Chapter</th>
            <th>Extraction</th>
            <th>Size</th>
            <th>Actions</th>
          </tr>
        </thead>
        <tbody>
          {shown.map((version) => (
            <tr key={version.id} className={version.missing ? "row-missing" : ""}>
              <td>
                <div className="family">{version.family_name}</div>
                <div className="mono tiny muted">{version.rel_path}</div>
                {version.version_label && <div className="tiny">label: {version.version_label}</div>}
              </td>
              <td>{version.version_number}</td>
              <td>
                <span className={`badge ${statusTone(version.file_status)}`}>
                  {version.status_marker} {version.file_status}
                </span>
                {version.is_current && <div className="tiny muted">current</div>}
                {version.missing && <div className="tiny muted">⛔ not on disk (record kept)</div>}
              </td>
              <td className="small">
                {version.arc_label ?? "—"}
                {version.chapter_label ? ` · ${version.chapter_label}` : ""}
              </td>
              <td className="small">
                {version.extraction_state}
                {version.extraction_note && (
                  <div className="tiny muted">{version.extraction_note}</div>
                )}
              </td>
              <td className="small">{formatSize(version.size_bytes)}</td>
              <td className="actions">
                <select
                  value=""
                  disabled={busy !== null}
                  onChange={(e) => {
                    const status = e.target.value;
                    if (!status) return;
                    if (status === "CURRENT") {
                      void act(`current-${version.id}`, () => api.setCurrent(version.id));
                    } else {
                      void act(`status-${version.id}`, () =>
                        api.classify(version.id, { status })
                      );
                    }
                  }}
                >
                  <option value="">Set status…</option>
                  {FILE_STATUSES.map((s) => (
                    <option key={s} value={s}>{s}</option>
                  ))}
                </select>
                <button
                  className="ghost tiny-btn"
                  disabled={busy !== null}
                  onClick={() => {
                    const arc = window.prompt("Arc label (e.g. Arc 3)", version.arc_label ?? "");
                    if (arc === null) return;
                    const chapter = window.prompt("Chapter label (optional)", version.chapter_label ?? "");
                    if (chapter === null) return;
                    void act(`meta-${version.id}`, () =>
                      api.classify(version.id, { arc, chapter })
                    );
                  }}
                >
                  Label…
                </button>
                <button
                  className="ghost tiny-btn"
                  disabled={busy !== null || version.is_current}
                  onClick={() => void act(`archive-${version.id}`, () => api.archive(version.id))}
                >
                  Archive
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <p className="muted small">
        Setting <strong>CURRENT</strong> supersedes the previous version of the same family — the old
        one is kept, relabelled HISTORICAL, and stays searchable. Nothing is ever deleted.
      </p>
    </section>
  );
}

function formatSize(bytes: number): string {
  if (bytes <= 0) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(1)} ${units[unit]}`;
}
