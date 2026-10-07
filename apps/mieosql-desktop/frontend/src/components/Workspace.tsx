import { useEffect, useMemo, useRef, useState } from "react";
import {
  Braces,
  CirclePlay,
  Eraser,
  FileCode2,
  GitBranch,
  Layers,
  ListTree,
  Plus,
  RefreshCw,
  Save,
  Square,
  Table2,
  Wand2,
  X,
} from "lucide-react";

import type { ObjectRef } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import type { Tab } from "../lib/store";
import { useStore } from "../lib/store";
import { FeatureHost } from "../features/registry";
import { EditorPane, type EditorHandle } from "./EditorPane";
import { ResultPanel } from "./ResultPanel";

// ---------------------------------------------------------------------------
// Workspace: tab bar + per-tab content (editor, table grid, object browser)
// ---------------------------------------------------------------------------

export function Workspace() {
  const store = useStore();
  const activeTab = store.tabs.find((t) => t.id === store.activeTabId);
  const editorRef = useRef<EditorHandle | null>(null);
  const prevTabIdRef = useRef<string | null>(null);
  const changeTimer = useRef<number | null>(null);

  // Persist the outgoing editor text, load the incoming one.
  useEffect(() => {
    const prev = prevTabIdRef.current;
    const view = editorRef.current;
    const next = activeTab;
    if (prev && prev !== next?.id && view) {
      useStore.getState().setSql(prev, view.getText());
    }
    prevTabIdRef.current = next?.id ?? null;
    if (next?.kind === "query" && view) {
      if (view.getText() !== next.sql) view.setText(next.sql);
    }
  }, [activeTab]);

  // Toolbar/menu events that target the editor.
  useEffect(() => {
    const clear = () => editorRef.current?.setText("");
    const focus = () => editorRef.current?.focus();
    window.addEventListener("mieosql:clear-editor", clear);
    window.addEventListener("mieosql:focus-editor", focus);
    return () => {
      window.removeEventListener("mieosql:clear-editor", clear);
      window.removeEventListener("mieosql:focus-editor", focus);
    };
  }, []);

  const activeSession = useStore((s) =>
    s.sessions.find((x) => x.id === activeTab?.sessionId),
  );

  const runActive = () => {
    const tab = activeTab;
    if (!tab || tab.kind !== "query") return;
    const text = editorRef.current?.getText() ?? tab.sql;
    store.setSql(tab.id, text);
    void store.requestRun(tab.id, text);
  };

  const onEditorChange = (text: string) => {
    if (changeTimer.current) window.clearTimeout(changeTimer.current);
    changeTimer.current = window.setTimeout(() => {
      const id = useStore.getState().activeTabId;
      if (id) useStore.getState().setSql(id, text);
    }, 350);
  };

  return (
    <section className="flex min-w-0 flex-1 flex-col bg-surface">
      {/* Tab bar */}
      <div className="flex h-[34px] shrink-0 items-center gap-1 overflow-x-auto bg-sidebar px-2">
        {store.tabs.map((tab) => (
          <TabButton key={tab.id} tab={tab} active={tab.id === store.activeTabId} />
        ))}
        <button
          className="grid h-6 w-6 shrink-0 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700"
          title="New query tab"
          onClick={() => store.newQueryTab()}
        >
          <Plus size={14} strokeWidth={1.8} />
        </button>
      </div>

      {!activeTab && <EmptyState />}

      {activeTab?.kind === "query" && (
        <div className="flex min-h-0 flex-1 flex-col">
          <QueryToolbar
            tab={activeTab}
            onRun={runActive}
            onExplain={() => {
              if (!activeTab.sessionId) return;
              const text = (editorRef.current?.getText() ?? activeTab.sql).trim();
              if (!text) return;
              store.explainActiveQuery(activeTab.sessionId, text);
            }}
            onBeautify={() =>
              window.dispatchEvent(new CustomEvent("mieosql:format-sql"))
            }
            onClear={() => {
              editorRef.current?.setText("");
              store.setSql(activeTab.id, "");
            }}
          />
          <SplitPane
            editor={
              <EditorPane
                ref={editorRef}
                initialDoc={activeTab.sql}
                engine={activeSession?.engine ?? null}
                sessionId={activeTab.sessionId}
                scope={activeTab.scope ?? null}
                onRun={runActive}
                onChange={onEditorChange}
                onCursor={store.setCursor}
              />
            }
            results={<ResultPanel tab={activeTab} />}
          />
        </div>
      )}

      {activeTab?.kind === "table" && (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex h-10 shrink-0 items-center justify-between border-b border-slate-200/80 bg-white px-3">
            <div className="flex items-center gap-2 text-[13px]">
              <Table2 size={14} className="text-slate-500" />
              <span className="font-medium text-slate-900">
                {activeTab.table}
              </span>
              <span className="text-slate-400">
                {activeTab.scope?.schema
                  ? `${activeTab.scope.schema}.`
                  : activeTab.scope?.database
                    ? `${activeTab.scope.database}.`
                    : ""}
                {activeTab.table}
              </span>
            </div>
            <LimitSelect />
          </div>
          <ResultPanel tab={activeTab} />
        </div>
      )}

      {activeTab?.kind === "objects" && <ObjectsView tab={activeTab} />}

      {activeTab?.kind === "feature" && <FeatureHost tab={activeTab} />}
    </section>
  );
}

