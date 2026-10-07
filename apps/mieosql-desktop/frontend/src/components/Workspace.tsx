import { useEffect, useMemo, useRef, useState } from "react";
import {
  Braces,
  CirclePlay,
  Eraser,
  FileCode2,
  GitBranch,
  ListTree,
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
    void store.runQuery(tab.id, text);
  };

  const onEditorChange = (text: string) => {
    if (changeTimer.current) window.clearTimeout(changeTimer.current);
    changeTimer.current = window.setTimeout(() => {
      const id = useStore.getState().activeTabId;
      if (id) useStore.getState().setSql(id, text);
    }, 350);
  };

  return (
    <section className="flex min-w-0 flex-1 flex-col bg-white">
      {/* Tab bar */}
      <div className="flex h-[30px] shrink-0 items-end gap-0.5 overflow-x-auto border-b border-[#cbd5e1] bg-[#e2e8f0] px-2 pt-1">
        {store.tabs.map((tab) => (
          <TabButton key={tab.id} tab={tab} active={tab.id === store.activeTabId} />
        ))}
        <button
          className="mb-0.5 rounded p-1 text-slate-500 hover:bg-white/70 hover:text-slate-700"
          title="New query tab"
          onClick={() => store.newQueryTab()}
        >
          <FileCode2 size={13} />
        </button>
      </div>

      {!activeTab && <EmptyState />}

      {activeTab?.kind === "query" && (
        <div className="flex min-h-0 flex-1 flex-col">
          <QueryToolbar
            tab={activeTab}
            onRun={runActive}
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
          <div className="flex h-[36px] shrink-0 items-center justify-between border-b border-slate-200 bg-[#f1f5f9] px-2">
            <div className="flex items-center gap-2 text-[11px]">
              <Table2 size={13} className="text-sky-500" />
              <span className="font-semibold text-slate-700">
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
    </section>
  );
}

function TabButton({ tab, active }: { tab: Tab; active: boolean }) {
  const store = useStore();
  const icon =
    tab.kind === "query" ? (
      <FileCode2 size={11} className="text-orange-500" />
    ) : tab.kind === "table" ? (
      <Table2 size={11} className="text-sky-500" />
    ) : (
      <ListTree size={11} className="text-emerald-600" />
    );
  return (
    <div
      className={`flex max-w-[220px] cursor-pointer items-center gap-1.5 rounded-t border border-b-0 px-2.5 py-1 ${
        active
          ? "border-[#cbd5e1] bg-white font-medium text-slate-800 shadow-[0_-1px_2px_rgba(0,0,0,0.04)]"
          : "border-transparent bg-[#e9edf2] text-slate-500 hover:bg-white/60"
      }`}
      onClick={() => store.setActiveTab(tab.id)}
      title={tab.title}
    >
      {tab.running ? (
        <span className="h-2 w-2 animate-pulse rounded-full bg-sky-500" />
      ) : (
        icon
      )}
      <span className="truncate text-[11px]">{tab.title}</span>
      {tab.results.length > 0 && !tab.running && (
        <span className="text-[9px] text-slate-400">
          {tab.results
            .reduce((acc, r) => acc + r.rows.length, 0)
            .toLocaleString()}
        </span>
      )}
      <button
        className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-700"
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
  onClear,
}: {
  tab: Tab;
  onRun: () => void;
  onClear: () => void;
}) {
  const store = useStore();
  const session = useStore((s) => s.sessions.find((x) => x.id === tab.sessionId));
  const runningResult = tab.results.find((r) => r.status === "running");

  const toolBtn =
    "flex items-center gap-1 rounded px-2 py-1 text-[11px] text-slate-700 hover:bg-white disabled:cursor-not-allowed disabled:text-slate-300";

  return (
    <div className="flex h-[36px] shrink-0 items-center justify-between border-b border-slate-200 bg-[#f1f5f9] px-2">
      <div className="flex items-center gap-1">
        <div className="inline-flex overflow-hidden rounded shadow-sm">
          <button
            id="run-btn"
            className="flex items-center gap-1.5 bg-sky-600 px-2.5 py-1 text-[11px] font-medium text-white transition hover:bg-sky-700 active:scale-95 disabled:opacity-50"
            onClick={onRun}
            disabled={!tab.sessionId || !!runningResult}
            title="Run query (Ctrl+Enter)"
          >
            <CirclePlay size={13} />
            Run
          </button>
          <button
            className="flex items-center gap-1.5 bg-rose-100 px-2 py-1 text-[11px] font-medium text-rose-600 transition hover:bg-rose-200 disabled:opacity-40"
            onClick={() => runningResult && store.cancelQuery(runningResult.queryId)}
            disabled={!runningResult}
            title="Stop query"
          >
            <Square size={11} />
            Stop
          </button>
        </div>

        <div className="mx-1 h-4 w-px bg-slate-300" />

        {session && (
          <>
            <button
              className={toolBtn}
              disabled={session.in_transaction}
              title="Begin transaction"
              onClick={() => void store.beginTransaction(session.id)}
            >
              <GitBranch size={13} className="text-emerald-600" />
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
            <div className="mx-1 h-4 w-px bg-slate-300" />
          </>
        )}

        <button className={toolBtn} disabled title="Explain plan — roadmap P1">
          <ListTree size={13} className="text-sky-600" />
          Explain
        </button>
        <button className={toolBtn} disabled title="Format SQL — roadmap P1">
          <Wand2 size={13} className="text-teal-600" />
          Beautify
        </button>
        <button
          className={toolBtn}
          disabled
          title="Visual query builder — roadmap P3"
        >
          <Braces size={13} className="text-violet-500" />
          Builder
        </button>
        <div className="mx-1 h-4 w-px bg-slate-300" />
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
        <div className="flex items-center gap-1.5 rounded bg-white px-2 py-0.5 text-[10px] ring-1 ring-slate-200">
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
    <label className="flex items-center gap-1 rounded bg-white px-2 py-0.5 text-[10px] ring-1 ring-slate-200">
      <span className="text-slate-400">Limit:</span>
      <select
        className="border-none bg-transparent p-0 text-[10px] font-medium text-sky-700 focus:outline-none"
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
        className="group flex h-[6px] shrink-0 cursor-row-resize items-center justify-center bg-[#e2e8f0] transition-colors hover:bg-sky-200"
        onMouseDown={() => {
          draggingRef.current = true;
          document.body.style.cursor = "row-resize";
        }}
      >
        <div className="h-1 w-8 rounded-full bg-slate-400 group-hover:bg-sky-500" />
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
      <div className="flex h-[30px] shrink-0 items-center justify-between border-b border-slate-200 bg-[#f1f5f9] px-2">
        <div className="flex items-center gap-2 text-[11px]">
          <button
            className="rounded p-1 text-slate-500 hover:bg-white"
            title="Reload"
            onClick={() => void load()}
          >
            ↻
          </button>
          <input
            className="w-48 rounded bg-white px-2 py-0.5 text-[11px] ring-1 ring-slate-200 focus:outline-none"
            placeholder="Filter objects…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <span className="text-[10px] text-slate-500">
          {shown.length.toLocaleString()} object(s)
        </span>
      </div>
      <div className="custom-scrollbar flex-1 overflow-auto bg-white">
        {error && <div className="p-4 text-[11px] text-rose-600">{error}</div>}
        {!objects && !error && (
          <div className="p-4 text-[11px] text-slate-400">Loading objects…</div>
        )}
        {objects && (
          <table className="w-full border-collapse text-left text-[11px]">
            <thead className="sticky top-0 bg-[#f1f5f9] shadow-sm">
              <tr className="text-slate-600">
                <th className="border-b border-slate-200 px-3 py-1 font-medium">
                  Name
                </th>
                <th className="w-[110px] border-b border-slate-200 px-3 py-1 text-right font-medium">
                  Rows
                </th>
                <th className="w-[180px] border-b border-slate-200 px-3 py-1 font-medium">
                  Modified
                </th>
                <th className="border-b border-slate-200 px-3 py-1 font-medium">
                  Comment
                </th>
              </tr>
            </thead>
            <tbody>
              {shown.map((o) => (
                <tr
                  key={`${o.schema ?? ""}.${o.name}`}
                  className="cursor-pointer hover:bg-sky-50"
                  onClick={() =>
                    store.openTable(
                      tab.sessionId!,
                      tab.scope ?? {},
                      o.name,
                    )
                  }
                >
                  <td className="flex items-center gap-2 px-3 py-[3px]">
                    <Table2 size={11} className="text-sky-500" />
                    <span className="text-slate-800">{o.name}</span>
                  </td>
                  <td className="px-3 py-[3px] text-right tabular-nums text-slate-600">
                    {o.rows_estimate?.toLocaleString() ?? ""}
                  </td>
                  <td className="px-3 py-[3px] font-code text-[10px] text-slate-500">
                    {o.updated_at ?? ""}
                  </td>
                  <td className="truncate px-3 py-[3px] text-slate-500">
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
    <div className="grid flex-1 place-items-center bg-[#f8fafc]">
      <div className="max-w-[420px] text-center">
        <div className="mx-auto mb-3 grid h-12 w-12 place-items-center rounded-xl bg-sky-600 text-xl font-black text-white shadow-lg">
          M
        </div>
        <h2 className="text-[15px] font-semibold text-slate-800">
          Welcome to MieoSQL
        </h2>
        <p className="mt-1 text-[11px] leading-relaxed text-slate-500">
          {hasSessions
            ? "Open a new query tab, or double-click a table in the connection tree to browse its data."
            : "Create a connection to get started — PostgreSQL, MySQL, MariaDB and SQLite are supported."}
        </p>
        <div className="mt-4 flex justify-center gap-2">
          <button
            className="rounded bg-sky-600 px-3 py-1.5 text-[11px] font-medium text-white hover:bg-sky-700"
            onClick={() => store.setConnectionModal(true)}
          >
            New Connection…
          </button>
          <button
            className="rounded border border-slate-300 bg-white px-3 py-1.5 text-[11px] text-slate-700 hover:bg-slate-100"
            onClick={() => store.newQueryTab()}
          >
            New Query
          </button>
        </div>
        <p className="mt-4 font-code text-[10px] text-slate-400">
          Ctrl+Enter run · Ctrl+N new query · double-click cell = full value
        </p>
      </div>
    </div>
  );
}
