import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, ArrowUp } from "lucide-react";

import type { ColumnMeta, Value } from "../lib/bindings";
import { compareValues, copyText, formatCell, kindBadge } from "../lib/format";
import type { ResultSet } from "../lib/store";
import { useStore } from "../lib/store";

export type GridApi = {
  scrollToTop: () => void;
  scrollToBottom: () => void;
  scrollBy: (delta: number) => void;
};
export type CellRef = { row: number; col: number };

const ROW_H = 22;
const GUTTER_W = 48;
const MIN_COL_W = 64;
const MAX_COL_W = 720;
const OVERSCAN = 8;

const NULL_VALUE = { t: "null" } as Value;

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

function badgeTone(kind: ColumnMeta["kind"]): string {
  switch (kind) {
    case "int":
    case "uint":
    case "float":
    case "decimal":
      return "text-amber-600";
    case "date":
    case "time":
    case "datetime":
    case "timestamptz":
      return "text-sky-600";
    case "json":
    case "uuid":
      return "text-violet-500";
    case "bytes":
    case "geometry":
      return "text-rose-500";
    case "bool":
      return "text-emerald-600";
    default:
      return "text-slate-400";
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
      return "font-code text-violet-700";
    case "binary":
      return "font-code text-rose-600";
    case "bool":
      return "font-code text-emerald-700";
    case "unsupported":
      return "italic text-amber-600";
    default:
      return "text-slate-800";
  }
}

type Props = {
  result: ResultSet;
  matchIndices: number[] | null;
  onViewCell: (rowIndex: number, colIndex: number) => void;
  onApi?: (api: GridApi) => void;
};

