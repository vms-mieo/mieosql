import { useEffect, useMemo, useRef, useState } from "react";
import {
  Braces,
  ChevronLeft,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  CirclePlay,
  Eraser,
  FileCode2,
  GitBranch,
  History,
  Keyboard,
  Layers,
  ListTree,
  Loader2,
  Plus,
  Plug,
  RefreshCw,
  Save,
  Square,
  Table2,
  Wand2,
  Wrench,
  X,
} from "lucide-react";

import type { Engine, ObjectRef } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { confirmDialog } from "../lib/dialog";
import { ENGINE_META } from "../lib/engines";
import { autofixSql, type SqlFix } from "../lib/sql-fix";
import { getSchema } from "../lib/sql-insight";
import type { Tab } from "../lib/store";
import { useStore } from "../lib/store";
import { FeatureHost } from "../features/registry";
import { EditorPane, type EditorHandle } from "./EditorPane";
import { AppLogo } from "./AppLogo";
import { EditorScopePickers } from "./EditorScopePickers";
import { EngineGlyph } from "./EngineGlyph";
import { ResultPanel } from "./ResultPanel";
import { SqlFixDialog } from "./SqlFixDialog";

// ---------------------------------------------------------------------------
// Workspace: tab bar + per-tab content (editor, table grid, object browser)
// ---------------------------------------------------------------------------

export function Workspace() {
  const store = useStore();
  const activeTab = store.tabs.find((t) => t.id === store.activeTabId);
  const editorRef = useRef<EditorHandle | null>(null);
  const prevTabIdRef = useRef<string | null>(null);
  const changeTimer = useRef<number | null>(null);
  const [fixDialog, setFixDialog] = useState<{
    original: string;
    fixed: string;
    fixes: SqlFix[];
    notes: string[];
  } | null>(null);

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
    const gotoOffset = (e: Event) => {
      const off = (e as CustomEvent<number>).detail;
      if (typeof off === "number") editorRef.current?.revealOffset(off);
    };
    const fix = () => {
      const view = editorRef.current;
      if (!view) return;
      const text = view.getText();
      if (!text.trim()) return;
      const state = useStore.getState();
      const active = state.tabs.find((t) => t.id === state.activeTabId);
      const session = state.sessions.find((s) => s.id === active?.sessionId);
      const engine = session?.engine ?? null;

      const show = (result: ReturnType<typeof autofixSql>) => {
        if (result.fixes.length === 0) {
          state.toast(
            "info",
            result.notes[0] ?? "No common syntax issue found — nothing to fix.",
          );
          return;
        }
        setFixDialog({ original: text, ...result });
      };

      const sessionId = active?.sessionId;
      const wantsIdentifiers =
        /\b(FROM|JOIN|UPDATE|INTO|USING|TABLE)\b/i.test(text);
      if (!session || !sessionId || !wantsIdentifiers) {
        show(autofixSql(engine, text));
        return;
      }

      // Load table/schema names so identifier typos (FROM userz → FROM users)
      // are repaired too. Cached by sql-insight; any failure falls back to the
      // keyword-only heuristics.
      void (async () => {
        const scope = active.scope ?? null;
        const [schema, names] = await Promise.all([
          getSchema(sessionId, scope),
          (async (): Promise<string[]> => {
            try {
              if (session.capabilities.list_schemas) {
                const list = await api.listSchemas(
                  sessionId,
                  scope?.database ?? null,
                );
                return list.map((s) => s.name);
              }
              if (session.capabilities.list_databases) {
                const list = await api.listDatabases(sessionId);
                return list.map((d) => d.name);
              }
            } catch {
              /* identifier hints are optional */
            }
            return [];
          })(),
        ]);
        show(
          autofixSql(engine, text, {
            tables: schema?.list.map((t) => t.name) ?? [],
            schemas: names,
          }),
        );
      })();
    };
    window.addEventListener("mieosql:clear-editor", clear);
    window.addEventListener("mieosql:focus-editor", focus);
    window.addEventListener("mieosql:fix-sql", fix);
    window.addEventListener("mieosql:goto-offset", gotoOffset);
    return () => {
      window.removeEventListener("mieosql:clear-editor", clear);
      window.removeEventListener("mieosql:focus-editor", focus);
      window.removeEventListener("mieosql:fix-sql", fix);
      window.removeEventListener("mieosql:goto-offset", gotoOffset);
    };
  }, []);

  const activeSession = useStore((s) =>
    s.sessions.find((x) => x.id === activeTab?.sessionId),
  );

  const runActive = () => {
    const tab = activeTab;
    if (!tab || tab.kind !== "query") return;
    // Beekeeper-style: a non-empty selection runs instead of the whole
    // script (the tab SQL is left untouched in that case).
    const selected = editorRef.current?.getSelection()?.trim() ?? "";
    if (selected) {
      void store.requestRun(tab.id, selected);
      return;
    }
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
    <section className="flex min-w-0 flex-1 flex-col bg-window">
      {/* Tab bar */}
      <div className="flex h-9 shrink-0 items-center gap-1 overflow-x-auto bg-sidebar px-2">
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
          <TxBanner sessionId={activeTab.sessionId} />
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
            onFix={() => window.dispatchEvent(new CustomEvent("mieosql:fix-sql"))}
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
          <TxBanner sessionId={activeTab.sessionId} />
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
            <div className="flex items-center gap-2">
              <TablePagination
                tab={activeTab}
                engine={activeSession?.engine ?? null}
              />
              <LimitSelect />
            </div>
          </div>
          <ResultPanel tab={activeTab} />
        </div>
      )}

      {activeTab?.kind === "objects" && <ObjectsView tab={activeTab} />}

      {activeTab?.kind === "feature" && <FeatureHost tab={activeTab} />}

      {fixDialog && (
        <SqlFixDialog
          original={fixDialog.original}
          fixed={fixDialog.fixed}
          fixes={fixDialog.fixes}
          notes={fixDialog.notes}
          onClose={() => setFixDialog(null)}
          onApply={() => {
            const view = editorRef.current;
            if (view) view.setText(fixDialog.fixed);
            const id = useStore.getState().activeTabId;
            if (id) useStore.getState().setSql(id, fixDialog.fixed);
            useStore
              .getState()
              .toast(
                "success",
                `Applied ${fixDialog.fixes.length} fix${
                  fixDialog.fixes.length > 1 ? "es" : ""
                }`,
              );
            setFixDialog(null);
          }}
        />
      )}
    </section>
  );
}

