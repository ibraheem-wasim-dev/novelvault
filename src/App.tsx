import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { CanonItem, Dashboard, OpenReport } from "./types";
import "./canon.css";
import AddFilesPanel from "./components/AddFilesPanel";
import AiPage from "./pages/AiPage";
import CanonPage from "./pages/CanonPage";
import FilesPage from "./pages/FilesPage";
import InboxPage from "./pages/InboxPage";
import ReaderPage from "./pages/ReaderPage";
import SearchPage from "./pages/SearchPage";
import SetupPage from "./pages/SetupPage";
import StoryPage from "./pages/StoryPage";

type Screen =
  | "dashboard"
  | "files"
  | "versions"
  | "story"
  | "canon"
  | "reader"
  | "ai"
  | "inbox"
  | "search"
  | "add"
  | "activity";

export default function App() {
  const [screen, setScreen] = useState<Screen>("dashboard");
  const [novel, setNovel] = useState<{ title: string; root: string; vault_dir: string } | null>(null);
  const [dashboard, setDashboard] = useState<Dashboard | null>(null);
  const [canonItems, setCanonItems] = useState<CanonItem[]>([]);
  const [openConflicts, setOpenConflicts] = useState(0);
  const [report, setReport] = useState<OpenReport | null>(null);
  void setReport;
  const [progress, setProgress] = useState<string | null>(null);
  const [watching, setWatching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  const refresh = useCallback(async () => {
    try {
      const current = await api.currentNovel();
      setNovel(current);
      setDashboard(await api.dashboard());
      setCanonItems(await api.canon());
      const conflicts = await api.conflicts(false);
      setOpenConflicts(conflicts.length);
      setRefreshKey((k) => k + 1);
    } catch {
      setNovel(null);
      setDashboard(null);
      setCanonItems([]);
      setOpenConflicts(0);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Live updates from the folder watcher.
  useEffect(() => {
    const unlistenChanges = listen<Array<{ kind: string; relPath: string }>>(
      "vault://changes",
      (event) => {
        const lines = event.payload.map((e) => `${e.kind} ${e.relPath}`);
        setProgress(lines.length ? `Folder activity: ${lines.join(" · ")}` : null);
        void refresh();
      }
    );
    const unlistenInbox = listen("vault://inbox", () => void refresh());
    const unlistenError = listen<string>("vault://watch-error", (event) =>
      setError(event.payload)
    );
    return () => {
      void unlistenChanges.then((f) => f());
      void unlistenInbox.then((f) => f());
      void unlistenError.then((f) => f());
    };
  }, [refresh]);

  async function rescan() {
    setError(null);
    try {
      setProgress("Scanning…");
      const summary = await api.scan();
      setProgress(summary.headline);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function toggleWatch() {
    setError(null);
    try {
      if (watching) {
        await api.watchStop();
        setWatching(false);
        setProgress("Stopped watching the folder.");
      } else {
        await api.watchStart(1500);
        setWatching(true);
        setProgress("Watching the folder. Files you save will appear in the Inbox.");
      }
    } catch (e) {
      setError(String(e));
    }
  }

  if (!novel) {
    return (
      <SetupPage
        onOpened={() => {
          void refresh();
        }}
      />
    );
  }

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">📚</span>
          <div>
            <div className="brand-title">NovelVault</div>
            <div className="muted small">{novel.title}</div>
          </div>
        </div>

        <nav>
          {(
            [
              ["dashboard", "🏠 Dashboard"],
              ["files", "📁 Files"],
              ["versions", "🔄 Versions"],
              ["story", "🧭 Story"],
              ["canon", "⚖️ Canon"],
              ["reader", "🧠 Story Brain"],
              ["ai", "🤖 Librarian"],
              ["inbox", "📥 Inbox"],
              ["search", "🔍 Search"],
              ["add", "➕ Bring files in"],
              ["activity", "🧾 Activity"],
            ] as Array<[Screen, string]>
          ).map(([key, label]) => (
            <button
              key={key}
              className={screen === key ? "nav active" : "nav"}
              onClick={() => setScreen(key)}
            >
              {label}
              {key === "inbox" && dashboard && dashboard.inbox_open > 0 && (
                <span className="pill">{dashboard.inbox_open}</span>
              )}
              {key === "canon" && openConflicts > 0 && <span className="pill">{openConflicts}</span>}
            </button>
          ))}
        </nav>

        <div className="sidebar-foot">
          <button className="ghost" onClick={rescan}>Rescan folder</button>
          <button className={watching ? "ghost on" : "ghost"} onClick={toggleWatch}>
            {watching ? "● Watching" : "○ Watch folder"}
          </button>
          <button
            className="ghost"
            onClick={() =>
              void (async () => {
                if (watching) {
                  await api.watchStop();
                  setWatching(false);
                }
                await api.closeNovel();
                setNovel(null);
              })()
            }
          >
            Switch novel
          </button>
          <div className="muted tiny" title={novel.root}>{novel.root}</div>
        </div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div className="crumbs">{novel.title}</div>
          <div className="topbar-right">
            {dashboard && (
              <>
                <span className="chip">📥 {dashboard.inbox_open} inbox</span>
                <span className="chip">⚫ {dashboard.unclassified} unclassified</span>
                <span className="chip">🟢 {dashboard.current_versions} current</span>
                <span className="chip">⚖️ {canonItems.length} canon</span>
                {openConflicts > 0 && (
                  <span className="chip" onClick={() => setScreen("canon")}>
                    ⚠ {openConflicts} conflict{openConflicts === 1 ? "" : "s"}
                  </span>
                )}
              </>
            )}
          </div>
        </header>

        {progress && <div className="banner">{progress}</div>}
        {error && <div className="banner error">{error}</div>}

        <div className="content">
          {screen === "dashboard" && <DashboardView dashboard={dashboard} report={report} />}
          {screen === "files" && (
            <FilesPage
              key={refreshKey}
              includeMissing={false}
              onChanged={refresh}
              onError={setError}
            />
          )}
          {screen === "versions" && (
            <FilesPage
              key={`v-${refreshKey}`}
              includeMissing={true}
              onChanged={refresh}
              onError={setError}
              showHistory
            />
          )}
          {screen === "story" && (
            <StoryPage key={`s-${refreshKey}`} onChanged={refresh} onError={setError} />
          )}
          {screen === "canon" && (
            <CanonPage key={`c-${refreshKey}`} onChanged={refresh} onError={setError} />
          )}
          {screen === "reader" && <ReaderPage key={`r-${refreshKey}`} onChanged={refresh} onError={setError} />}
          {screen === "ai" && <AiPage key={`a-${refreshKey}`} onChanged={refresh} onError={setError} />}
          {screen === "inbox" && (
            <InboxPage key={`i-${refreshKey}`} onChanged={refresh} onError={setError} />
          )}
          {screen === "search" && <SearchPage onError={setError} />}
          {screen === "add" && (
            <section>
              <h2>Bring files into this novel</h2>
              <AddFilesPanel onDone={refresh} onError={setError} />
              <p className="muted small">
                Files land as UNKNOWN and wait in the Inbox. Copies are verified by hash before they
                are trusted, and an existing file is never overwritten — a name clash is brought in
                under a new name instead.
              </p>
            </section>
          )}
          {screen === "activity" && <ActivityView />}
        </div>
      </main>
    </div>
  );
}

function DashboardView({
  dashboard,
  report,
}: {
  dashboard: Dashboard | null;
  report: OpenReport | null;
}) {
  if (!dashboard) return <p className="muted">No novel open.</p>;
  const stats: Array<[string, number]> = [
    ["Files (families)", dashboard.files],
    ["Versions", dashboard.versions],
    ["Marked CURRENT", dashboard.current_versions],
    ["Unclassified", dashboard.unclassified],
    ["Off disk (kept)", dashboard.missing],
    ["Extraction problems", dashboard.extraction_failed],
    ["Snapshots", dashboard.snapshots],
    ["Inbox open", dashboard.inbox_open],
  ];
  return (
    <section>
      <h2>Dashboard</h2>
      <div className="grid">
        {stats.map(([label, value]) => (
          <div className="stat" key={label}>
            <div className="stat-value">{value}</div>
            <div className="muted small">{label}</div>
          </div>
        ))}
      </div>

      <h3>Inbox by kind</h3>
      {Object.keys(dashboard.inbox_by_kind).length === 0 ? (
        <p className="muted">Inbox is empty.</p>
      ) : (
        <table className="table">
          <thead>
            <tr><th>Kind</th><th>Open</th></tr>
          </thead>
          <tbody>
            {Object.entries(dashboard.inbox_by_kind).map(([kind, count]) => (
              <tr key={kind}><td>{kind}</td><td>{count}</td></tr>
            ))}
          </tbody>
        </table>
      )}

      <h3>Folder</h3>
      <table className="table">
        <tbody>
          <tr><th>Root</th><td className="mono">{dashboard.novel.title} · {report?.root ?? ""}</td></tr>
          <tr><th>Vault data</th><td className="mono">.novelvault/vault.db</td></tr>
          <tr><th>Portable</th><td>Yes — every path is stored relative to the novel folder</td></tr>
        </tbody>
      </table>

      <p className="muted small">{dashboard.counts_note}</p>
    </section>
  );
}

function ActivityView() {
  const [rows, setRows] = useState<Array<{ at: string; action: string; entityType: string; entityId: string }>>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .audit(100)
      .then(setRows)
      .catch((e) => setError(String(e)));
  }, []);

  return (
    <section>
      <h2>Activity</h2>
      <p className="muted small">
        Every mutation the app performs is appended here — including every file it brought in and
        every canon decision. Rows are never rewritten or removed.
      </p>
      {error && <p className="error">{error}</p>}
      <table className="table">
        <thead>
          <tr><th>When</th><th>Action</th><th>Entity</th><th>Id</th></tr>
        </thead>
        <tbody>
          {rows.map((row, index) => (
            <tr key={`${row.at}-${index}`}>
              <td className="mono small">{row.at}</td>
              <td>{row.action}</td>
              <td>{row.entityType}</td>
              <td className="mono small">{row.entityId.slice(0, 8)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
