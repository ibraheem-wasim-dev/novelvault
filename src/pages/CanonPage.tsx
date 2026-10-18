import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type {
  CanonConflict,
  CanonHistoryRow,
  CanonItem,
  Character,
  FileVersion,
  ImpactReport,
  KnowledgeRow,
} from "../types";
import { CANON_STATUSES, KNOWLEDGE_STATES, canonTone } from "../types";

/**
 * Canon.
 *
 * Every control here maps to one deliberate author decision. Nothing on this screen
 * happens by itself: a fact is proposed, sourced, then promoted with a written reason;
 * changing a value never destroys the old one; and a detected conflict is shown, never
 * silently resolved.
 */
export default function CanonPage({
  onChanged,
  onError,
}: {
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  const [items, setItems] = useState<CanonItem[]>([]);
  const [conflicts, setConflicts] = useState<CanonConflict[]>([]);
  const [knowledge, setKnowledge] = useState<KnowledgeRow[]>([]);
  const [characters, setCharacters] = useState<Character[]>([]);
  const [versions, setVersions] = useState<FileVersion[]>([]);
  const [includeSuperseded, setIncludeSuperseded] = useState(false);
  const [filter, setFilter] = useState("");

  const [subject, setSubject] = useState("");
  const [value, setValue] = useState("");
  const [category, setCategory] = useState("object");

  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [reasons, setReasons] = useState<Record<string, string>>({});
  const [targets, setTargets] = useState<Record<string, string>>({});
  const [sourceFor, setSourceFor] = useState<Record<string, string>>({});
  const [impact, setImpact] = useState<ImpactReport | null>(null);
  const [history, setHistory] = useState<CanonHistoryRow[] | null>(null);

  const [knowCharacter, setKnowCharacter] = useState("");
  const [knowItem, setKnowItem] = useState("");
  const [knowState, setKnowState] = useState<string>(KNOWLEDGE_STATES[0]);

  const load = useCallback(async () => {
    try {
      const [canonRows, conflictRows, knowledgeRows, characterRows, versionRows] =
        await Promise.all([
          api.canon(undefined, includeSuperseded),
          api.conflicts(true),
          api.knowledge(),
          api.characters(),
          api.versions(),
        ]);
      setItems(canonRows);
      setConflicts(conflictRows);
      setKnowledge(knowledgeRows);
      setCharacters(characterRows);
      setVersions(versionRows);
      if (characterRows.length && !knowCharacter) setKnowCharacter(characterRows[0].id);
      if (canonRows.length && !knowItem) setKnowItem(canonRows[0].id);
    } catch (e) {
      onError(String(e));
    }
  }, [includeSuperseded, knowCharacter, knowItem, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  async function propose() {
    if (!subject.trim() || !value.trim()) {
      onError("A canon proposal needs a subject and a value.");
      return;
    }
    try {
      await api.proposeCanon(subject.trim(), value.trim(), { category });
      setSubject("");
      setValue("");
      await load();
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  async function attachSource(item: CanonItem) {
    const chosen = sourceFor[item.id];
    if (!chosen) {
      onError("Pick a file version to use as the source.");
      return;
    }
    const version = versions.find((v) => v.id === chosen);
    try {
      await api.addCanonSource(
        item.id,
        "FileVersion",
        chosen,
        version?.family_name,
        version ? `v${version.version_number}` : undefined
      );
      await load();
    } catch (e) {
      onError(String(e));
    }
  }

  async function promote(item: CanonItem) {
    const target = targets[item.id] ?? "CANON";
    const reason = reasons[item.id] ?? "";
    if (!reason.trim()) {
      onError("Promoting a canon fact needs a reason — it is written into the history.");
      return;
    }
    try {
      await api.promoteCanon(item.id, target, reason.trim());
      await load();
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  async function changeValue(item: CanonItem) {
    const draft = (drafts[item.id] ?? "").trim();
    const reason = (reasons[item.id] ?? "").trim();
    if (!draft) {
      onError("Type the new value first.");
      return;
    }
    if (!reason) {
      onError("Changing canon needs a reason — the old value is kept and the change is recorded.");
      return;
    }
    try {
      await api.changeCanonValue(item.id, draft, reason);
      setDrafts((prev) => ({ ...prev, [item.id]: "" }));
      await load();
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  async function showImpact(item: CanonItem) {
    try {
      setImpact(await api.canonImpact(item.id));
      setHistory(null);
    } catch (e) {
      onError(String(e));
    }
  }

  async function showHistory(item: CanonItem) {
    try {
      setHistory(await api.canonHistory(item.id));
      setImpact(null);
    } catch (e) {
      onError(String(e));
    }
  }

  async function runProbe() {
    try {
      const report = await api.probe();
      await load();
      onChanged();
      onError(
        report.conflicts_raised === 0
          ? `Checked ${report.subjects_checked} superseded fact(s): no contradictions found.`
          : `${report.conflicts_raised} new conflict(s): ${report.details.join(" · ")}`
      );
    } catch (e) {
      onError(String(e));
    }
  }

  async function resolve(conflict: CanonConflict, decision: string) {
    const reason = reasons[conflict.id] ?? "";
    if (!reason.trim()) {
      onError("A resolution needs a reason, so the decision is auditable.");
      return;
    }
    try {
      await api.resolveConflict(conflict.id, decision, reason.trim());
      await load();
    } catch (e) {
      onError(String(e));
    }
  }

  async function recordKnowledge() {
    if (!knowCharacter || !knowItem) {
      onError("Pick a character and a canon fact.");
      return;
    }
    try {
      await api.setKnowledge(knowCharacter, knowItem, knowState);
      await load();
    } catch (e) {
      onError(String(e));
    }
  }

  const visible = items.filter((item) =>
    filter.trim()
      ? item.subject.toLowerCase().includes(filter.trim().toLowerCase()) ||
        item.value.toLowerCase().includes(filter.trim().toLowerCase())
      : true
  );
  const openConflicts = conflicts.filter((c) => c.status === "OPEN");

  return (
    <section>
      <div className="section-head">
        <h2>Canon</h2>
        <span className="muted small">
          R1 nothing becomes canon by itself · R2 no value is ever destroyed · R3 every fact needs a
          source
        </span>
      </div>

      <div className="controls">
        <input
          value={subject}
          onChange={(e) => setSubject(e.target.value)}
          placeholder="Subject, e.g. Keepsake"
          aria-label="Canon subject"
        />
        <input
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="Value, e.g. wooden bird"
          aria-label="Canon value"
        />
        <input
          className="narrow"
          value={category}
          onChange={(e) => setCategory(e.target.value)}
          aria-label="Canon category"
        />
        <button className="primary" onClick={propose}>
          Propose fact
        </button>
        <button onClick={runProbe}>Check for conflicts</button>
      </div>

      {openConflicts.length > 0 && (
        <div className="conflict-block">
          <h3>⚠ {openConflicts.length} open conflict(s)</h3>
          <p className="muted small">
            The detector found these; it did not choose. Decide, with a reason, and the decision is
            recorded. Canon itself is only changed by you, explicitly.
          </p>
          <table className="table">
            <thead>
              <tr>
                <th>Subject</th>
                <th>Canon says</th>
                <th>Other source says</th>
                <th>Which source</th>
                <th>Decision</th>
              </tr>
            </thead>
            <tbody>
              {openConflicts.map((conflict) => (
                <tr key={conflict.id} className="conflict-row">
                  <td>
                    <strong>{conflict.subject}</strong>
                    <div className="muted tiny mono">{conflict.detector}</div>
                  </td>
                  <td>{conflict.current_value}</td>
                  <td>{conflict.conflicting_value}</td>
                  <td className="mono small">{conflict.conflicting_source}</td>
                  <td className="actions">
                    <input
                      className="reason"
                      value={reasons[conflict.id] ?? ""}
                      onChange={(e) =>
                        setReasons((prev) => ({ ...prev, [conflict.id]: e.target.value }))
                      }
                      placeholder="reason"
                      aria-label="Resolution reason"
                    />
                    <button className="tiny-btn" onClick={() => void resolve(conflict, "keep_current")}>
                      Keep current
                    </button>
                    <button className="tiny-btn" onClick={() => void resolve(conflict, "accept_change")}>
                      Accept change
                    </button>
                    <button className="tiny-btn" onClick={() => void resolve(conflict, "defer")}>
                      Defer
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="controls">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter by subject or value"
          aria-label="Filter canon"
        />
        <label className="toggle">
          <input
            type="checkbox"
            checked={includeSuperseded}
            onChange={(e) => setIncludeSuperseded(e.target.checked)}
          />
          show superseded &amp; rejected
        </label>
      </div>

      {visible.length === 0 && <p className="muted">No canon recorded yet.</p>}

      {visible.map((item) => (
        <div className={`canon-card ${canonTone(item.status)}`} key={item.id}>
          <div className="canon-head">
            <strong>{item.subject}</strong>
            <span className={`canon-chip ${canonTone(item.status)}`}>{item.status_label}</span>
            {item.is_superseded && <span className="muted tiny">superseded — kept as history</span>}
            {item.category && <span className="muted tiny">{item.category}</span>}
          </div>

          <div className="canon-value">“{item.value}”</div>
          {item.reason && <div className="muted tiny">reason: {item.reason}</div>}

          <div className="canon-line">
            <span className="muted small">
              {item.sources.length === 0
                ? "No source yet — R3 blocks promotion without one."
                : `Sources: ${item.sources
                    .map((s) => `${s.source_type}${s.location ? ` ${s.location}` : ""}`)
                    .join(", ")}`}
            </span>
          </div>

          <div className="actions">
            <select
              value={sourceFor[item.id] ?? ""}
              onChange={(e) => setSourceFor((prev) => ({ ...prev, [item.id]: e.target.value }))}
            >
              <option value="">choose a file version…</option>
              {versions.map((v) => (
                <option key={v.id} value={v.id}>
                  {v.family_name} v{v.version_number} {v.file_status}
                </option>
              ))}
            </select>
            <button className="tiny-btn" onClick={() => void attachSource(item)}>
              Attach source
            </button>

            <select
              value={targets[item.id] ?? "CANON"}
              onChange={(e) => setTargets((prev) => ({ ...prev, [item.id]: e.target.value }))}
            >
              {CANON_STATUSES.map((s) => (
                <option key={s} value={s}>
                  {s.replace(/_/g, " ")}
                </option>
              ))}
            </select>
            <button className="tiny-btn" onClick={() => void promote(item)}>
              Promote
            </button>

            <input
              className="reason"
              value={reasons[item.id] ?? ""}
              onChange={(e) => setReasons((prev) => ({ ...prev, [item.id]: e.target.value }))}
              placeholder="reason"
              aria-label="Reason"
            />
          </div>

          <div className="actions">
            <input
              className="reason"
              value={drafts[item.id] ?? ""}
              onChange={(e) => setDrafts((prev) => ({ ...prev, [item.id]: e.target.value }))}
              placeholder="new value"
              aria-label="New value"
            />
            <button className="tiny-btn" onClick={() => void changeValue(item)}>
              Save change
            </button>
            <button className="tiny-btn" onClick={() => void showImpact(item)}>
              Impact
            </button>
            <button className="tiny-btn" onClick={() => void showHistory(item)}>
              History
            </button>
          </div>
        </div>
      ))}

      {impact && (
        <div className="plan">
          <div className="plan-head">
            <strong>
              Impact — {impact.subject} = “{impact.value}” [{impact.status}]
            </strong>
            <button className="tiny-btn" onClick={() => setImpact(null)}>
              close
            </button>
          </div>
          <p className="small">{impact.summary}</p>
          {impact.is_locked && (
            <p className="error small">This fact is LOCKED CANON — changing it is deliberate.</p>
          )}
          {impact.affected_notes.length > 0 && (
            <>
              <h4>Notes</h4>
              <ul className="small">
                {impact.affected_notes.map((n) => (
                  <li key={n}>{n}</li>
                ))}
              </ul>
            </>
          )}
          {impact.affected_characters.length > 0 && (
            <>
              <h4>Who knows</h4>
              <ul className="small">
                {impact.affected_characters.map((c) => (
                  <li key={c}>{c}</li>
                ))}
              </ul>
            </>
          )}
          {impact.files_mentioning_old_value.length > 0 && (
            <>
              <h4>Files containing this value</h4>
              <ul className="small mono">
                {impact.files_mentioning_old_value.map((f) => (
                  <li key={f}>{f}</li>
                ))}
              </ul>
            </>
          )}
          {impact.chapters_mentioning_subject.length > 0 && (
            <>
              <h4>Chapter records</h4>
              <ul className="small">
                {impact.chapters_mentioning_subject.map((c) => (
                  <li key={c}>{c}</li>
                ))}
              </ul>
            </>
          )}
        </div>
      )}

      {history && (
        <div className="plan">
          <div className="plan-head">
            <strong>History — the append-only record</strong>
            <button className="tiny-btn" onClick={() => setHistory(null)}>
              close
            </button>
          </div>
          <table className="table">
            <thead>
              <tr>
                <th>When</th>
                <th>Action</th>
                <th>Reason</th>
                <th>Before</th>
                <th>After</th>
              </tr>
            </thead>
            <tbody>
              {history.map((row) => (
                <tr key={row.id}>
                  <td className="mono tiny">{row.at}</td>
                  <td>{row.action}</td>
                  <td className="small">{row.reason}</td>
                  <td className="mono tiny">{row.before_json}</td>
                  <td className="mono tiny">{row.after_json}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <h3>Who knows what (R4)</h3>
      <p className="muted small">
        Reader knowledge and character knowledge are separate rows on purpose — a character can be
        wrong about a fact the reader already knows.
      </p>
      <div className="controls">
        <select value={knowCharacter} onChange={(e) => setKnowCharacter(e.target.value)}>
          {characters.map((c) => (
            <option key={c.id} value={c.id}>
              {c.is_reader_sentinel ? "👁 Reader" : c.name}
            </option>
          ))}
        </select>
        <select value={knowItem} onChange={(e) => setKnowItem(e.target.value)}>
          {items.map((i) => (
            <option key={i.id} value={i.id}>
              {i.subject} = {i.value}
            </option>
          ))}
        </select>
        <select value={knowState} onChange={(e) => setKnowState(e.target.value)}>
          {KNOWLEDGE_STATES.map((s) => (
            <option key={s} value={s}>
              {s.replace(/_/g, " ")}
            </option>
          ))}
        </select>
        <button onClick={recordKnowledge}>Record</button>
      </div>
      <table className="table">
        <thead>
          <tr>
            <th></th>
            <th>Who</th>
            <th>Fact</th>
            <th>State</th>
          </tr>
        </thead>
        <tbody>
          {knowledge.map((row) => (
            <tr key={row.id}>
              <td>{row.glyph}</td>
              <td>{row.character_name}</td>
              <td>{row.subject}</td>
              <td>{row.state.replace(/_/g, " ")}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
