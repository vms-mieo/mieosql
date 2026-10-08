import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, ArrowUp, Pin } from "lucide-react";

import type { ColumnMeta, Scope, Value } from "../lib/bindings";
import {
  compareValues,
  copyText,
  formatCell,
  kindBadge,
  plainText,
  toCsv,
  toJsonLines,
} from "../lib/format";

import type { ResultSet } from "../lib/store";
import { useStore } from "../lib/store";

// ---------------------------------------------------------------------------
// Inline grid editing
//
// `GridEditRequest` mirrors the `apply_grid_edits` IPC contract. The types are
// declared locally (rather than imported from the generated bindings) so this
// file stays decoupled from the backend generator; the shapes are structurally
// equal. Committed editor text is sent as `{ t: "text" }`: the backend binds it
// as text and the database casts it to the column type.
// ---------------------------------------------------------------------------

export type GridCell = { column: string; value: Value };
export type RowChanges = { keys: GridCell[]; changes: GridCell[] };
export type GridEditRequest = {
  session_id: string;
  scope: Scope;
  table: string;
  inserts: GridCell[][];
  updates: RowChanges[];
  deletes: GridCell[][];
};
export type EditApplyResult = {
  inserted: number;
  updated: number;
  deleted: number;
  failed: number;
  errors: string[];
  elapsed_ms: number;
};

/** Pending edits per existing row: original result row index -> col index -> value. */
export type RowEditMap = Map<number, Map<number, Value>>;
/** A draft ("Add row") row: col index -> value; untouched columns are skipped on insert. */
export type DraftRow = Map<number, Value>;
/** Selection reported to the panel, in original result coordinates. */
export type GridSelection =
  | { kind: "row"; index: number }
  | { kind: "draft"; index: number };

export type GridApi = {
  scrollToTop: () => void;
  scrollToBottom: () => void;
  scrollBy: (delta: number) => void;
  /** Current rectangular range as a column subset + row values (display order). */
  getRangeData?: () => RangeCopyData | null;
  clearRange?: () => void;
};
export type CellRef = { row: number; col: number };

const ROW_H = 30;
const GUTTER_W = 48;
const MIN_COL_W = 64;
const MAX_COL_W = 720;
const OVERSCAN = 8;

const NULL_VALUE = { t: "null" } as Value;
/** Rectangular cell range in DISPLAY coordinates (after sort/filter). */
export type GridRange = {
  fromRow: number;
  fromCol: number;
  toRow: number;
  toCol: number;
};

/** Clipboard formats offered by the "Copy as …" menu. */
export type CopyFormat = "csv" | "tsv" | "json" | "sql" | "md";

/** Column subset + row values backing one Copy-as operation. */
export type RangeCopyData = { columns: ColumnMeta[]; rows: Value[][] };

/** A find-navigation request from the panel: step `dir`, deduped by `n`. */
export type FindCommand = { dir: 1 | -1; n: number };

// ---------------------------------------------------------------------------
// Copy-as builders.
//
// CSV and JSON-lines reuse `toCsv` / `toJsonLines` from `lib/format`; the
// TSV / Markdown / SQL-INSERT serializers below compose the same `plainText`
// primitive so NULLs and nested values behave identically everywhere.
// ---------------------------------------------------------------------------

function quoteIdent(name: string): string {
  return `"${name.replace(/"/g, '""')}"`;
}

function base64ToHex(b64: string): string | null {
  try {
    const bin = atob(b64);
    let hex = "";
    for (let i = 0; i < bin.length; i++) {
      hex += bin.charCodeAt(i).toString(16).padStart(2, "0");
    }
    return hex;
  } catch {
    return null;
  }
}

/** A `Value` rendered as a SQL literal for INSERT statements. */
function sqlLiteral(value: Value): string {
  switch (value.t) {
    case "null":
      return "NULL";
    case "bool":
      return value.v ? "TRUE" : "FALSE";
    case "int":
    case "uint":
    case "decimal":
      return value.v;
    case "float":
      return Number.isFinite(value.v) ? String(value.v) : "NULL";
    case "text":
    case "date":
    case "time":
    case "datetime":
    case "uuid":
      return `'${value.v.replace(/'/g, "''")}'`;
    case "bytes": {
      const hex = base64ToHex(value.v);
      return hex !== null ? `X'${hex}'` : "NULL";
    }
    case "json":
    case "array": {
      const raw =
        value.t === "json"
          ? (JSON.stringify(value.v) ?? "null")
          : `[${value.v.map(plainText).join(", ")}]`;
      return `'${raw.replace(/'/g, "''")}'`;
    }
    default:
      return "NULL";
  }
}

/** TSV with raw tabs/newlines (spreadsheet-paste friendly, like DBeaver). */
function toTsvText(columns: ColumnMeta[], rows: Value[][]): string {
  const header = columns.map((c) => c.name).join("\t");
  const body = rows.map((row) => row.map((v) => plainText(v)).join("\t")).join("\n");
  return `${header}\n${body}`;
}

function escapeMdCell(s: string): string {
  return s.replace(/\|/g, "\\|").replace(/\r\n|\n|\r/g, "<br/>");
}

function toMarkdownTable(columns: ColumnMeta[], rows: Value[][]): string {
  const header = `| ${columns.map((c) => escapeMdCell(c.name)).join(" | ")} |`;
  const sep = `| ${columns.map(() => "---").join(" | ")} |`;
  const body = rows.map(
    (row) => `| ${row.map((v) => escapeMdCell(plainText(v))).join(" | ")} |`,
  );
  return [header, sep, ...body].join("\n");
}

