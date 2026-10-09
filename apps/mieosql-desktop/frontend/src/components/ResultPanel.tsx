import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Copy,
  Crosshair,
  Database,
  FileDown,
  FileJson,
  FileSpreadsheet,
  FileText,
  Filter,
  Grid3x3,
  List,
  Pin,
  Plus,
  RotateCw,
  ScrollText,
  Search,
  Terminal,
  Trash2,
  Undo2,
  Wrench,
  X,
} from "lucide-react";

import type { ApiError, ColumnDef, ExportFormat, Value } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { copyText, plainText } from "../lib/format";
import type { LogMessage, ResultSet, Tab } from "../lib/store";
import { EXPORT_FORMATS, exportFormatLabel, useStore } from "../lib/store";
import { ResultGrid, buildCopyText } from "./ResultGrid";
import type { CopyFormat, FindCommand, GridApi, GridRange } from "./ResultGrid";
import { RecordView } from "./RecordView";
import type {
  DraftRow,
  EditApplyResult,
  GridCell,
  GridEditRequest,
  GridSelection,
  RowChanges,
  RowEditMap,
} from "./ResultGrid";
import { CellViewer } from "./CellViewer";

type SubTab = "result" | "messages" | "status";

/** Cached `table_columns` definition for a table tab, keyed by scope+table. */
type TableDefs = { columns: ColumnDef[]; failed: boolean };

const editBtnClass =
  "flex h-7 shrink-0 items-center gap-1 whitespace-nowrap rounded-md px-2 text-[12.5px] font-medium text-slate-600 hover:bg-slate-900/[0.05] disabled:opacity-40";
const editIconClass =
  "grid h-7 w-7 shrink-0 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-rose-600 disabled:opacity-40";
const applyBtnClass =
  "flex h-7 shrink-0 items-center gap-1 whitespace-nowrap rounded-md bg-accent px-2.5 text-[12.5px] font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-40";

/**
 * A committed editor value is always text (the backend binds text and the DB
 * casts it). Treat it as a no-op when the plain text matches what the cell
 * already showed, so retyping "42" does not leave an amber "changed" mark.
 */
function sameCellValue(original: Value | undefined, next: Value): boolean {
  if (original === undefined || original.t === "null") return next.t === "null";
  if (next.t === "null") return false;
  return next.t === "text"
    ? plainText(original) === next.v
    : plainText(original) === plainText(next);
}

