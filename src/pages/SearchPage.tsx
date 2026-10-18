import { useState } from "react";
import { api } from "../api";
import type { SearchHit } from "../types";

interface Props {
  onError: (message: string) => void;
}

export default function SearchPage({ onError }: Props) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [busy, setBusy] = useState(false);

  async function run() {
    if (!query.trim()) return;
    setBusy(true);
    try {
      setHits(await api.search(query, 40));
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section>
      <h2>Search</h2>
      <p className="muted small">
        Searches file names and extracted text across current <em>and</em> archived material.
        Off-disk files stay searchable too — the record and its text are kept.
      </p>

      <div className="controls">
        <input
          placeholder="wooden bird, Maheen, Arc 2…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void run();
          }}
        />
        <button className="primary" onClick={() => void run()} disabled={busy}>
          {busy ? "Searching…" : "Search"}
        </button>
      </div>

      {hits !== null && hits.length === 0 && <p className="muted">No matches.</p>}

      {hits?.map((hit) => (
        <article className="card" key={hit.file_version_id}>
          <header className="card-head">
            <span className="badge">{hit.status_marker} {hit.file_status}</span>
            <strong>{hit.family_name}</strong>
            <span className="muted small">v{hit.version_number}</span>
            {hit.is_current && <span className="badge tone-current">current</span>}
          </header>
          <div className="mono tiny muted">{hit.rel_path}</div>
          {hit.snippet && <p className="snippet">{hit.snippet}</p>}
          <div className="muted tiny">matched in {hit.matched_in}</div>
        </article>
      ))}
    </section>
  );
}