function toSqlInserts(
  tableName: string,
  columns: ColumnMeta[],
  rows: Value[][],
): string {
  const table = quoteIdent(tableName);
  const cols = columns.map((c) => quoteIdent(c.name)).join(", ");
  return rows
    .map(
      (row) =>
        `INSERT INTO ${table} (${cols}) VALUES (${row.map(sqlLiteral).join(", ")});`,
    )
    .join("\n");
}

/** Format one Copy-as payload; `tableName` is only used for SQL INSERTs. */
export function buildCopyText(
  kind: CopyFormat,
  tableName: string,
  columns: ColumnMeta[],
  rows: Value[][],
): string {
  switch (kind) {
    case "csv":
      return toCsv(columns, rows);
    case "tsv":
      return toTsvText(columns, rows);
    case "json":
      return toJsonLines(columns, rows);
    case "sql":
      return toSqlInserts(tableName, columns, rows);
    case "md":
      return toMarkdownTable(columns, rows);
  }
}

function clampW(w: number): number {
  return Math.max(MIN_COL_W, Math.min(MAX_COL_W, Math.round(w)));
}

function defaultWidth(col: ColumnMeta): number {
  const nameW = col.name.length * 7.6 + 66;
  switch (col.kind) {
    case "int":
    case "uint":
    case "float":
    case "decimal":
      return clampW(Math.max(96, nameW));
    case "bool":
    case "bit":
      return clampW(Math.max(80, nameW));
    case "date":
    case "time":
      return clampW(Math.max(110, nameW));
    case "datetime":
    case "timestamptz":
      return clampW(Math.max(178, nameW));
    case "json":
    case "bytes":
    case "geometry":
      return clampW(Math.max(160, nameW));
    default:
      return clampW(Math.max(130, nameW));
  }
}

function cellClass(tone: ReturnType<typeof formatCell>["tone"]): string {
  switch (tone) {
    case "null":
      return "italic text-slate-400";
    case "number":
      return "font-code text-slate-800";
    case "temporal":
      return "font-code text-slate-500";
    case "json":
      return "font-code text-slate-700";
    case "binary":
      return "font-code text-slate-600";
    case "bool":
      return "font-code text-slate-800";
    case "unsupported":
      return "italic text-amber-600";
    default:
      return "text-slate-800";
  }
}

/** Muted tone for the type badge in column headers. */
function badgeTone(kind: ColumnMeta["kind"]): string {
  switch (kind) {
    case "int":
    case "uint":
    case "float":
    case "decimal":
      return "text-slate-400";
    case "date":
    case "time":
    case "datetime":
    case "timestamptz":
      return "text-slate-400";
    case "json":
    case "uuid":
      return "text-slate-400";
    case "bytes":
    case "geometry":
      return "text-slate-400";
    case "bool":
      return "text-slate-400";
    default:
      return "text-slate-400";
  }
}

/** State of the inline cell editor (at most one open at a time). */
type EditingState = {
  /** Display row: virtual list index, or `total + draftIndex` for draft rows. */
  row: number;
  col: number;
  draft: boolean;
  draftIndex: number; // index into `drafts` when draft, -1 otherwise
  origRow: number; // original result row index when not draft, -1 otherwise
  value: string; // raw editor text
};

type Props = {
  result: ResultSet;
  matchIndices: number[] | null;
  /** `rowIndex` is the ORIGINAL row index (before sorting/filtering). */
  onViewCell: (rowIndex: number, colIndex: number, displayRow: number) => void;
  onApi?: (api: GridApi) => void;
  // ---- inline editing (all optional; the grid stays read-only when disabled)
  /** Whether double-click inline editing is enabled right now. */
  editable?: boolean;
  /** Primary-key column indices — immutable: double-click opens the viewer. */
  pkCols?: number[];
  edits?: RowEditMap;
  deletedRows?: Set<number>;
  drafts?: DraftRow[];
  onEditCell?: (rowIndex: number, colIndex: number, value: Value) => void;
  onToggleDeleteRow?: (rowIndex: number) => void;
  onEditDraft?: (draftIndex: number, colIndex: number, value: Value) => void;
  onRemoveDraft?: (draftIndex: number) => void;
  onSelectionChange?: (sel: GridSelection | null) => void;
  /** Fired when ⌘/Ctrl+Enter was pressed inside the inline editor. */
  onApplyShortcut?: () => void;
  /** Fired when the rectangular range changes (null = no range). */
  onRangeChange?: (range: GridRange | null) => void;
  /** Lowercased find query from the panel find bar; "" disables find. */
  findQuery?: string;
  /** Incrementing navigation request (Enter/Shift+Enter in the find bar). */
  findCommand?: FindCommand | null;
  /** Reports (matchCount, 1-based active index, 0 when no matches). */
  onFindMatches?: (total: number, active: number) => void;
  /** Ctrl+F pressed while the grid has focus: the panel opens its find bar. */
  onRequestFind?: () => void;

};