export function ResultGrid({ result, matchIndices, onViewCell, onApi }: Props) {
  const { columns, rows } = result;
  const toast = useStore((s) => s.toast);

  const [widths, setWidths] = useState<number[]>([]);
  const [sort, setSort] = useState<{ col: number; dir: "asc" | "desc" } | null>(
    null,
  );
  const [sel, setSel] = useState<CellRef | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(320);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Reset view state for a new query.
  useEffect(() => {
    setSort(null);
    setSel(null);
    setScrollTop(0);
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

  useEffect(() => {
    onApi?.({
      scrollToTop: () => scrollRef.current?.scrollTo({ top: 0 }),
      scrollToBottom: () =>
        scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight }),
      scrollBy: (delta) => scrollRef.current?.scrollBy({ top: delta }),
    });
  }, [onApi]);

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
  const rowAt = useCallback(
    (i: number): Value[] => {
      const row = indices ? rows[indices[i]!] : rows[i];
      return row ?? [];
    },
    [indices, rows],
  );

  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const visibleCount = Math.ceil(viewportH / ROW_H) + OVERSCAN * 2;
  const end = Math.min(total, start + visibleCount);

  const totalWidth = GUTTER_W + widths.reduce((a, b) => a + b, 0);

  const cycleSort = (col: number) => {
    setSort((s) => {
      if (!s || s.col !== col) return { col, dir: "asc" };
      if (s.dir === "asc") return { col, dir: "desc" };
      return null;
    });
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
    const row = rowAt(sel.row);
    const text = wholeRow
      ? row.map((v) => (v ? formatCell(v).text : "")).join("\t")
      : sel.col < row.length
        ? formatCell(row[sel.col]!).text
        : "";
    void copyText(text).then((ok) =>
      toast(ok ? "success" : "error", ok ? "Copied" : "Copy failed"),
    );
  };

  const ensureVisible = (rowIdx: number) => {
    const el = scrollRef.current;
    if (!el) return;
    const top = rowIdx * ROW_H;
    if (top < el.scrollTop) {
      el.scrollTop = top;
    } else if (top + ROW_H > el.scrollTop + el.clientHeight) {
      el.scrollTop = top + ROW_H - el.clientHeight;
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!sel) return;
    const maxRow = total - 1;
    const maxCol = Math.max(0, columns.length - 1);
    if ((e.ctrlKey || e.metaKey) && (e.key === "c" || e.key === "C")) {
      copySelection(e.shiftKey);
      e.preventDefault();
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        setSel({ row: Math.min(maxRow, sel.row + 1), col: sel.col });
        ensureVisible(Math.min(maxRow, sel.row + 1));
        e.preventDefault();
        break;
      case "ArrowUp":
        setSel({ row: Math.max(0, sel.row - 1), col: sel.col });
        ensureVisible(Math.max(0, sel.row - 1));
        e.preventDefault();
        break;
      case "ArrowRight":
        setSel({ row: sel.row, col: Math.min(maxCol, sel.col + 1) });
        e.preventDefault();
        break;
      case "ArrowLeft":
        setSel({ row: sel.row, col: Math.max(0, sel.col - 1) });
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
      case "Escape":
        setSel(null);
        break;
    }
  };

  const running = result.status === "running";

  if (columns.length === 0 && running) {
    return (
      <div className="flex-1 overflow-hidden bg-white p-4">
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

  if (total === 0) {
    return (
      <div className="grid flex-1 place-items-center bg-white text-slate-400">
        {running ? "Waiting for rows…" : "Query returned no rows"}
      </div>
    );
  }

  return (
    <div
      ref={scrollRef}
      className="custom-scrollbar relative flex-1 overflow-auto bg-white outline-none"
      onScroll={(e) => setScrollTop((e.target as HTMLDivElement).scrollTop)}
      onKeyDown={onKeyDown}
      tabIndex={0}
    >
      <div style={{ minWidth: totalWidth + 1 }}>
        {/* header */}
        <div
          className="sticky top-0 z-10 flex border-b border-slate-300 bg-[#f1f5f9]"
          style={{ height: 24 }}
        >
          <div
            className="flex shrink-0 items-center justify-center border-r border-slate-300 text-[10px] text-slate-400"
            style={{ width: GUTTER_W }}
          >
            #
          </div>
          {columns.map((col, i) => (
            <div
              key={`${col.ordinal}-${col.name}`}
              className="relative flex shrink-0 items-center border-r border-slate-300"
              style={{ width: widths[i] ?? 120, minWidth: widths[i] ?? 120 }}
            >
              <button
                className="flex min-w-0 flex-1 items-center gap-1 px-2 text-left"
                onClick={() => cycleSort(i)}
                title={`${col.name} — ${col.native_type} · click to sort`}
              >
                <span className="min-w-0 truncate text-[11px] font-semibold text-slate-700">
                  {col.name}
                </span>
                <span
                  className={`ml-auto shrink-0 text-[9px] font-medium ${badgeTone(col.kind)}`}
                >
                  {kindBadge(col.kind)}
                </span>
                {sort?.col === i &&
                  (sort.dir === "asc" ? (
                    <ArrowUp size={10} className="shrink-0 text-sky-600" />
                  ) : (
                    <ArrowDown size={10} className="shrink-0 text-sky-600" />
                  ))}
              </button>
              <div
                className="absolute top-0 right-0 h-full w-[5px] cursor-col-resize hover:bg-sky-400/60"
                onMouseDown={(e) => startResize(e, i)}
                onDoubleClick={() => autoFit(i)}
              />
            </div>
          ))}
        </div>

        {/* virtualized body */}
        <div className="relative" style={{ height: total * ROW_H }}>
          {Array.from({ length: end - start }, (_, k) => {
            const i = start + k;
            const row = rowAt(i);
            const selectedRow = sel?.row === i;
            return (
              <div
                key={i}
                className={`grid-row absolute left-0 flex ${
                  selectedRow ? "selected" : i % 2 === 1 ? "bg-[#fafbfd]" : "bg-white"
                }`}
                style={{ top: i * ROW_H, height: ROW_H, minWidth: totalWidth }}
              >
                <div
                  className="flex shrink-0 cursor-pointer items-center justify-center border-r border-b border-[#eef2f7] bg-[#f6f8fb] text-[10px] text-slate-400 select-none"
                  style={{ width: GUTTER_W }}
                  onClick={() => setSel({ row: i, col: sel?.col ?? 0 })}
                >
                  {i + 1}
                </div>
                {columns.map((col, ci) => {
                  const value = row[ci] ?? NULL_VALUE;
                  const view = formatCell(value);
                  const focused = sel?.row === i && sel?.col === ci;
                  return (
                    <div
                      key={`${col.ordinal}-${col.name}`}
                      className={`grid-cell cursor-default ${
                        focused ? "focused" : ""
                      }`}
                      style={{ width: widths[ci] ?? 120, minWidth: widths[ci] ?? 120 }}
                      onClick={() => setSel({ row: i, col: ci })}
                      onDoubleClick={() => onViewCell(i, ci)}
                      title={view.title}
                    >
                      <span
                        className={`block truncate ${cellClass(view.tone)}`}
                        style={{ textAlign: view.align }}
                      >
                        {view.text}
                      </span>
                    </div>
                  );
                })}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