function TabButton({ tab, active }: { tab: Tab; active: boolean }) {
  const store = useStore();
  const icon =
    tab.kind === "query" ? (
      <FileCode2 size={12} className="text-slate-500" />
    ) : tab.kind === "table" ? (
      <Table2 size={12} className="text-slate-500" />
    ) : tab.kind === "feature" ? (
      <Layers size={12} className="text-slate-500" />
    ) : (
      <ListTree size={12} className="text-slate-500" />
    );
  return (
    <div
      className={`flex h-6 max-w-[220px] shrink-0 cursor-default items-center gap-1.5 rounded-md px-2 ${
        active
          ? "bg-surface font-medium text-slate-900 shadow-[0_1px_2px_rgba(0,0,0,0.08)]"
          : "text-slate-500 hover:bg-slate-900/[0.04]"
      }`}
      onClick={() => store.setActiveTab(tab.id)}
      title={tab.title}
    >
      {tab.running ? (
        <span className="h-2 w-2 animate-pulse rounded-full bg-accent" />
      ) : (
        icon
      )}
      <span className="truncate text-[12.5px]">{tab.title}</span>
      {tab.results.length > 0 && !tab.running && (
        <span className="text-[11px] text-slate-400 tabular-nums">
          {tab.results
            .reduce((acc, r) => acc + r.rows.length, 0)
            .toLocaleString()}
        </span>
      )}
      <button
        className="grid h-4 w-4 place-items-center rounded text-slate-400 hover:bg-slate-900/[0.08] hover:text-slate-600"
        onClick={(e) => {
          e.stopPropagation();
          store.closeTab(tab.id);
        }}
      >
        <X size={10} />
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Query toolbar
// ---------------------------------------------------------------------------

function QueryToolbar({
  tab,
  onRun,
  onExplain,
  onBeautify,
  onClear,
}: {
  tab: Tab;
  onRun: () => void;
  onExplain: () => void;
  onBeautify: () => void;
  onClear: () => void;
}) {
  const store = useStore();
  const session = useStore((s) => s.sessions.find((x) => x.id === tab.sessionId));
  const runningResult = tab.results.find((r) => r.status === "running");

  const toolBtn =
    "flex h-7 items-center gap-1 rounded-md px-2 text-[12.5px] text-slate-700 hover:bg-slate-900/[0.05] disabled:pointer-events-none disabled:text-slate-300";

  return (
    <div className="flex h-10 shrink-0 items-center justify-between border-b border-slate-200/80 bg-white px-3">
      <div className="flex items-center gap-1">
        <button
          id="run-btn"
          className="flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[12.5px] font-medium text-white transition-colors hover:bg-accent-hover active:scale-[0.98] disabled:opacity-40"
          onClick={onRun}
          disabled={!tab.sessionId || !!runningResult}
          title="Run query (⌘Enter)"
        >
          <CirclePlay size={13} />
          Run
        </button>
        <button
          className="flex h-7 items-center gap-1.5 rounded-md px-2 text-[12.5px] font-medium text-slate-600 transition-colors hover:bg-slate-900/[0.05] disabled:opacity-40"
          onClick={() => runningResult && store.cancelQuery(runningResult.queryId)}
          disabled={!runningResult}
          title="Stop query"
        >
          <Square size={10} className="text-rose-500" />
          Stop
        </button>

        <div className="mx-1.5 h-4 w-px bg-slate-200" />

        {session && (
          <>
            <button
              className={toolBtn}
              disabled={session.in_transaction}
              title="Begin transaction"
              onClick={() => void store.beginTransaction(session.id)}
            >
              <GitBranch size={13} className="text-slate-500" />
              Begin
            </button>
            <button
              className={toolBtn}
              disabled={!session.in_transaction}
              title="Commit transaction"
              onClick={() => void store.commitTransaction(session.id)}
            >
              Commit
            </button>
            <button
              className={toolBtn}
              disabled={!session.in_transaction}
              title="Rollback transaction"
              onClick={() => void store.rollbackTransaction(session.id)}
            >
              Rollback
            </button>
            <div className="mx-1.5 h-4 w-px bg-slate-200" />
          </>
        )}

        <button
          className={toolBtn}
          disabled={!tab.sessionId || !!runningResult}
          title="Visual explain (runs EXPLAIN)"
          onClick={onExplain}
        >
          <ListTree size={13} className="text-slate-500" />
          Explain
        </button>
        <button
          className={toolBtn}
          title="Format SQL (⇧⌥F)"
          onClick={onBeautify}
        >
          <Wand2 size={13} className="text-slate-500" />
          Beautify
        </button>
        <button
          className={toolBtn}
          title="Visual query builder"
          onClick={() => store.openFeatureDialog("builder")}
        >
          <Braces size={13} className="text-slate-500" />
          Builder
        </button>
        <div className="mx-1.5 h-4 w-px bg-slate-200" />
        <button
          className={toolBtn}
          disabled
          title="Saved queries — roadmap P2"
        >
          <Save size={13} />
        </button>
        <button className={toolBtn} onClick={onClear} title="Clear editor">
          <Eraser size={13} />
        </button>
      </div>

      <div className="flex items-center gap-2">
        <div className="flex h-7 items-center gap-1.5 rounded-md bg-slate-900/[0.04] px-2 text-[11.5px]">
          <span
            className={`inline-block h-1.5 w-1.5 rounded-full ${
              session ? "bg-emerald-500" : "bg-slate-300"
            }`}
          />
          <span className="max-w-[160px] truncate text-slate-600">
            {session ? session.name : "no connection"}
            {session?.database ? ` · ${session.database}` : ""}
          </span>
        </div>
        <LimitSelect />
      </div>
    </div>
  );
}

function LimitSelect() {
  const limit = useStore((s) => s.limit);
  const options = useStore((s) => s.limitOptions);
  const setLimit = useStore((s) => s.setLimit);
  return (
    <label className="flex h-7 items-center gap-1 rounded-md bg-slate-900/[0.04] px-2 text-[11.5px]">
      <span className="text-slate-500">Limit</span>
      <select
        className="border-none bg-transparent p-0 text-[12.5px] font-medium text-slate-800 focus:outline-none"
        value={limit}
        onChange={(e) => setLimit(Number(e.target.value))}
      >
        {options.map((o) => (
          <option key={o} value={o}>
            {o === 0 ? "No limit" : o}
          </option>
        ))}
      </select>
    </label>
  );
}

// ---------------------------------------------------------------------------
// Editor / results split
// ---------------------------------------------------------------------------

function SplitPane({
  editor,
  results,
}: {
  editor: React.ReactNode;
  results: React.ReactNode;
}) {
  const [pct, setPct] = useState(48);
  const containerRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!draggingRef.current || !containerRef.current) return;
      const rect = containerRef.current.getBoundingClientRect();
      const next = ((e.clientY - rect.top) / rect.height) * 100;
      setPct(Math.min(85, Math.max(12, next)));
    };
    const onUp = () => {
      draggingRef.current = false;
      document.body.style.cursor = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, []);

  useEffect(() => {
    const set = (p: number) => setPct(p);
    const onEditor = () => set(72);
    const onResults = () => set(16);
    const onReset = () => set(48);
    window.addEventListener("mieosql:split-editor", onEditor);
    window.addEventListener("mieosql:split-results", onResults);
    window.addEventListener("mieosql:split-reset", onReset);
    return () => {
      window.removeEventListener("mieosql:split-editor", onEditor);
      window.removeEventListener("mieosql:split-results", onResults);
      window.removeEventListener("mieosql:split-reset", onReset);
    };
  }, []);

  return (
    <div ref={containerRef} className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 overflow-hidden bg-white" style={{ height: `${pct}%` }}>
        {editor}
      </div>
      <div
        className="group flex h-[5px] shrink-0 cursor-row-resize items-center justify-center bg-slate-100 transition-colors hover:bg-accent/15"
        onMouseDown={() => {
          draggingRef.current = true;
          document.body.style.cursor = "row-resize";
        }}
      >
        <div className="h-px w-10 bg-slate-300 group-hover:bg-accent" />
      </div>
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">{results}</div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Object browser (list view tab)
// ---------------------------------------------------------------------------

function ObjectsView({ tab }: { tab: Tab }) {
  const store = useStore();
  const [objects, setObjects] = useState<ObjectRef[] | null>(null);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);

  const load = useMemo(
    () => async () => {
      if (!tab.sessionId) return;
      setError(null);
      setObjects(null);
      try {
        setObjects(
          await api.listObjects(
            tab.sessionId,
            tab.scope ?? {},
            tab.objectKind ?? "table",
            null,
          ),
        );
      } catch (err) {
        setError(errorText(err));
      }
    },
    [tab.sessionId, tab.scope, tab.objectKind],
  );

  useEffect(() => {
    void load();
  }, [load]);

  const shown = (objects ?? []).filter(
    (o) => !filter.trim() || o.name.toLowerCase().includes(filter.toLowerCase()),
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-slate-200/80 bg-white px-3">
        <div className="flex items-center gap-2 text-[13px]">
          <button
            className="grid h-7 w-7 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700"
            title="Reload"
            onClick={() => void load()}
          >
            <RefreshCw size={13} strokeWidth={1.8} />
          </button>
          <input
            className="h-7 w-56 rounded-md bg-slate-900/[0.04] px-2 text-[12.5px] text-slate-800 placeholder:text-slate-500 focus:ring-2 focus:ring-accent/40 focus:outline-none"
            placeholder="Filter objects…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <span className="text-[12px] text-slate-500">
          {shown.length.toLocaleString()} object(s)
        </span>
      </div>
      <div className="custom-scrollbar flex-1 overflow-auto bg-surface">
        {error && <div className="p-4 text-[13px] text-rose-600">{error}</div>}
        {!objects && !error && (
          <div className="p-4 text-[13px] text-slate-500">Loading objects…</div>
        )}
        {objects && (
          <table className="w-full border-collapse text-left text-[12.5px]">
            <thead className="sticky top-0 bg-sidebar">
              <tr className="text-slate-500">
                <th className="border-b border-slate-200 px-3 py-1.5 text-[11.5px] font-semibold">
                  Name
                </th>
                <th className="w-[110px] border-b border-slate-200 px-3 py-1.5 text-right text-[11.5px] font-semibold">
                  Rows
                </th>
                <th className="w-[180px] border-b border-slate-200 px-3 py-1.5 text-[11.5px] font-semibold">
                  Modified
                </th>
                <th className="border-b border-slate-200 px-3 py-1.5 text-[11.5px] font-semibold">
                  Comment
                </th>
              </tr>
            </thead>
            <tbody>
              {shown.map((o) => (
                <tr
                  key={`${o.schema ?? ""}.${o.name}`}
                  className="cursor-default border-b border-[#EDEDF0] hover:bg-slate-900/[0.03]"
                  onClick={() =>
                    store.openTable(
                      tab.sessionId!,
                      tab.scope ?? {},
                      o.name,
                    )
                  }
                >
                  <td className="flex items-center gap-2 px-3 py-1.5">
                    <Table2 size={12} className="text-slate-500" />
                    <span className="text-slate-800">{o.name}</span>
                  </td>
                  <td className="px-3 py-1.5 text-right text-slate-600 tabular-nums">
                    {o.rows_estimate?.toLocaleString() ?? ""}
                  </td>
                  <td className="px-3 py-1.5 font-code text-[12px] text-slate-500">
                    {o.updated_at ?? ""}
                  </td>
                  <td className="truncate px-3 py-1.5 text-slate-500">
                    {o.comment ?? ""}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Empty state
// ---------------------------------------------------------------------------

function EmptyState() {
  const store = useStore();
  const hasSessions = store.sessions.length > 0;
  return (
    <div className="grid flex-1 place-items-center bg-surface">
      <div className="max-w-[400px] text-center">
        <div className="mx-auto grid h-10 w-10 place-items-center rounded-[10px] bg-accent text-[17px] font-semibold text-white">
          M
        </div>
        <h2 className="mt-4 text-[20px] font-semibold tracking-tight text-slate-900">
          Welcome to MieoSQL
        </h2>
        <p className="mt-1.5 text-[13px] text-slate-500">
          {hasSessions
            ? "Open a new query, or double-click a table in the sidebar to browse its data."
            : "Connect your database to get started."}
        </p>
        {!hasSessions && (
          <p className="mt-3 text-[12px] text-slate-400">
            PostgreSQL · MySQL · MariaDB · SQLite
          </p>
        )}
        <div className="mt-5 flex flex-col items-center gap-1">
          <button
            className="flex h-8 items-center gap-1.5 rounded-md bg-accent px-4 text-[13px] font-medium text-white hover:bg-accent-hover"
            onClick={() => store.setConnectionModal(true)}
          >
            <Plus size={14} strokeWidth={1.8} />
            New Connection
          </button>
          <button
            className="mt-1 flex h-7 items-center gap-1.5 rounded-md px-3 text-[12.5px] text-accent hover:bg-accent/10"
            onClick={() => store.newQueryTab()}
          >
            <FileCode2 size={13} strokeWidth={1.8} />
            New Query
            <span className="font-code text-[11px] text-slate-400">⌘N</span>
          </button>
        </div>
        <p className="mt-6 font-code text-[11px] text-slate-400">
          ⌘Enter run · double-click a cell for the full value
        </p>
      </div>
    </div>
  );
}