export function ResultGrid({
  result,
  matchIndices,
  onViewCell,
  onApi,
  editable = false,
  pkCols = [],
  edits,
  deletedRows,
  drafts = [],
  onEditCell,
  onToggleDeleteRow,
  onEditDraft,
  onRemoveDraft,
  onSelectionChange,
  onApplyShortcut,
  onRangeChange,
  findQuery = "",
  findCommand = null,
  onFindMatches,
  onRequestFind,

}: Props) {
  const { columns, rows } = result;
  const toast = useStore((s) => s.toast);

  const [widths, setWidths] = useState<number[]>([]);
  const [sort, setSort] = useState<{ col: number; dir: "asc" | "desc" } | null>(
    null,
  );
  const [sel, setSel] = useState<CellRef | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(320);
  const [editing, setEditing] = useState<EditingState | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLInputElement>(null);
  // Set while an editor is being closed without committing (Escape) so the
  // input's blur event does not sneak the abandoned value into the buffer.
  const closingRef = useRef(false);
  const prevDraftCount = useRef(drafts.length);
  // Rectangular range selection (drag / shift-click / shift-arrows), in
  // DISPLAY coordinates. Coexists with the single-cell `sel`, which tracks
  // the anchor end of the range.
  const [range, setRange] = useState<GridRange | null>(null);
  const [dragging, setDragging] = useState(false);
  // Leading-column freeze count (0 = off). Kept in grid state only, so it is
  // lost on re-run — persisting per tab (e.g. localStorage) is a TODO.
  const [frozen, setFrozen] = useState(0);
  // 0-based index into `findHits`.
  const [findActive, setFindActive] = useState(0);
  // Mutable drag/anchor state lives in refs so the window mouseup listener
  // (registered once) always sees the latest values.
  const draggingRef = useRef(false);
  const anchorRef = useRef<CellRef | null>(null);
  const rangeRef = useRef<GridRange | null>(null);
  rangeRef.current = range;
  const activeRef = useRef(0);
  const lastFindN = useRef(0);
  const prevFindQ = useRef("");


  // Reset view state for a new query. All edit buffers live in the panel and
  // are discarded there when `queryId` changes.
  useEffect(() => {
    setSort(null);
    setSel(null);
    setScrollTop(0);
    setEditing(null);
    setRange(null);
    anchorRef.current = null;
    setFrozen(0);
    setFindActive(0);
    activeRef.current = 0;

  }, [result.queryId]);

  // Column widths initialize (and re-init) when the column set changes.
  useEffect(() => {
    if (columns.length > 0 && widths.length !== columns.length) {
      setWidths(columns.map(defaultWidth));
    }
  }, [columns, widths.length]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () => setViewportH(el.clientHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Latest range data for the panel's "Copy as …" menu (registered on the
  // api below). Assigned during render so the api closure never goes stale.
  const rangeDataProvider = useRef<() => RangeCopyData | null>(() => null);

  useEffect(() => {
    onApi?.({
      scrollToTop: () => scrollRef.current?.scrollTo({ top: 0 }),
      scrollToBottom: () =>
        scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight }),
      scrollBy: (delta) => scrollRef.current?.scrollBy({ top: delta }),
      getRangeData: () => rangeDataProvider.current(),
      clearRange: () => {
        draggingRef.current = false;
        anchorRef.current = null;
        setRange(null);
        setDragging(false);
      },
    });
  }, [onApi]);

  // Report the rectangular range to the panel (drives "Copy as …" scope).
  useEffect(() => {
    onRangeChange?.(range);
  }, [range, onRangeChange]);

  // The filter remaps display rows, so a stale range would point at the wrong
  // rows — drop it (single-cell `sel` is kept, as before).
  useEffect(() => {
    setRange(null);
    anchorRef.current = null;
  }, [matchIndices]);

  // End a drag selection anywhere (including outside the grid). A click
  // without drag collapses back to single-cell selection, not a 1-cell range,
  // so "Copy as …" still covers the whole result after a plain click.
  useEffect(() => {
    const onUp = () => {
      if (!draggingRef.current) return;
      draggingRef.current = false;
      setDragging(false);
      const r = rangeRef.current;
      if (r && r.fromRow === r.toRow && r.fromCol === r.toCol) setRange(null);
    };
    window.addEventListener("mouseup", onUp);
    return () => window.removeEventListener("mouseup", onUp);
  }, []);

  const indices = useMemo(() => {
    if (!sort) return matchIndices;
    const arr = matchIndices
      ? matchIndices.slice()
      : Array.from({ length: rows.length }, (_, i) => i);
    const kind = columns[sort.col]?.kind ?? "unknown";
    const dir = sort.dir === "asc" ? 1 : -1;
    arr.sort(
      (a, b) =>
        dir *
        compareValues(
          rows[a]?.[sort.col] ?? NULL_VALUE,
          rows[b]?.[sort.col] ?? NULL_VALUE,
          kind,
        ),
    );
    return arr;
  }, [matchIndices, sort, rows, columns]);

  const total = indices ? indices.length : rows.length;
  const draftCount = drafts.length;
  const totalRows = total + draftCount;

  const rowAt = useCallback(
    (i: number): Value[] => {
      const row = indices ? rows[indices[i]!] : rows[i];
      return row ?? [];
    },
    [indices, rows],
  );

  /** Original result row index behind a display row (identity for drafts). */
  const origOf = useCallback(
    (displayRow: number): number =>
      indices ? (indices[displayRow] ?? displayRow) : displayRow,
    [indices],
  );

  /** Effective cell values of a display row, including pending edits/drafts. */
  const displayRowValues = useCallback(
    (displayRow: number): Array<Value | undefined> => {
      if (displayRow >= total) {
        const draft = drafts[displayRow - total];
        return draft ? columns.map((_, ci) => draft.get(ci)) : [];
      }
      const orig = indices ? (indices[displayRow] ?? displayRow) : displayRow;
      const base = rows[orig] ?? [];
      const pending = edits?.get(orig);
      if (!pending) return base;
      return columns.map((_, ci) => pending.get(ci) ?? base[ci]);
    },
    [total, drafts, columns, indices, rows, edits],
  );
  // ---- frozen columns: sticky left offsets (gutter + preceding widths)
  const frozenCols = Math.max(0, Math.min(frozen, columns.length));
  const frozenLeft = useMemo(() => {
    const offs: number[] = [];
    let acc = GUTTER_W;
    for (let i = 0; i < columns.length; i++) {
      offs.push(acc);
      acc += widths[i] ?? 120;
    }
    return offs;
  }, [columns.length, widths]);

  // ---- rectangular range helpers
  const normRange = (r: GridRange) => ({
    r0: Math.max(0, Math.min(r.fromRow, r.toRow)),
    r1: Math.min(total - 1, Math.max(r.fromRow, r.toRow)),
    c0: Math.max(0, Math.min(r.fromCol, r.toCol)),
    c1: Math.min(columns.length - 1, Math.max(r.fromCol, r.toCol)),
  });

  const inRange = (displayRow: number, ci: number): boolean => {
    if (!range) return false;
    const { r0, r1, c0, c1 } = normRange(range);
    return displayRow >= r0 && displayRow <= r1 && ci >= c0 && ci <= c1;
  };

  // Backing data for "Copy as …": column subset + row values in display
  // order (sort/filter apply, like the visible grid).
  rangeDataProvider.current = () => {
    if (!range || columns.length === 0) return null;
    const { r0, r1, c0, c1 } = normRange(range);
    if (r1 < r0 || c1 < c0) return null;
    const cols = columns.slice(c0, c1 + 1);
    const out: Value[][] = [];
    for (let r = r0; r <= r1; r++) {
      const row = rowAt(r);
      const cells: Value[] = [];
      for (let c = c0; c <= c1; c++) cells.push(row[c] ?? NULL_VALUE);
      out.push(cells);
    }
    return { columns: cols, rows: out };
  };

  const copyRange = () => {
    const data = rangeDataProvider.current();
    if (!data) return;
    const cells = data.rows.length * data.columns.length;
    const text = buildCopyText("tsv", "", data.columns, data.rows);
    void copyText(text).then((ok) =>
      toast(
        ok ? "success" : "error",
        ok ? `Copied ${cells.toLocaleString()} cell${cells === 1 ? "" : "s"} (TSV)` : "Copy failed",
      ),
    );
  };

  // ---- find in grid (jump + highlight; never filters rows, unlike the
  // panel filter behind `matchIndices`)
  const findQ = findQuery.toLowerCase();
  const findHits = useMemo(() => {
    if (!findQ || columns.length === 0) return [] as CellRef[];
    const hits: CellRef[] = [];
    const cap = 2000; // bound highlight work on huge result sets
    outer: for (let r = 0; r < total; r++) {
      const row = rowAt(r);
      for (let c = 0; c < columns.length; c++) {
        const v = row[c];
        if (!v) continue;
        if (plainText(v).toLowerCase().includes(findQ)) {
          hits.push({ row: r, col: c });
          if (hits.length >= cap) break outer;
        }
      }
    }
    return hits;
  }, [findQ, columns.length, rowAt, total]);

  const findSet = useMemo(() => {
    if (findHits.length === 0) return null;
    const set = new Set<string>();
    for (const h of findHits) set.add(`${h.row}:${h.col}`);
    return set;
  }, [findHits]);

  const safeFindActive =
    findHits.length === 0 ? 0 : Math.min(findActive, findHits.length - 1);

  /** Move selection to a find hit and scroll it into view (no focus steal). */
  const jumpToHit = (hit: CellRef) => {
    setSel({ row: hit.row, col: hit.col });
    ensureVisible(hit.row);
    // `ensureVisible` only scrolls vertically; the horizontal pass runs after
    // the virtual list re-renders the target row.
    requestAnimationFrame(() => {
      scrollRef.current
        ?.querySelector(`[data-cell="${hit.row}:${hit.col}"]`)
        ?.scrollIntoView({ block: "nearest", inline: "nearest" });
    });
  };

  // New query text: restart from the first match and reveal it.
  useEffect(() => {
    if (prevFindQ.current === findQ) return;
    prevFindQ.current = findQ;
    activeRef.current = 0;
    setFindActive(0);
    if (findQ && findHits.length > 0) jumpToHit(findHits[0]!);
    // `jumpToHit`/`ensureVisible` are render-scoped helpers; only the query
    // and the resulting hits matter here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [findQ, findHits]);

  // Enter / Shift+Enter in the find bar steps through the matches.
  useEffect(() => {
    if (!findCommand || findCommand.n === 0 || findHits.length === 0) return;
    if (findCommand.n === lastFindN.current) return;
    lastFindN.current = findCommand.n;
    const next =
      (activeRef.current + findCommand.dir + findHits.length) % findHits.length;
    activeRef.current = next;
    setFindActive(next);
    jumpToHit(findHits[next]!);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [findCommand, findHits]);

  useEffect(() => {
    onFindMatches?.(findHits.length, findHits.length === 0 ? 0 : safeFindActive + 1);
  }, [findHits.length, safeFindActive, onFindMatches]);


  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const visibleCount = Math.ceil(viewportH / ROW_H) + OVERSCAN * 2;
  const end = Math.min(total, start + visibleCount);

  const totalWidth = GUTTER_W + widths.reduce((a, b) => a + b, 0);

  // ---------------------------------------------------------------- editing

  const beginEdit = useCallback(
    (displayRow: number, ci: number) => {
      if (!editable) return;
      if (displayRow >= total) {
        const di = displayRow - total;
        const current = drafts[di]?.get(ci) ?? NULL_VALUE;
        closingRef.current = false;
        setEditing({
          row: displayRow,
          col: ci,
          draft: true,
          draftIndex: di,
          origRow: -1,
          value: plainText(current),
        });
      } else {
        const orig = origOf(displayRow);
        // Deleted rows and primary keys are read-only (use ⌥-double-click to
        // open the value viewer instead).
        if (deletedRows?.has(orig)) return;
        if (pkCols.includes(ci)) return;
        const current = edits?.get(orig)?.get(ci) ?? rows[orig]?.[ci] ?? NULL_VALUE;
        closingRef.current = false;
        setEditing({
          row: displayRow,
          col: ci,
          draft: false,
          draftIndex: -1,
          origRow: orig,
          value: plainText(current),
        });
      }
      setSel({ row: displayRow, col: ci });
    },
    [editable, total, drafts, origOf, deletedRows, pkCols, edits, rows],
  );

  const commitEdit = (refocus = false) => {
    const cur = editing;
    if (!cur) return;
    setEditing(null);
    // Empty input => SQL NULL; non-empty => text. The backend binds text and
    // the database casts it (documented `apply_grid_edits` behavior).
    const value: Value = cur.value === "" ? NULL_VALUE : { t: "text", v: cur.value };
    if (cur.draft) onEditDraft?.(cur.draftIndex, cur.col, value);
    else onEditCell?.(cur.origRow, cur.col, value);
    if (refocus) requestAnimationFrame(() => scrollRef.current?.focus());
  };

  const cancelEdit = () => {
    closingRef.current = true;
    setEditing(null);
    requestAnimationFrame(() => scrollRef.current?.focus());
  };

  /** Commit and move the editor left/right (Tab / Shift+Tab), skipping PK cols. */
  const moveEdit = (dir: 1 | -1) => {
    const cur = editing;
    if (!cur) return;
    let ci = cur.col + dir;
    while (ci >= 0 && ci < columns.length && !cur.draft && pkCols.includes(ci)) {
      ci += dir;
    }
    commitEdit();
    if (ci >= 0 && ci < columns.length) beginEdit(cur.row, ci);
    else requestAnimationFrame(() => scrollRef.current?.focus());
  };

  const onEditorKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      const apply = e.metaKey || e.ctrlKey;
      commitEdit(!apply);
      if (apply) window.setTimeout(() => onApplyShortcut?.(), 0);
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelEdit();
    } else if (e.key === "Tab") {
      e.preventDefault();
      moveEdit(e.shiftKey ? -1 : 1);
    }
  };

  const onEditorBlur = () => {
    if (closingRef.current) {
      closingRef.current = false;
      return;
    }
    commitEdit();
  };

  const editorKey = editing ? `${editing.row}:${editing.col}` : null;
  useEffect(() => {
    if (!editorKey) return;
    const el = editorRef.current;
    if (el) {
      el.focus();
      el.select(); // select-all, Excel-like
    }
  }, [editorKey]);

  // Editing can be turned off under the user's feet (filter typed, PK list
  // re-fetched, …): drop any open editor.
  useEffect(() => {
    if (!editable && editing) setEditing(null);
  }, [editable, editing]);

  // Report the selection to the panel in ORIGINAL row coordinates.
  useEffect(() => {
    if (!onSelectionChange) return;
    if (!sel) {
      onSelectionChange(null);
      return;
    }
    if (sel.row >= total) {
      const di = sel.row - total;
      onSelectionChange(di < draftCount ? { kind: "draft", index: di } : null);
    } else {
      onSelectionChange({
        kind: "row",
        index: indices ? (indices[sel.row] ?? sel.row) : sel.row,
      });
    }
  }, [sel, total, draftCount, indices, onSelectionChange]);

  // A draft row was just added: select it and open the first cell for typing.
  useEffect(() => {
    const grew = drafts.length > prevDraftCount.current;
    prevDraftCount.current = drafts.length;
    if (!grew || !editable || columns.length === 0) return;
    beginEdit(total + drafts.length - 1, 0);
    requestAnimationFrame(() => {
      const el = scrollRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  }, [drafts, editable, total, columns.length, beginEdit]);

  // ------------------------------------------------------------------ layout

  const cycleSort = (col: number) => {
    setEditing(null); // display rows are about to move — drop the open editor
    // A display-space range would go stale too.
    setRange(null);
    anchorRef.current = null;
    setSort((s) => {
      if (!s || s.col !== col) return { col, dir: "asc" };
      if (s.dir === "asc") return { col, dir: "desc" };
      return null;
    });
  };

  // ---- range selection pointer handling (drag / shift-click)
  const onCellMouseDown = (e: React.MouseEvent, displayRow: number, ci: number) => {
    if (e.shiftKey && (anchorRef.current || range)) {
      // Shift-click extends from the anchor cell.
      const anchor = anchorRef.current ?? {
        row: range!.fromRow,
        col: range!.fromCol,
      };
      setRange({ fromRow: anchor.row, fromCol: anchor.col, toRow: displayRow, toCol: ci });
      setSel({ row: displayRow, col: ci });
    } else {
      anchorRef.current = { row: displayRow, col: ci };
      draggingRef.current = true;
      setDragging(true);
      setRange({ fromRow: displayRow, fromCol: ci, toRow: displayRow, toCol: ci });
      setSel({ row: displayRow, col: ci });
    }
  };

  const onCellMouseEnter = (displayRow: number, ci: number) => {
    if (!draggingRef.current) return;
    const anchor = anchorRef.current;
    if (!anchor) return;
    setRange({ fromRow: anchor.row, fromCol: anchor.col, toRow: displayRow, toCol: ci });
    setSel({ row: displayRow, col: ci });
  };

  const selectRowGutter = (displayRow: number) => {
    setRange(null);
    anchorRef.current = null;
    setSel({ row: displayRow, col: sel?.col ?? 0 });
  };

  const setWidth = (i: number, w: number) =>
    setWidths((ws) => ws.map((x, k) => (k === i ? clampW(w) : x)));

  const startResize = (e: React.MouseEvent, i: number) => {
    e.preventDefault();
    e.stopPropagation();
    const startX = e.clientX;
    const startW = widths[i] ?? MIN_COL_W;
    const onMove = (ev: MouseEvent) => setWidth(i, startW + (ev.clientX - startX));
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
    };
    document.body.style.cursor = "col-resize";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const autoFit = (i: number) => {
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.font = "12px ui-monospace, Menlo, monospace";
    const col = columns[i];
    if (!col) return;
    let w = ctx.measureText(col.name).width + 64; // header + badge
    const sample = Math.min(total, 200);
    for (let k = 0; k < sample; k++) {
      const v = rowAt(k)[i];
      if (!v) continue;
      w = Math.max(w, ctx.measureText(formatCell(v).text).width + 26);
    }
    setWidth(i, w);
  };

  const copySelection = (wholeRow: boolean) => {
    if (!sel) return;
    const values = displayRowValues(sel.row);
    let text = "";
    if (wholeRow) {
      text = values.map((v) => (v ? formatCell(v).text : "")).join("\t");
    } else {
      const v = values[sel.col];
      text = v ? formatCell(v).text : "";
    }
    void copyText(text).then((ok) =>
      toast(ok ? "success" : "error", ok ? "Copied" : "Copy failed"),
    );
  };

  const ensureVisible = (rowIdx: number) => {
    const el = scrollRef.current;
    if (!el) return;
    if (rowIdx >= total) {
      // Draft rows live below the virtual list — reveal the bottom.
      el.scrollTop = el.scrollHeight;
      return;
    }
    const top = rowIdx * ROW_H;
    if (top < el.scrollTop) {
      el.scrollTop = top;
    } else if (top + ROW_H > el.scrollTop + el.clientHeight) {
      el.scrollTop = top + ROW_H - el.clientHeight;
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    // Find works even with no cell selected.
    if ((e.ctrlKey || e.metaKey) && (e.key === "f" || e.key === "F")) {
      onRequestFind?.();
      e.preventDefault();
      return;
    }
    if (!sel) return;
    const maxRow = Math.max(0, totalRows - 1);
    const maxCol = Math.max(0, columns.length - 1);
    if ((e.ctrlKey || e.metaKey) && (e.key === "c" || e.key === "C")) {
      // A rectangular range copies as TSV; otherwise keep the single-cell /
      // whole-row behavior.
      if (rangeRef.current) copyRange();
      else copySelection(e.shiftKey);
      e.preventDefault();
      return;
    }
    // Shift+arrows grow the range from the anchor; plain arrows move the
    // single selection and drop any range.
    const extendSel = (dRow: number, dCol: number) => {
      const next = {
        row: Math.min(Math.max(0, sel.row + dRow), Math.max(0, maxRow)),
        col: Math.min(Math.max(0, sel.col + dCol), maxCol),
      };
      const anchor = anchorRef.current ?? { row: sel.row, col: sel.col };
      anchorRef.current = anchor;
      setRange({
        fromRow: anchor.row,
        fromCol: anchor.col,
        toRow: next.row,
        toCol: next.col,
      });
      setSel(next);
      ensureVisible(next.row);
    };
    const moveSel = (row: number, col: number) => {
      setRange(null);
      anchorRef.current = null;
      setSel({ row, col });
      ensureVisible(row);
    };
    switch (e.key) {
      case "ArrowDown":
        if (e.shiftKey) extendSel(1, 0);
        else moveSel(Math.min(maxRow, sel.row + 1), sel.col);
        e.preventDefault();
        break;
      case "ArrowUp":
        if (e.shiftKey) extendSel(-1, 0);
        else moveSel(Math.max(0, sel.row - 1), sel.col);
        e.preventDefault();
        break;
      case "ArrowRight":
        if (e.shiftKey) extendSel(0, 1);
        else moveSel(sel.row, Math.min(maxCol, sel.col + 1));
        e.preventDefault();
        break;
      case "ArrowLeft":
        if (e.shiftKey) extendSel(0, -1);
        else moveSel(sel.row, Math.max(0, sel.col - 1));
        e.preventDefault();
        break;
      case "Home":
        scrollRef.current?.scrollTo({ top: 0 });
        e.preventDefault();
        break;
      case "End":
        scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
        e.preventDefault();
        break;
      case "Delete":
      case "Backspace":
        if (editable) {
          if (sel.row >= total) onRemoveDraft?.(sel.row - total);
          else onToggleDeleteRow?.(origOf(sel.row)); // mark/unmark for deletion
          e.preventDefault();
        }
        break;
      case "Enter":
        if (editable && !e.ctrlKey && !e.metaKey) {
          const isDraftRow = sel.row >= total;
          const orig = isDraftRow ? -1 : origOf(sel.row);
          const rowDeleted = !isDraftRow && (deletedRows?.has(orig) ?? false);
          if (isDraftRow || (!rowDeleted && !pkCols.includes(sel.col))) {
            beginEdit(sel.row, sel.col);
          } else if (!isDraftRow) {
            onViewCell(orig, sel.col, sel.row);
          }
          e.preventDefault();
        }
        break;
      case "Escape":
        // Esc first drops the range, then the single selection.
        if (rangeRef.current) {
          setRange(null);
          anchorRef.current = null;
        } else {
          setSel(null);
        }
        break;
    }
  };

  const running = result.status === "running";

  if (columns.length === 0 && running) {
    return (
      <div className="flex-1 overflow-hidden bg-surface p-4">
        {[0, 1, 2].map((i) => (
          <div key={i} className="flex gap-3 py-1.5" style={{ opacity: 1 - i * 0.3 }}>
            {[120, 200, 90, 160, 110].map((w, k) => (
              <div
                key={k}
                className="h-3 animate-pulse rounded bg-slate-200"
                style={{ width: w }}
              />
            ))}
          </div>
        ))}
      </div>
    );
  }

  if (total === 0 && draftCount === 0) {
    return (
      <div className="grid flex-1 place-items-center bg-surface text-[13px] text-slate-500">
        {running ? "Waiting for rows…" : "Query returned no rows"}
      </div>
    );
  }

  /** One cell of an existing or draft row. */
  const renderCell = (
    values: Array<Value | undefined>,
    displayRow: number,
    ci: number,
    opts: { pending: Value | undefined; deleted: boolean; canEdit: boolean },
  ) => {
    const col = columns[ci]!;
    const isDraftRow = displayRow >= total;
    const base = values[ci] ?? NULL_VALUE;
    const value = opts.pending ?? base;
    const view = formatCell(value);
    // Untouched draft cells render empty (they are skipped on insert anyway).
    const draftEmpty = isDraftRow && values[ci] === undefined;
    const focused = sel?.row === displayRow && sel?.col === ci;
    const isEditing = editing?.row === displayRow && editing?.col === ci;
    const changed = opts.pending !== undefined;
    const editableCell =
      editable && opts.canEdit && (isDraftRow || !pkCols.includes(ci));
    // Find/range highlight (priority: active hit > hit > range > edited).
    const activeHit =
      findHits.length > 0 &&
      findHits[safeFindActive]?.row === displayRow &&
      findHits[safeFindActive]?.col === ci;
    const hit = !activeHit && (findSet?.has(`${displayRow}:${ci}`) ?? false);
    const inSelRange = inRange(displayRow, ci);
    const highlight = activeHit
      ? "bg-amber-300"
      : hit
        ? "bg-amber-100"
        : inSelRange
          ? "bg-accent/15"
          : "";
    const isFrozenCell = ci < frozenCols;
    const title = isDraftRow
      ? "Double-click to edit"
      : editableCell && !opts.deleted
        ? `${view.title} — double-click to edit · ⌥-double-click for full value`
        : pkCols.includes(ci) && editable
          ? `${view.title} — primary key (read-only)`
          : view.title;
    return (
      <div
        key={`${col.ordinal}-${col.name}`}
        data-cell={`${displayRow}:${ci}`}
        className={`grid-cell cursor-default ${focused ? "focused" : ""} ${
          changed ? "bg-amber-100" : ""
        } ${highlight}`}
        style={{
          width: widths[ci] ?? 120,
          minWidth: widths[ci] ?? 120,
          ...(isFrozenCell
            ? {
                position: "sticky" as const,
                left: frozenLeft[ci] ?? GUTTER_W,
                zIndex: 5,
                // Sticky cells paint over scrolled cells, so they need an
                // opaque background unless a highlight paints its own.
                background: highlight || changed ? undefined : "inherit",
              }
            : null),
        }}
        onMouseDown={(e) => onCellMouseDown(e, displayRow, ci)}
        onMouseEnter={() => onCellMouseEnter(displayRow, ci)}
        onClick={() => setSel({ row: displayRow, col: ci })}
        onDoubleClick={(e) => {
          if (editableCell && !opts.deleted && !e.altKey) {
            beginEdit(displayRow, ci);
          } else if (!isDraftRow) {
            onViewCell(origOf(displayRow), ci, displayRow);
          }
        }}
        title={title}
      >
        {isEditing ? (
          <input
            ref={editorRef}
            className="block h-full w-full border-0 bg-transparent p-0 text-[12.5px] text-slate-800 outline-none"
            value={editing?.value ?? ""}
            spellCheck={false}
            onChange={(e) =>
              setEditing((cur) => (cur ? { ...cur, value: e.target.value } : cur))
            }
            onKeyDown={onEditorKeyDown}
            onBlur={onEditorBlur}
            onDoubleClick={(e) => e.stopPropagation()}
          />
        ) : draftEmpty ? (
          <span className="block h-full" />
        ) : (
          <span
            className={`block truncate text-[12.5px] ${cellClass(view.tone)} ${
              opts.deleted ? "line-through opacity-50" : ""
            }`}
            style={{ textAlign: view.align }}
          >
            {view.text}
          </span>
        )}
      </div>
    );
  };

  return (
    <div
      ref={scrollRef}
      className={`custom-scrollbar relative flex-1 overflow-auto bg-surface outline-none ${
        dragging ? "select-none" : ""
      }`}
      onScroll={(e) => setScrollTop((e.target as HTMLDivElement).scrollTop)}
      onKeyDown={onKeyDown}
      tabIndex={0}
    >
      <div style={{ minWidth: totalWidth + 1 }}>
        {/* header */}
        <div
          className="sticky top-0 z-10 flex border-b border-slate-200/70 bg-sidebar"
          style={{ height: ROW_H }}
        >
          <div
            className="sticky left-0 z-[6] flex shrink-0 items-center justify-center bg-sidebar text-[11.5px] font-medium text-slate-400"

            style={{ width: GUTTER_W }}
          >
            #
          </div>
          {columns.map((col, i) => {
            const isFrozen = i < frozenCols;
            return (
              <div
                key={`${col.ordinal}-${col.name}`}
                className="group relative flex shrink-0 items-center"
                style={{
                  width: widths[i] ?? 120,
                  minWidth: widths[i] ?? 120,
                  ...(isFrozen
                    ? {
                        position: "sticky" as const,
                        left: frozenLeft[i] ?? GUTTER_W,
                        zIndex: 5,
                        background: "inherit",
                      }
                    : null),
                }}
              >
                <button
                  className="flex min-w-0 flex-1 items-center gap-1 px-2 text-left"
                  onClick={() => cycleSort(i)}
                  title={`${col.name} — ${col.native_type} · click to sort`}
                >
                  <span className="min-w-0 truncate text-[12px] font-medium text-slate-600">
                    {col.name}
                  </span>
                  <span
                    className={`ml-auto shrink-0 text-[10.5px] font-medium ${badgeTone(col.kind)}`}
                  >
                    {kindBadge(col.kind)}
                  </span>
                  {sort?.col === i && (
                    <span className="ml-auto flex shrink-0 items-center">
                      {sort.dir === "asc" ? (
                        <ArrowUp size={11} strokeWidth={2} className="text-accent" />
                      ) : (
                        <ArrowDown size={11} strokeWidth={2} className="text-accent" />
                      )}
                    </span>
                  )}
                </button>
                <button
                  className={`shrink-0 rounded p-0.5 ${
                    isFrozen
                      ? "text-accent"
                      : "hidden text-slate-300 hover:text-accent group-hover:block"
                  }`}
                  title={isFrozen ? "Unfreeze columns" : `Freeze up to "${col.name}"`}
                  onClick={() => setFrozen((f) => (f === i + 1 ? 0 : i + 1))}
                >
                  <Pin size={10} strokeWidth={2} />
                </button>
                <div
                  className="absolute top-0 right-0 h-full w-[5px] cursor-col-resize hover:bg-accent/40"
                  onMouseDown={(e) => startResize(e, i)}
                  onDoubleClick={() => autoFit(i)}
                />
              </div>
            );
          })}
        </div>

        {/* virtualized body */}
        <div className="relative" style={{ height: total * ROW_H }}>
          {Array.from({ length: end - start }, (_, k) => {
            const i = start + k;
            const orig = origOf(i);
            const base = rows[orig] ?? [];
            const pendingMap = edits?.get(orig);
            const deleted = deletedRows?.has(orig) ?? false;
            const edited = (pendingMap?.size ?? 0) > 0;
            const selectedRow = sel?.row === i;
            return (
              <div
                key={i}
                className={`grid-row absolute left-0 flex ${
                  selectedRow ? "selected" : i % 2 === 1 ? "bg-slate-50" : "bg-surface"
                } ${deleted ? "opacity-60" : ""} ${
                  deleted
                    ? "shadow-[inset_2px_0_0_0_#fb7185]"
                    : edited
                      ? "shadow-[inset_2px_0_0_0_#fbbf24]"
                      : ""
                }`}
                style={{ top: i * ROW_H, height: ROW_H, minWidth: totalWidth }}
              >
                <div
                  className={`sticky left-0 z-[6] flex shrink-0 cursor-pointer items-center justify-center border-r border-b border-slate-200 select-none ${
                    deleted
                      ? "bg-rose-50 text-rose-500 line-through"
                      : edited
                        ? "bg-amber-50 text-amber-600"
                        : "bg-sidebar text-slate-400"
                  }`}
                  style={{ width: GUTTER_W }}
                  onClick={() => selectRowGutter(i)}
                >
                  {i + 1}
                </div>
                {columns.map((_, ci) =>
                  renderCell(base, i, ci, {
                    pending: pendingMap?.get(ci),
                    deleted,
                    canEdit: !deleted,
                  }),
                )}
              </div>
            );
          })}
        </div>

        {/* draft (new, unsaved) rows appended below the virtual list */}
        {draftCount > 0 && (
          <div style={{ minWidth: totalWidth }}>
            {drafts.map((draft, di) => {
              const displayRow = total + di;
              const selectedRow = sel?.row === displayRow;
              const values = columns.map((_, ci) => draft.get(ci));
              return (
                <div
                  key={`draft-${di}`}
                  className={`grid-row flex shadow-[inset_2px_0_0_0_#34d399] ${
                    selectedRow ? "selected" : ""
                  }`}
                  style={{
                    height: ROW_H,
                    minWidth: totalWidth,
                    background: selectedRow ? undefined : "rgba(16, 185, 129, 0.06)",
                  }}
                >
                  <div
                    className="flex shrink-0 cursor-pointer items-center justify-center border-r border-b border-slate-200 bg-emerald-50 text-[13px] font-semibold text-emerald-600 select-none"
                    style={{ width: GUTTER_W }}
                    title="New row (unsaved)"
                    onClick={() => setSel({ row: displayRow, col: sel?.col ?? 0 })}
                  >
                    +
                  </div>
                  {columns.map((_, ci) =>
                    renderCell(values, displayRow, ci, {
                      pending: undefined,
                      deleted: false,
                      canEdit: true,
                    }),
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