function TabButton({ tab, active }: { tab: Tab; active: boolean }) {
  const store = useStore();
  const session = store.sessions.find((s) => s.id === tab.sessionId);
  const profileColor = store.profiles.find(
    (p) => p.id === session?.profile_id,
  )?.color;
  // Query tabs show their target database (Sequel Ace-style); the colored
  // dot mirrors the connection accent so prod/dev are told apart at a glance.
  const dbSuffix =
    tab.kind === "query" && tab.scope?.database ? ` · ${tab.scope.database}` : "";
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
      title={`${tab.title}${dbSuffix}`}
    >
      {tab.running ? (
        <span className="h-2 w-2 animate-pulse rounded-full bg-accent" />
      ) : (
        icon
      )}
      {profileColor && (
        <span
          className="h-2 w-2 shrink-0 rounded-full"
          style={{ backgroundColor: profileColor }}
          title={session?.name}
        />
      )}
      <span className="truncate text-[13px]">
        {tab.title}
        {dbSuffix && <span className="text-slate-400">{dbSuffix}</span>}
      </span>
      {tab.results.length > 0 && !tab.running && (
        <span className="font-num text-[11.5px] text-slate-400">
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
// Transaction banner (DBeaver-style): visible only when in_transaction
// ---------------------------------------------------------------------------

function TxBanner({ sessionId }: { sessionId: string | null }) {
  const inTx = useStore((s) =>
    s.sessions.some((x) => x.id === sessionId && x.in_transaction),
  );
  const commit = useStore((s) => s.commitTransaction);
  const rollback = useStore((s) => s.rollbackTransaction);
  if (!sessionId || !inTx) return null;
  const onRollback = () => {
    void (async () => {
      if (
        await confirmDialog("Discard all uncommitted changes (rollback)?", {
          title: "Rollback",
          kind: "warning",
        })
      ) {
        void rollback(sessionId);
      }
    })();
  };
  return (
    <div className="flex h-8 shrink-0 items-center justify-between border-b border-amber-200/70 bg-amber-50 px-3">
      <span className="flex items-center gap-1.5 text-[12px] text-amber-700">
        <span className="inline-block h-1.5 w-1.5 rounded-full bg-amber-500" />
        <span className="font-medium">Uncommitted changes</span>
        <span className="text-amber-600/80">
          — transaction open, auto-commit off
        </span>
      </span>
      <span className="flex items-center gap-1.5">
        <button
          className="flex h-6 items-center rounded-md bg-accent px-2 text-[12px] font-medium text-white hover:bg-accent-hover"
          title="Commit transaction"
          onClick={() => void commit(sessionId)}
        >
          Commit
        </button>
        <button
          className="flex h-6 items-center rounded-md border border-amber-300 bg-white px-2 text-[12px] text-slate-700 hover:bg-amber-100/60"
          title="Rollback transaction"
          onClick={onRollback}
        >
          Rollback
        </button>
      </span>
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
  onFix,
  onClear,
}: {
  tab: Tab;
  onRun: () => void;
  onExplain: () => void;
  onBeautify: () => void;
  onFix: () => void;
  onClear: () => void;
}) {
  const store = useStore();
  const session = useStore((s) => s.sessions.find((x) => x.id === tab.sessionId));
  const runningResult = tab.results.find((r) => r.status === "running");

  const toolBtn =
    "flex h-7 items-center gap-1 rounded-md px-2 text-[12.5px] text-slate-700 hover:bg-slate-900/[0.05] disabled:pointer-events-none disabled:text-slate-300";

  return (
    <div
      className="flex h-10 shrink-0 items-center justify-between gap-2 border-b border-slate-200/80 bg-white px-3"
    >
      <div className="custom-scrollbar flex min-w-[210px] flex-1 items-center gap-1 overflow-x-auto">
        <button
          id="run-btn"
          className="flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[12.5px] font-medium text-white transition-colors hover:bg-accent-hover active:scale-[0.98] disabled:opacity-40"
          onClick={onRun}
          disabled={!tab.sessionId || !!runningResult}
          title="Run selection, or whole query (⌘Enter)"
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
        {!tab.sessionId && (
          <span className="ml-1 shrink-0 text-[12px] font-medium text-amber-600">
            No connection — pick one on the right
          </span>
        )}

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
          title="Fix common SQL syntax mistakes (SELET → SELECT, missing commas, …)"
          onClick={onFix}
        >
          <Wrench size={13} className="text-slate-500" />
          Fix
        </button>
        <button
          className={toolBtn}
          title="Visual query builder"
          onClick={() => store.openFeatureDialog("builder")}
        >
          <Braces size={13} className="text-slate-500" />
          Builder
        </button>
        <button
          className={toolBtn}
          title="Query history — search and re-run past queries"
          onClick={() => store.openFeatureDialog("history")}
        >
          <History size={13} className="text-slate-500" />
          History
        </button>
        <button
          className={toolBtn}
          title="Keyboard shortcuts"
          onClick={() => store.openFeatureDialog("shortcuts")}
        >
          <Keyboard size={13} className="text-slate-500" />
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

      <div className="flex min-w-0 shrink items-center gap-2">
        <EditorScopePickers tab={tab} />
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
    <label className="flex h-7 shrink-0 items-center gap-1 rounded-md bg-slate-900/[0.04] px-2 text-[11.5px]">
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
// Table pagination — next / previous / last page of a table preview. Page
// size is the global Limit; the "last" button runs COUNT(*) on first use.
// ---------------------------------------------------------------------------

function TablePagination({ tab, engine }: { tab: Tab; engine: Engine | null }) {
  const store = useStore();
  const limit = store.limit;
  const offset = tab.offset ?? 0;
  const result =
    tab.results[tab.activeResult] ?? tab.results[tab.results.length - 1];
  const rows = result?.rows.length ?? 0;
  const sqlEngine = engine !== null && ENGINE_META[engine].kind === "sql";
  if (!sqlEngine || limit <= 0) return null;

  const page = Math.floor(offset / limit) + 1;
  const total = tab.totalRows ?? null;
  const totalPages =
    total !== null ? Math.max(1, Math.ceil(total / limit)) : null;
  const atLastKnown = total !== null && offset + limit >= total;
  const canPrev = !tab.running && offset > 0;
  const canNext = !tab.running && rows >= limit && !atLastKnown;
  const canLast = !tab.running && !tab.counting && !atLastKnown;

  const btn =
    "grid h-7 w-7 place-items-center rounded-md text-slate-500 transition-colors hover:bg-slate-900/[0.05] hover:text-slate-700 disabled:pointer-events-none disabled:text-slate-300";

  return (
    <div className="flex shrink-0 items-center gap-0.5">
      <button
        className={btn}
        title="First page"
        disabled={!canPrev}
        onClick={() => void store.previewPage(tab.id, 0)}
      >
        <ChevronsLeft size={13} strokeWidth={1.8} />
      </button>
      <button
        className={btn}
        title="Previous page"
        disabled={!canPrev}
        onClick={() => void store.previewPage(tab.id, Math.max(0, offset - limit))}
      >
        <ChevronLeft size={13} strokeWidth={1.8} />
      </button>
      <span className="min-w-[64px] text-center font-num text-[11.5px] text-slate-500">
        Page {page.toLocaleString()}
        {totalPages !== null ? ` / ${totalPages.toLocaleString()}` : ""}
      </span>
      <button
        className={btn}
        title="Next page"
        disabled={!canNext}
        onClick={() => void store.previewPage(tab.id, offset + limit)}
      >
        <ChevronRight size={13} strokeWidth={1.8} />
      </button>
      <button
        className={btn}
        title="Last page (runs COUNT(*))"
        disabled={!canLast}
        onClick={() => void store.previewLastPage(tab.id)}
      >
        {tab.counting ? (
          <Loader2 size={13} className="animate-spin" />
        ) : (
          <ChevronsRight size={13} strokeWidth={1.8} />
        )}
      </button>
      {rows > 0 && (
        <span className="ml-1 hidden font-num text-[11px] text-slate-400 xl:inline">
          {(offset + 1).toLocaleString()}–{(offset + rows).toLocaleString()}
          {total !== null ? ` of ${total.toLocaleString()}` : ""}
        </span>
      )}
    </div>
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

  const selected = store.selected;
  const selectedName =
    selected &&
    selected.sessionId === tab.sessionId &&
    (selected.scope.database ?? null) === (tab.scope?.database ?? null) &&
    (selected.scope.schema ?? null) === (tab.scope?.schema ?? null)
      ? selected.table
      : null;

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
              {shown.map((o) => {
                const sel = o.name === selectedName;
                return (
                  <tr
                    key={`${o.schema ?? ""}.${o.name}`}
                    className={`cursor-default border-b border-slate-200/70 ${
                      sel ? "bg-accent text-white" : "hover:bg-slate-900/[0.03]"
                    }`}
                    onClick={() => {
                      if (
                        o.kind === "function" ||
                        o.kind === "procedure" ||
                        o.kind === "trigger" ||
                        o.kind === "sequence"
                      ) {
                        void api
                          .objectDdl(tab.sessionId!, tab.scope ?? {}, o.kind, o.name)
                          .then((ddl) => {
                            if (ddl) store.newQueryTab(tab.sessionId!, ddl);
                            else
                              store.toast(
                                "info",
                                "DDL is not available for this object yet",
                              );
                          })
                          .catch((err) => store.toast("error", errorText(err)));
                        return;
                      }
                      store.openTable(
                        tab.sessionId!,
                        tab.scope ?? {},
                        o.name,
                      );
                    }}
                  >
                    <td className="flex items-center gap-2 px-3 py-1.5">
                      <Table2
                        size={12}
                        className={sel ? "text-white" : "text-sky-600"}
                      />
                      <span className={sel ? "text-white" : "text-slate-800"}>
                        {o.name}
                      </span>
                    </td>
                    <td
                      className={`font-num px-3 py-1.5 text-right ${
                        sel ? "text-white/85" : "text-slate-600"
                      }`}
                    >
                      {o.rows_estimate?.toLocaleString() ?? ""}
                    </td>
                    <td
                      className={`px-3 py-1.5 font-code text-[12px] ${
                        sel ? "text-white/85" : "text-slate-500"
                      }`}
                    >
                      {o.updated_at ?? ""}
                    </td>
                    <td
                      className={`truncate px-3 py-1.5 ${
                        sel ? "text-white/85" : "text-slate-500"
                      }`}
                    >
                      {o.comment ?? ""}
                    </td>
                  </tr>
                );
              })}
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
  const engines: { label: string; engine: Engine }[] = [
    { label: "PostgreSQL", engine: "postgres" },
    { label: "MySQL", engine: "mysql" },
    { label: "MariaDB", engine: "mariadb" },
    { label: "SQLite", engine: "sqlite" },
    { label: "Kafka", engine: "kafka" },
  ];
  return (
    <div className="grid flex-1 place-items-center">
      <div className="max-w-[420px] text-center">
        <div className="mx-auto w-fit">
          <AppLogo size={44} radius={12} />
        </div>
        <h2 className="mt-4 text-[20px] font-semibold tracking-tight text-slate-900">
          Welcome to MieoSQL
        </h2>
        <p className="mt-1.5 text-[13px] text-slate-500">
          {hasSessions
            ? "Open a new query, or double-click a table in the sidebar to browse its data."
            : "Connect your data and get started."}
        </p>
        {!hasSessions && (
          <div className="mt-4 flex flex-wrap items-center justify-center gap-x-4 gap-y-1.5 text-[11.5px] text-slate-500">
            {engines.map((e) => (
              <span key={e.label} className="flex items-center gap-1.5">
                <EngineGlyph engine={e.engine} size={11} />
                {e.label}
              </span>
            ))}
          </div>
        )}
        <div className="mt-5 flex flex-col items-center gap-1">
          <button
            className="flex h-9 items-center gap-1.5 rounded-md bg-accent px-5 text-[13px] font-medium text-white hover:bg-accent-hover"
            onClick={() => store.setConnectionModal(true)}
          >
            <Plug size={14} strokeWidth={1.6} />
            New Connection
          </button>
          <button
            className="mt-1 flex h-7 items-center gap-1.5 rounded-md px-3 text-[12.5px] text-accent hover:bg-accent/10"
            onClick={() => store.newQueryTab()}
          >
            <FileCode2 size={13} strokeWidth={1.6} />
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