export function ResultPanel({ tab }: { tab: Tab }) {
  const store = useStore();
  const [subTab, setSubTab] = useState<SubTab>("result");
  const [filter, setFilter] = useState("");
  const deferredFilter = useDeferredValue(filter);
  const [viewer, setViewer] = useState<{
    row: number;
    col: number;
    displayRow?: number;
  } | null>(null);
  const [exportOpen, setExportOpen] = useState(false);
  const [viewMode, setViewMode] = useState<"grid" | "form">("grid");
  const [formRow, setFormRow] = useState(0);

  // ---- inline editing buffers (kept here so they survive sub-tab switches).
  // `edits`/`drafts` are keyed by ORIGINAL row/coordinate space; sorting and
  // filtering never re-key them (the grid translates via its `indices` map).
  const [defsByKey, setDefsByKey] = useState<Map<string, TableDefs>>(
    () => new Map(),
  );
  const [edits, setEdits] = useState<RowEditMap>(() => new Map());
  const [deletedRows, setDeletedRows] = useState<Set<number>>(() => new Set());
  const [drafts, setDrafts] = useState<DraftRow[]>([]);
  const [gridSel, setGridSel] = useState<GridSelection | null>(null);
  const [applying, setApplying] = useState(false);
  const [applyErrors, setApplyErrors] = useState<string[] | null>(null);
  const [errorsOpen, setErrorsOpen] = useState(false);
  const gridApiRef = useRef<GridApi | null>(null);
  // Rectangular range size reported by the grid (cell count, null = none).
  const [rangeCells, setRangeCells] = useState<number | null>(null);
  // Numeric aggregates over the current range (DbGate-style selection math).
  const [rangeStats, setRangeStats] = useState<{
    numeric: number;
    sum: number;
    avg: number;
  } | null>(null);
  // Find bar state (the grid owns matching/highlighting/navigation).
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const deferredFind = useDeferredValue(findQuery);
  const [findTotal, setFindTotal] = useState(0);
  const [findActive, setFindActive] = useState(0);
  const [findCmd, setFindCmd] = useState<FindCommand | null>(null);
  const findInputRef = useRef<HTMLInputElement>(null);

  const result: ResultSet | undefined = tab.results[tab.activeResult];
  const running = result?.status === "running";

  // Switch to Result after a new run starts.
  const lastResultCount = useRef(tab.results.length);
  useEffect(() => {
    if (tab.results.length !== lastResultCount.current) {
      lastResultCount.current = tab.results.length;
      setSubTab("result");
      setFilter("");
      setFindOpen(false);
      setFindQuery("");
      setFindTotal(0);
      setFindActive(0);
      setRangeCells(null);
      setRangeStats(null);
    }
  }, [tab.results.length]);

  const matchIndices = useMemo(() => {
    if (!result || !deferredFilter.trim()) return null;
    const q = deferredFilter.toLowerCase();
    const out: number[] = [];
    for (let i = 0; i < result.rows.length; i++) {
      const row = result.rows[i];
      if (!row) continue;
      if (row.some((v) => plainText(v).toLowerCase().includes(q))) out.push(i);
    }
    return out;
  }, [result, deferredFilter]);

  // ------------------------------------------------------------- editability
  // Inline editing is only offered for table tabs whose base table has a
  // primary key that is present in the result columns (the preview is
  // SELECT *, so this normally holds) and only when no filter is set.
  const isTableTab =
    tab.kind === "table" && !!tab.sessionId && !!tab.scope && !!tab.table;
  const defsKey = isTableTab
    ? `${tab.sessionId}|${tab.scope?.database ?? ""}|${tab.scope?.schema ?? ""}|${tab.table}`
    : null;

  // Fetch (and cache) the table definition once per scope+table.
  useEffect(() => {
    if (!defsKey || !tab.sessionId || !tab.scope || !tab.table) return;
    if (defsByKey.has(defsKey)) return;
    const sessionId = tab.sessionId;
    const scope = tab.scope;
    const table = tab.table;
    let cancelled = false;
    void api.tableColumns(sessionId, scope, table).then(
      (columns) => {
        if (!cancelled) {
          setDefsByKey((prev) => new Map(prev).set(defsKey, { columns, failed: false }));
        }
      },
      () => {
        // Without PK knowledge the tab stays safely read-only.
        if (!cancelled) {
          setDefsByKey((prev) => new Map(prev).set(defsKey, { columns: [], failed: true }));
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [defsKey, defsByKey, tab.sessionId, tab.scope, tab.table]);

  const tableDefs = defsKey ? (defsByKey.get(defsKey) ?? null) : null;
  const defsReady = !!tableDefs && !tableDefs.failed;
  const pkNames = useMemo(
    () =>
      defsReady && tableDefs
        ? tableDefs.columns.filter((c) => c.is_primary_key).map((c) => c.name)
        : [],
    [defsReady, tableDefs],
  );
  const pkCols = useMemo(() => {
    if (!result || pkNames.length === 0) return [] as number[];
    const byName = new Map(result.columns.map((c, i) => [c.name, i] as const));
    return pkNames
      .map((n) => byName.get(n))
      .filter((i): i is number => i !== undefined);
  }, [pkNames, result]);

  const filterActive = filter.trim().length > 0;
  const stripVisible =
    isTableTab &&
    !!result &&
    result.status === "done" &&
    !result.error &&
    !!defsKey &&
    !!tableDefs;
  const baseEditable =
    stripVisible && defsReady && pkNames.length > 0 && pkCols.length === pkNames.length;
  const editable = baseEditable && !filterActive;
  const editHint = !stripVisible
    ? null
    : tableDefs?.failed
      ? "Column info unavailable — editing disabled"
      : pkNames.length === 0
        ? "No primary key — editing disabled"
        : pkCols.length < pkNames.length
          ? "Primary key not in result — editing disabled"
          : filterActive
            ? "Clear filter to edit"
            : null;

  const editCount = useMemo(() => {
    let n = 0;
    for (const row of edits.values()) n += row.size;
    return n;
  }, [edits]);
  const newCount = useMemo(() => drafts.filter((d) => d.size > 0).length, [drafts]);
  const deletedCount = deletedRows.size;
  const pendingCount = editCount + newCount + deletedCount;

  const discardEdits = useCallback(() => {
    setEdits(new Map());
    setDeletedRows(new Set());
    setDrafts([]);
    setGridSel(null);
    setApplyErrors(null);
    setErrorsOpen(false);
  }, []);

  // A new result set (re-run, tab switch, result switch) invalidates all
  // buffers and closes the value viewer, so nothing is applied to stale rows.
  const queryId = result?.queryId ?? null;
  const lastQueryId = useRef<string | null>(null);
  useEffect(() => {
    if (lastQueryId.current === queryId) return;
    lastQueryId.current = queryId;
    discardEdits();
    setViewer(null);
    setFormRow(0);
  }, [queryId, discardEdits]);

  const handleEditCell = useCallback(
    (rowIndex: number, colIndex: number, value: Value) => {
      setEdits((prev) => {
        const original = result?.rows[rowIndex]?.[colIndex];
        const next = new Map(prev);
        const row = new Map(next.get(rowIndex) ?? []);
        if (sameCellValue(original, value)) row.delete(colIndex); // no-op edit
        else row.set(colIndex, value);
        if (row.size === 0) next.delete(rowIndex);
        else next.set(rowIndex, row);
        return next;
      });
    },
    [result],
  );

  const handleEditDraft = useCallback(
    (draftIndex: number, colIndex: number, value: Value) => {
      setDrafts((prev) =>
        prev.map((row, i) => {
          if (i !== draftIndex) return row;
          const next = new Map(row);
          // Empty input = NULL; untouched draft cells are skipped on insert.
          if (value.t === "null") next.delete(colIndex);
          else next.set(colIndex, value);
          return next;
        }),
      );
    },
    [],
  );

  const removeDraft = useCallback((draftIndex: number) => {
    setDrafts((prev) => prev.filter((_, i) => i !== draftIndex));
  }, []);

  const addDraftRow = useCallback(() => {
    if (!editable) return;
    setDrafts((prev) => [...prev, new Map<number, Value>()]);
  }, [editable]);

  const toggleDeleteRow = useCallback((rowIndex: number) => {
    setDeletedRows((prev) => {
      const next = new Set(prev);
      if (next.has(rowIndex)) next.delete(rowIndex);
      else next.add(rowIndex);
      return next;
    });
    // Cell edits on a row marked for deletion are moot; drop them.
    setEdits((prev) => {
      if (!prev.has(rowIndex)) return prev;
      const next = new Map(prev);
      next.delete(rowIndex);
      return next;
    });
  }, []);

  const handleGridSelection = useCallback((s: GridSelection | null) => {
    setGridSel(s);
  }, []);

  const deleteSelected = () => {
    if (!editable || !gridSel) return;
    if (gridSel.kind === "draft") removeDraft(gridSel.index);
    else toggleDeleteRow(gridSel.index);
  };

  // -------------------------------------------------------------- apply
  // Keys are read from the untouched result row (edits are buffered apart),
  // so updates/deletes always target the current database row.
  const buildEditRequest = (): GridEditRequest | null => {
    if (!tab.sessionId || !tab.scope || !tab.table || !result) return null;
    const colName = (ci: number) => result.columns[ci]?.name ?? `col_${ci}`;
    const nullValue: Value = { t: "null" };

    // Only touched draft cells are sent; empty cells are left to DB defaults.
    const inserts: GridCell[][] = drafts
      .filter((d) => d.size > 0)
      .map((d) =>
        [...d.entries()]
          .sort((a, b) => a[0] - b[0])
          .map(([ci, value]) => ({ column: colName(ci), value })),
      );

    const updates: RowChanges[] = [];
    for (const [rowIndex, cols] of edits) {
      if (deletedRows.has(rowIndex) || cols.size === 0) continue;
      const base = result.rows[rowIndex];
      if (!base) continue;
      updates.push({
        keys: pkCols.map((ci) => ({
          column: colName(ci),
          value: base[ci] ?? nullValue,
        })),
        changes: [...cols.entries()]
          .sort((a, b) => a[0] - b[0])
          .map(([ci, value]) => ({ column: colName(ci), value })),
      });
    }

    const deletes: GridCell[][] = [...deletedRows]
      .sort((a, b) => a - b)
      .map((rowIndex) => {
        const base = result.rows[rowIndex];
        return pkCols.map((ci) => ({
          column: colName(ci),
          value: base?.[ci] ?? nullValue,
        }));
      });

    return {
      session_id: tab.sessionId,
      scope: tab.scope,
      table: tab.table,
      inserts,
      updates,
      deletes,
    };
  };

  const applyEdits = async () => {
    if (applying || pendingCount === 0 || !baseEditable) return;
    const request = buildEditRequest();
    if (!request) return;
    setApplying(true);
    setApplyErrors(null);
    try {
      const res: EditApplyResult = await api.applyGridEdits(request);
      const applied = res.inserted + res.updated + res.deleted;
      if (res.failed > 0) {
        // Keep the buffers so the user can fix/discard and retry.
        setApplyErrors(
          res.errors.length > 0 ? res.errors : [`${res.failed} change(s) failed`],
        );
        setErrorsOpen(true);
        store.toast("error", `Apply failed: ${res.failed} change(s) not applied`);
      } else {
        store.toast(
          "success",
          `Applied ${applied} change${applied === 1 ? "" : "s"} · ${res.inserted} inserted, ${res.updated} updated, ${res.deleted} deleted · ${res.elapsed_ms} ms`,
        );
        discardEdits();
        void store.runPreview(tab.id);
      }
    } catch (err) {
      const text = errorText(err);
      setApplyErrors([text]);
      setErrorsOpen(true);
      store.toast("error", text);
    } finally {
      setApplying(false);
    }
  };

  // ⌘/Ctrl+Enter applies while the grid (or an inline editor) has focus.
  const applyRef = useRef<() => void>(() => undefined);
  applyRef.current = () => void applyEdits();
  const requestApply = useCallback(() => applyRef.current(), []);

  useEffect(() => {
    if (!baseEditable || pendingCount === 0 || applying) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
        e.preventDefault();
        applyRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [baseEditable, pendingCount, applying]);

  const rerun = () => {
    if (tab.kind === "table") void store.runPreview(tab.id);
    else if (result?.sql) void store.requestRun(tab.id, result.sql);
  };

  const copyDataAs = (kind: CopyFormat) => {
    if (!result) return;
    // A rectangular range copies just its cells (display order); otherwise
    // the full loaded result is copied.
    const data = gridApiRef.current?.getRangeData?.();
    const columns = data?.columns ?? result.columns;
    const rows = data?.rows ?? result.rows;
    if (rows.length === 0) return;
    const label =
      kind === "csv"
        ? "CSV"
        : kind === "tsv"
          ? "TSV"
          : kind === "json"
            ? "JSON lines"
            : kind === "sql"
              ? "SQL INSERTs"
              : "Markdown";
    const scope = data ? "selection" : "all rows";
    const text = buildCopyText(kind, tab.table ?? "result", columns, rows);
    void copyText(text).then((ok) =>
      store.toast(ok ? "success" : "error", ok ? `Copied ${label} (${scope})` : "Copy failed"),
    );
  };

  // Back-compat aliases for the original Copy CSV / JSON-lines entries.
  const copyData = (kind: "csv" | "json") => copyDataAs(kind);

  const handleRangeChange = useCallback((r: GridRange | null) => {
    setRangeCells(
      r
        ? (Math.abs(r.toRow - r.fromRow) + 1) * (Math.abs(r.toCol - r.fromCol) + 1)
        : null,
    );
    if (!r) {
      setRangeStats(null);
      return;
    }
    let data: { rows: Value[][] } | null = null;
    try {
      data = gridApiRef.current?.getRangeData?.() ?? null;
    } catch {
      data = null;
    }
    if (!data) {
      setRangeStats(null);
      return;
    }
    let numeric = 0;
    let sum = 0;
    for (const row of data.rows) {
      for (const v of row) {
        if (!v || v.t === "null") continue;
        const text = plainText(v).trim().replace(/,/g, "");
        if (text === "") continue;
        const n = Number(text);
        if (Number.isFinite(n)) {
          numeric += 1;
          sum += n;
        }
      }
    }
    setRangeStats(numeric > 0 ? { numeric, sum, avg: sum / numeric } : null);
  }, []);

  const handleFindMatches = useCallback((total: number, active: number) => {
    setFindTotal(total);
    setFindActive(active);
  }, []);

  const openFind = useCallback(() => {
    setFindOpen(true);
    requestAnimationFrame(() => findInputRef.current?.focus());
  }, []);

  const closeFind = useCallback(() => {
    setFindOpen(false);
  }, []);

  const stepFind = useCallback((dir: 1 | -1) => {
    setFindCmd((c) => ({ dir, n: (c?.n ?? 0) + 1 }));
  }, []);

  const messageCount = tab.messages.length;
  const errorCount = tab.messages.filter((m) => m.level === "error").length;

  // Form view browses raw `result.rows` (ignores the grid filter); clamp the
  // remembered row when the result set changes size.
  const formRowCount = result?.rows.length ?? 0;
  const safeFormRow =
    formRowCount === 0
      ? 0
      : Math.min(Math.max(0, formRow), formRowCount - 1);

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-sidebar">
      {/* sub-tab bar + timing */}
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-slate-200/80 bg-sidebar px-2 select-none">
        <div className="flex h-full items-center">
          {tab.results.map((r, i) => (
            <button
              key={r.queryId}
              title={resultTitle(
                store.sessions.find((s) => s.id === tab.sessionId)?.name,
                r.sql,
                r.startedAt,
                r.elapsedMs,
              )}
              className={`relative flex h-full items-center gap-1.5 px-3 text-[12.5px] transition-colors ${
                i === tab.activeResult && subTab === "result"
                  ? "font-medium text-slate-900"
                  : "text-slate-500 hover:text-slate-700"
              }`}
              onClick={() => {
                store.setActiveResult(tab.id, i);
                setSubTab("result");
              }}
            >
              {i === tab.activeResult && subTab === "result" && (
                <span className="absolute inset-x-2 bottom-0 h-[2px] rounded-full bg-accent" />
              )}
              <Grid3x3 size={12} strokeWidth={1.7} />
              Result {i + 1}
              <span className="text-slate-400">
                ({r.rows.length.toLocaleString()}
                {r.status === "running" ? "…" : " rows"})
              </span>
              <span
                role="button"
                title={r.pinned ? "Unpin result" : "Pin result"}
                className={`rounded p-0.5 ${
                  r.pinned
                    ? "text-accent"
                    : "text-slate-300 hover:text-slate-500"
                }`}
                onClick={(e) => {
                  e.stopPropagation();
                  store.togglePin(tab.id, i);
                }}
              >
                <Pin size={10} strokeWidth={1.8} />
              </span>
              <span
                role="button"
                title={`Close Result ${i + 1}`}
                className="rounded p-0.5 text-slate-300 hover:bg-slate-900/[0.06] hover:text-rose-500"
                onClick={(e) => {
                  e.stopPropagation();
                  store.closeResult(tab.id, i);
                }}
              >
                <X size={10} strokeWidth={2} />
              </span>
            </button>
          ))}
          <button
            className={`relative flex h-full items-center gap-1.5 px-3 text-[12.5px] transition-colors ${
              subTab === "messages"
                ? "font-medium text-slate-900 after:absolute after:inset-x-2 after:bottom-0 after:h-[2px] after:rounded-full after:bg-accent"
                : "text-slate-500 hover:text-slate-700"
            }`}
            onClick={() => setSubTab("messages")}
          >
            <Terminal size={12} strokeWidth={1.7} />
            Messages
            {messageCount > 0 && (
              <span
                className={`rounded-full px-1 text-[10.5px] font-semibold ${
                  errorCount > 0
                    ? "bg-rose-100 text-rose-700"
                    : "bg-slate-900/[0.06] text-slate-600"
                }`}
              >
                {messageCount}
              </span>
            )}
          </button>
          <button
            className={`relative flex h-full items-center gap-1.5 px-3 text-[12.5px] transition-colors ${
              subTab === "status"
                ? "font-medium text-slate-900 after:absolute after:inset-x-2 after:bottom-0 after:h-[2px] after:rounded-full after:bg-accent"
                : "text-slate-500 hover:text-slate-700"
            }`}
            onClick={() => setSubTab("status")}
          >
            <ScrollText size={12} strokeWidth={1.7} />
            Status
          </button>
          {tab.results.length > 0 && (
            <button
              className="ml-1 grid h-6 w-6 shrink-0 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.05] hover:text-rose-500"
              title="Close all results"
              onClick={() => store.closeAllResults(tab.id)}
            >
              <X size={12} strokeWidth={1.7} />
            </button>
          )}
        </div>
        <div className="flex items-center gap-2 pr-1 text-[11px] text-slate-500">
          {result && (
            <>
              {result.status === "running" ? (
                <span className="flex items-center gap-1 text-accent">
                  <RotateCw size={12} strokeWidth={1.7} className="animate-spin" />
                  running…
                </span>
              ) : (
                <span className="flex items-center gap-1">
                  <CheckCircle2
                    size={12}
                    strokeWidth={1.7}
                    className={result.error ? "text-rose-500" : "text-emerald-500"}
                  />
                  {result.elapsedMs !== null
                    ? `${result.elapsedMs} ms`
                    : result.status}
                </span>
              )}
              <span>•</span>
              <span className="text-slate-700">
                {result.rows.length.toLocaleString()} rows retrieved
              </span>
              {result.stats && result.stats.rows_affected > 0 && (
                <>
                  <span>•</span>
                  <span className="text-slate-700">
                    {result.stats.rows_affected.toLocaleString()} affected
                  </span>
                </>
              )}
              {result.truncated && (
                <>
                  <span>•</span>
                  <span className="flex items-center gap-1 text-amber-600">
                    <AlertTriangle size={12} strokeWidth={1.7} /> limit reached
                  </span>
                </>
              )}
            </>
          )}
        </div>
      </div>

      {/* toolbar */}
      <div className="flex min-h-10 shrink-0 flex-wrap items-center justify-between gap-y-1 border-b border-slate-200/80 bg-white px-3 py-1 text-[12.5px]">
        <div className="flex flex-wrap items-center gap-1">
          <button
            className="grid h-7 w-7 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700 disabled:opacity-40"
            title="Re-run"
            disabled={running || !tab.sessionId}
            onClick={rerun}
          >
            <RotateCw size={13} strokeWidth={1.7} />
          </button>
          {running && (
            <button
              className="grid h-7 w-7 place-items-center rounded-md text-rose-500 hover:bg-rose-50"
              title="Stop query"
              onClick={() => {
                if (result) void store.cancelQuery(result.queryId);
              }}
            >
              ✕
            </button>
          )}
          <div className="relative">
            <button
              className="flex h-7 shrink-0 items-center gap-1 whitespace-nowrap rounded-md px-2 text-[12.5px] font-medium text-slate-600 hover:bg-slate-900/[0.05] disabled:opacity-40"
              disabled={!result || result.rows.length === 0}
              onClick={() => setExportOpen((v) => !v)}
            >
              <FileDown size={13} strokeWidth={1.7} className="text-slate-500" />
              Export
            </button>
            {exportOpen && (
              <div className="absolute top-full left-0 z-40 mt-1 w-52 overflow-hidden rounded-md border border-slate-300/70 bg-white py-1 shadow-[0_10px_34px_rgba(0,0,0,0.16)]">
                {EXPORT_FORMATS.map((format) => (
                  <button
                    key={format}
                    className="group flex w-full items-center gap-2 px-3 py-[5px] text-left text-[13px] text-slate-800 hover:bg-accent hover:text-white"
                    disabled={!result || result.rows.length === 0}
                    onClick={() => {
                      setExportOpen(false);
                      void store.exportResult(tab.id, tab.activeResult, format);
                    }}
                  >
                    <ExportIcon format={format} />
                    Save as {exportFormatLabel(format)}…
                  </button>
                ))}
                <div className="my-1 h-px bg-slate-200" />
                <button
                  className="group flex w-full items-center gap-2 px-3 py-[5px] text-left text-[13px] text-slate-800 hover:bg-accent hover:text-white"
                  onClick={() => {
                    setExportOpen(false);
                    copyData("csv");
                  }}
                >
                  <Copy size={12} strokeWidth={1.7} className="text-slate-500 group-hover:text-white/75" /> Copy as CSV
                </button>
                <button
                  className="group flex w-full items-center gap-2 px-3 py-[5px] text-left text-[13px] text-slate-800 hover:bg-accent hover:text-white"
                  onClick={() => {
                    setExportOpen(false);
                    copyData("json");
                  }}
                >
                  <Copy size={12} strokeWidth={1.7} className="text-slate-500 group-hover:text-white/75" /> Copy as JSON lines
                </button>
                <button
                  className="group flex w-full items-center gap-2 px-3 py-[5px] text-left text-[13px] text-slate-800 hover:bg-accent hover:text-white"
                  onClick={() => {
                    setExportOpen(false);
                    copyDataAs("tsv");
                  }}
                >
                  <Copy size={12} strokeWidth={1.7} className="text-slate-500 group-hover:text-white/75" /> Copy as TSV
                </button>
                <button
                  className="group flex w-full items-center gap-2 px-3 py-[5px] text-left text-[13px] text-slate-800 hover:bg-accent hover:text-white"
                  onClick={() => {
                    setExportOpen(false);
                    copyDataAs("sql");
                  }}
                >
                  <Copy size={12} strokeWidth={1.7} className="text-slate-500 group-hover:text-white/75" /> Copy as SQL INSERTs
                </button>
                <button
                  className="group flex w-full items-center gap-2 px-3 py-[5px] text-left text-[13px] text-slate-800 hover:bg-accent hover:text-white"
                  onClick={() => {
                    setExportOpen(false);
                    copyDataAs("md");
                  }}
                >
                  <Copy size={12} strokeWidth={1.7} className="text-slate-500 group-hover:text-white/75" /> Copy as Markdown table
                </button>
              </div>
            )}
          </div>
          {stripVisible && (
            <>
              <div className="mx-1.5 h-4 w-px bg-slate-200" />
              <button
                className={editBtnClass}
                disabled={!editable || applying}
                title="Add a new (unsaved) row at the bottom"
                onClick={addDraftRow}
              >
                <Plus size={13} strokeWidth={1.8} className="text-slate-500" />
                Add row
              </button>
              <button
                className={editIconClass}
                disabled={!editable || applying || !gridSel}
                title={
                  gridSel?.kind === "row" && deletedRows.has(gridSel.index)
                    ? "Unmark this row for deletion (⌫)"
                    : gridSel?.kind === "draft"
                      ? "Remove this new row (⌫)"
                      : "Mark the selected row for deletion (⌫)"
                }
                onClick={deleteSelected}
              >
                <Trash2 size={13} strokeWidth={1.7} />
              </button>
              <button
                className={applyBtnClass}
                disabled={!baseEditable || pendingCount === 0 || applying}
                title="Apply all pending changes"
                onClick={() => void applyEdits()}
              >
                {applying ? (
                  <RotateCw size={12} strokeWidth={2} className="animate-spin" />
                ) : (
                  <Check size={12} strokeWidth={2.2} />
                )}
                Apply{pendingCount > 0 ? ` (${pendingCount})` : ""}
              </button>
              <button
                className={editBtnClass}
                disabled={pendingCount === 0 || applying}
                title="Discard all local changes"
                onClick={discardEdits}
              >
                <Undo2 size={13} strokeWidth={1.7} className="text-slate-500" />
                Discard
              </button>
              {pendingCount > 0 && (
                <span className="ml-0.5 whitespace-nowrap text-[11.5px] text-slate-400">
                  {[
                    editCount > 0
                      ? `${editCount} edit${editCount === 1 ? "" : "s"}`
                      : null,
                    newCount > 0 ? `${newCount} new` : null,
                    deletedCount > 0 ? `${deletedCount} deleted` : null,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              )}
              {editHint && (
                <span className="ml-0.5 whitespace-nowrap text-[11.5px] text-slate-400">{editHint}</span>
              )}
            </>
          )}
          <div className="ml-1 flex h-7 shrink-0 items-center rounded-md bg-slate-900/[0.04] px-2 focus-within:ring-2 focus-within:ring-accent/40">
            <Filter size={12} strokeWidth={1.7} className="mr-1 text-slate-500" />
            <input
              className="w-44 border-none bg-transparent p-0 text-[12.5px] placeholder:text-slate-400 focus:outline-none"
              placeholder="Filter loaded rows…"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          </div>
          {findOpen ? (
            <div className="ml-1 flex h-7 shrink-0 items-center gap-0.5 rounded-md bg-slate-900/[0.04] px-1.5 focus-within:ring-2 focus-within:ring-accent/40">
              <input
                ref={findInputRef}
                className="w-36 border-none bg-transparent p-0 text-[12.5px] placeholder:text-slate-400 focus:outline-none"
                placeholder="Find in grid…"
                value={findQuery}
                onChange={(e) => setFindQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    stepFind(e.shiftKey ? -1 : 1);
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    closeFind();
                  }
                }}
              />
              <span className="px-0.5 text-[11px] text-slate-500 tabular-nums">
                {findTotal === 0 ? "0/0" : `${findActive}/${findTotal}`}
              </span>
              <button
                className="grid h-6 w-6 place-items-center rounded text-slate-500 hover:bg-slate-900/[0.06] hover:text-slate-700"
                title="Previous match (Shift+Enter)"
                onClick={() => stepFind(-1)}
              >
                <ChevronUp size={12} strokeWidth={1.7} />
              </button>
              <button
                className="grid h-6 w-6 place-items-center rounded text-slate-500 hover:bg-slate-900/[0.06] hover:text-slate-700"
                title="Next match (Enter)"
                onClick={() => stepFind(1)}
              >
                <ChevronDown size={12} strokeWidth={1.7} />
              </button>
              <button
                className="grid h-6 w-6 place-items-center rounded text-slate-500 hover:bg-slate-900/[0.06] hover:text-slate-700"
                title="Close find (Esc)"
                onClick={closeFind}
              >
                <X size={12} strokeWidth={1.7} />
              </button>
            </div>
          ) : (
            <button
              className="ml-1 grid h-7 w-7 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700"
              title="Find in grid (Ctrl+F)"
              onClick={openFind}
            >
              <Search size={13} strokeWidth={1.7} />
            </button>
          )}
        </div>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          {rangeCells !== null && (
            <span className="text-[12px] font-medium whitespace-nowrap text-accent">
              {rangeCells.toLocaleString()} selected
              {rangeStats && rangeStats.numeric > 0 && (
                <span className="font-normal text-slate-500">
                  {" · "}
                  {rangeStats.numeric.toLocaleString()} numeric · Σ{" "}
                  {formatAgg(rangeStats.sum)} · avg {formatAgg(rangeStats.avg)}
                </span>
              )}
            </span>
          )}
          <div className="flex shrink-0 items-center rounded-md bg-slate-900/[0.04] p-0.5">
            <button
              className={`grid h-6 w-7 place-items-center rounded-[5px] transition-colors ${
                viewMode === "grid"
                  ? "bg-white text-slate-800 shadow-sm"
                  : "text-slate-400 hover:text-slate-600"
              }`}
              title="Grid view"
              onClick={() => setViewMode("grid")}
            >
              <Grid3x3 size={13} strokeWidth={1.7} />
            </button>
            <button
              className={`grid h-6 w-7 place-items-center rounded-[5px] transition-colors ${
                viewMode === "form"
                  ? "bg-white text-slate-800 shadow-sm"
                  : "text-slate-400 hover:text-slate-600"
              }`}
              title="Form view (one record at a time)"
              onClick={() => setViewMode("form")}
            >
              <List size={13} strokeWidth={1.7} />
            </button>
          </div>
          {stripVisible && (
            <span className="whitespace-nowrap text-[11.5px] text-slate-500">
              {pendingCount > 0
                ? "⌘/Ctrl+Enter apply · ⌫ delete row"
                : editable
                  ? "Double-click to edit · ⌫ delete row · ⌥-double-click view"
                  : ""}
            </span>
          )}
          <span className="whitespace-nowrap text-[12px] text-slate-600">
            {matchIndices
              ? `${matchIndices.length.toLocaleString()} / ${(result?.rows.length ?? 0).toLocaleString()} rows`
              : `${(result?.rows.length ?? 0).toLocaleString()} rows`}
          </span>
        </div>
      </div>

      {/* apply errors (collapsible; buffers are kept so nothing is lost) */}
      {applyErrors && applyErrors.length > 0 && (
        <div className="shrink-0 border-b border-rose-200 bg-rose-50 px-3 py-1.5 text-[12px] text-rose-700">
          <div className="flex items-start gap-2">
            <AlertTriangle size={13} strokeWidth={1.8} className="mt-0.5 shrink-0" />
            <div className="min-w-0 flex-1">
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium">
                  Apply failed — {applyErrors.length} error
                  {applyErrors.length === 1 ? "" : "s"}; changes were kept.
                </span>
                <span className="flex shrink-0 items-center gap-2">
                  {applyErrors.length > 1 && (
                    <button
                      className="text-rose-600 hover:underline"
                      onClick={() => setErrorsOpen((v) => !v)}
                    >
                      {errorsOpen ? "Hide all" : "Show all"}
                    </button>
                  )}
                  <button
                    className="text-rose-500 hover:text-rose-700"
                    title="Dismiss"
                    onClick={() => setApplyErrors(null)}
                  >
                    ✕
                  </button>
                </span>
              </div>
              <div className={errorsOpen ? "" : "truncate"}>{applyErrors[0]}</div>
              {errorsOpen &&
                applyErrors.slice(1).map((e, i) => (
                  <div key={i} className="mt-0.5 font-code whitespace-pre-wrap">
                    {e}
                  </div>
                ))}
            </div>
          </div>
        </div>
      )}

      {/* content */}
      {subTab === "result" && (
        <>
          {!result ? (
            <div className="grid flex-1 place-items-center bg-surface text-[13px] text-slate-500">
              Run the query to see results
            </div>
          ) : result.error ? (
            <ErrorPanel
              error={result.error}
              sql={result.sql}
              onShowMessages={() => setSubTab("messages")}
            />
          ) : result.status === "done" && result.rows.length === 0 ? (
            <div className="grid flex-1 place-items-center bg-surface">
              <div className="flex flex-col items-center gap-1.5 text-center">
                <CheckCircle2 size={22} strokeWidth={1.6} className="text-emerald-500" />
                <div className="text-[13.5px] font-medium text-slate-700">
                  Success — no rows returned
                </div>
                <div className="text-[12px] text-slate-400">
                  {result.elapsedMs !== null ? `${result.elapsedMs} ms` : ""}
                  {result.stats && result.stats.rows_affected > 0
                    ? ` · ${result.stats.rows_affected.toLocaleString()} row${result.stats.rows_affected === 1 ? "" : "s"} affected`
                    : ""}
                </div>
              </div>
            </div>
          ) : viewMode === "form" ? (
            <RecordView
              columns={result.columns}
              rows={result.rows}
              rowIndex={safeFormRow}
              onRowChange={setFormRow}
              onViewCell={(r, c) => setViewer({ row: r, col: c, displayRow: r })}
            />
          ) : (
            <ResultGrid
              result={result}
              matchIndices={matchIndices}
              onViewCell={(r, c, displayRow) =>
                setViewer({ row: r, col: c, displayRow })
              }
              onApi={(api) => (gridApiRef.current = api)}
              onRangeChange={handleRangeChange}
              findQuery={findOpen ? deferredFind : ""}
              findCommand={findCmd}
              onFindMatches={handleFindMatches}
              onRequestFind={openFind}
              editable={editable}
              pkCols={pkCols}
              edits={edits}
              deletedRows={deletedRows}
              drafts={drafts}
              onEditCell={handleEditCell}
              onToggleDeleteRow={toggleDeleteRow}
              onEditDraft={handleEditDraft}
              onRemoveDraft={removeDraft}
              onSelectionChange={handleGridSelection}
              onApplyShortcut={requestApply}
            />
          )}
        </>
      )}

      {subTab === "messages" && (
        <MessagesView messages={tab.messages} onClear={() => undefined} />
      )}

      {subTab === "status" && (
        <StatusView tab={tab} result={result} />
      )}

      {viewer && result && (
        <CellViewer
          column={result.columns[viewer.col]}
          value={result.rows[viewer.row]?.[viewer.col] ?? { t: "null" }}
          rowNumber={(viewer.displayRow ?? viewer.row) + 1}
          onClose={() => setViewer(null)}
        />
      )}
    </div>
  );
}

/** Compact number for selection aggregates (up to 2 decimals). */
function formatAgg(n: number): string {
  return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

/** Tooltip for a result sub-tab: connection + time + SQL (DBeaver-style). */
function resultTitle(
  sessionName: string | undefined,
  sql: string,
  startedAt: number,
  elapsedMs: number | null,
): string {
  const first = sql.split("\n").find((l) => l.trim().length > 0) ?? sql;
  const short = first.length > 160 ? `${first.slice(0, 160)}…` : first;
  const when = new Date(startedAt).toLocaleString();
  const took = elapsedMs !== null ? ` · ${elapsedMs} ms` : "";
  return `${sessionName ?? "query"} · ${when}${took}\n${short}`;
}

function ExportIcon({ format }: { format: ExportFormat }) {
  switch (format) {
    case "csv":
    case "xlsx":
      return <FileSpreadsheet size={12} strokeWidth={1.7} className="text-slate-500 group-hover:text-white/75" />;
    case "tsv":
      return <FileText size={12} strokeWidth={1.7} className="text-slate-500 group-hover:text-white/75" />;
    case "json":
    case "json_lines":
      return <FileJson size={12} strokeWidth={1.7} className="text-slate-500 group-hover:text-white/75" />;
    case "sql":
      return <Database size={12} strokeWidth={1.7} className="text-slate-500 group-hover:text-white/75" />;
  }
}

/**
 * Character offset of a query error inside its SQL text: native `position`
 * when the engine reports one (Postgres), otherwise the start of the
 * `... at line N` line MySQL reports (e.g. error 1064). Null when unknown.
 */
function errorOffset(sql: string, error: ApiError): number | null {
  if (typeof error.position === "number" && Number.isFinite(error.position)) {
    return Math.max(0, Math.min(error.position, sql.length));
  }
  const m = / at line (\d+)/i.exec(error.message);
  if (!m) return null;
  const lineNo = Math.max(1, parseInt(m[1], 10));
  const lines = sql.split("\n");
  let off = 0;
  for (let i = 0; i < Math.min(lineNo - 1, lines.length); i++) {
    off += (lines[i]?.length ?? 0) + 1;
  }
  return Math.min(off, sql.length);
}

function ErrorPanel({
  error,
  sql,
  onShowMessages,
}: {
  error: ApiError;
  sql: string;
  onShowMessages: () => void;
}) {
  const offset = errorOffset(sql, error);
  return (
    <div className="flex-1 overflow-auto bg-rose-50 p-4">
      <div className="flex items-start gap-2 rounded-md border border-rose-200 bg-white p-3">
        <AlertTriangle size={16} strokeWidth={1.7} className="mt-0.5 shrink-0 text-rose-500" />
        <div className="min-w-0">
          <div className="font-log text-rose-600">{error.code}</div>
          <div className="mt-0.5 text-[13px] font-medium text-slate-900">
            {error.message}
          </div>
          {error.detail && (
            <div className="mt-1 font-log whitespace-pre-wrap text-slate-600">
              {error.detail}
            </div>
          )}
          {error.hint && (
            <div className="mt-1 text-[13px] text-slate-500">
              Hint: {error.hint}
            </div>
          )}
          {error.db_code && (
            <div className="mt-1 font-log text-slate-400">
              code: {error.db_code}
            </div>
          )}
          <div className="mt-2 flex items-center gap-3">
            {error.code === "syntax" && (
              <button
                className="inline-flex items-center gap-1 text-[13px] font-medium text-accent hover:underline"
                onClick={() =>
                  window.dispatchEvent(new CustomEvent("mieosql:fix-sql"))
                }
              >
                <Wrench size={12} strokeWidth={1.8} />
                Try to fix the syntax
              </button>
            )}
            {offset !== null && (
              <button
                className="inline-flex items-center gap-1 text-[13px] font-medium text-accent hover:underline"
                onClick={() =>
                  window.dispatchEvent(
                    new CustomEvent("mieosql:goto-offset", { detail: offset }),
                  )
                }
              >
                <Crosshair size={12} strokeWidth={1.8} />
                Go to error
              </button>
            )}
            <button
              className="text-[13px] text-accent hover:underline"
              onClick={onShowMessages}
            >
              Open Messages tab
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function MessagesView({
  messages,
}: {
  messages: LogMessage[];
  onClear: () => void;
}) {
  return (
    <div className="custom-scrollbar font-log flex-1 overflow-auto bg-surface p-3">
      {messages.length === 0 && (
        <div className="text-slate-600">Nothing here yet.</div>
      )}
      {messages.map((m) => (
        <div
          key={m.id}
          className={`flex items-start gap-2 py-0.5 ${
            m.level === "error"
              ? "text-rose-600"
              : m.level === "warn"
                ? "text-amber-600"
                : "text-slate-700"
          }`}
        >
          <span className="w-[52px] shrink-0 text-[11px] text-slate-500">
            {new Date(m.at).toLocaleTimeString()}
          </span>
          <span className="whitespace-pre-wrap">
            {m.level === "error" ? "✗ " : m.level === "warn" ? "! " : "› "}
            {m.text}
          </span>
        </div>
      ))}
    </div>
  );
}

function StatusView({ tab, result }: { tab: Tab; result?: ResultSet }) {
  const session = useStore((s) =>
    s.sessions.find((x) => x.id === tab.sessionId),
  );
  const rows: [string, string][] = [
    ["Query id", result?.queryId ?? "—"],
    ["Status", result?.status ?? "—"],
    ["Rows", (result?.rows.length ?? 0).toLocaleString()],
    ["Rows affected", String(result?.stats?.rows_affected ?? 0)],
    [
      "Last insert id",
      result?.stats?.last_insert_id !== undefined
        ? String(result.stats.last_insert_id)
        : "—",
    ],
    ["Elapsed", result?.elapsedMs !== null && result?.elapsedMs !== undefined ? `${result.elapsedMs} ms` : "—"],
    ["Truncated by limit", result?.truncated ? "yes" : "no"],
    ["Session", session ? `${session.name} (${session.engine})` : "—"],
    ["Database", session?.database ?? "—"],
    ["In transaction", session?.in_transaction ? "yes" : "no"],
  ];
  return (
    <div className="custom-scrollbar flex-1 overflow-auto bg-surface p-4">
      <div className="grid grid-cols-[180px_1fr] gap-x-4 gap-y-1.5 text-[12.5px]">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <div className="text-slate-500">{k}</div>
            <div className="font-code text-slate-800">{v}</div>
          </div>
        ))}
      </div>
    </div>
  );
}
