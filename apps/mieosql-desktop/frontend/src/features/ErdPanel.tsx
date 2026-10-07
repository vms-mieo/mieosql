// ER Diagram panel (feature tab): renders a schema snapshot as an SVG
// entity-relationship diagram with pan/zoom, FK routing, search highlight
// and SVG/PNG export.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { save } from "@tauri-apps/plugin-dialog";
import {
  Database,
  FileDown,
  FileImage,
  Maximize,
  Network,
  RefreshCw,
  Search,
  ZoomIn,
  ZoomOut,
} from "lucide-react";

import type { SchemaSnapshot, Scope } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import type { Tab } from "../lib/store";
import { useStore } from "../lib/store";
import {
  BOX_HEADER_HEIGHT,
  COLUMN_ROW_HEIGHT,
  computeErdLayout,
  type ErdBox,
  type ErdEdge,
} from "./erd-layout";

type ErdPanelProps = {
  sessionId?: string;
  scope?: Scope;
};

type View = { x: number; y: number; k: number };

const MIN_ZOOM = 0.2;
const MAX_ZOOM = 3;
const FIT_PADDING = 36;

function clamp(v: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, v));
}

function truncate(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, Math.max(1, max - 1))}…` : s;
}

function compactCount(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}k`;
  return String(n);
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

export function ErdPanel({ tab }: { tab: Tab }) {
  const props = (tab.featureProps ?? {}) as ErdPanelProps;
  const activeSessionId = useStore((s) => s.activeSessionId);
  const selected = useStore((s) => s.selected);
  const sessions = useStore((s) => s.sessions);
  const toast = useStore((s) => s.toast);

  const sessionId = props.sessionId ?? activeSessionId ?? null;
  const session = sessions.find((s) => s.id === sessionId);

  const scope = useMemo<Scope>(() => {
    if (props.scope) return props.scope;
    if (selected && sessionId && selected.sessionId === sessionId) {
      return selected.scope;
    }
    return {};
  }, [props.scope, selected, sessionId]);

  const [snapshot, setSnapshot] = useState<SchemaSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestSeq = useRef(0);

  const load = useCallback(async () => {
    if (!sessionId) {
      setSnapshot(null);
      setError(null);
      return;
    }
    const seq = ++requestSeq.current;
    setLoading(true);
    setError(null);
    try {
      const snap = await api.schemaSnapshot(sessionId, scope, true, 300);
      if (seq === requestSeq.current) setSnapshot(snap);
    } catch (err) {
      if (seq === requestSeq.current) {
        setSnapshot(null);
        setError(errorText(err));
      }
    } finally {
      if (seq === requestSeq.current) setLoading(false);
    }
  }, [sessionId, scope]);

  useEffect(() => {
    void load();
  }, [load]);

  const layout = useMemo(
    () => (snapshot ? computeErdLayout(snapshot) : null),
    [snapshot],
  );

  // --- Pan / zoom ----------------------------------------------------------
  const containerRef = useRef<HTMLDivElement | null>(null);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [view, setView] = useState<View>({ x: 40, y: 30, k: 1 });
  const [dragging, setDragging] = useState(false);
  const dragRef = useRef<{ px: number; py: number; vx: number; vy: number } | null>(
    null,
  );

  const fit = useCallback(() => {
    const el = containerRef.current;
    if (!el || !layout) return;
    const rect = el.getBoundingClientRect();
    if (rect.width < 60 || rect.height < 60) return;
    const k = clamp(
      Math.min(
        (rect.width - FIT_PADDING * 2) / Math.max(1, layout.width),
        (rect.height - FIT_PADDING * 2) / Math.max(1, layout.height),
      ),
      MIN_ZOOM,
      MAX_ZOOM,
    );
    setView({
      x: (rect.width - layout.width * k) / 2,
      y: (rect.height - layout.height * k) / 2,
      k,
    });
  }, [layout]);

  useEffect(() => {
    if (layout) fit();
  }, [layout, fit]);

  const zoomAt = useCallback((cx: number, cy: number, factor: number) => {
    setView((v) => {
      const k = clamp(v.k * factor, MIN_ZOOM, MAX_ZOOM);
      const scale = k / v.k;
      return { k, x: cx - (cx - v.x) * scale, y: cy - (cy - v.y) * scale };
    });
  }, []);

  const zoomBy = useCallback(
    (factor: number) => {
      const el = containerRef.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      zoomAt(rect.width / 2, rect.height / 2, factor);
    },
    [zoomAt],
  );

  // Native (non-passive) wheel listener so preventDefault actually works.
  useEffect(() => {
    const el = containerRef.current;
    if (!el || !layout) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const factor = Math.exp(-e.deltaY * 0.0015);
      zoomAt(e.clientX - rect.left, e.clientY - rect.top, factor);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [layout, zoomAt]);

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    dragRef.current = { px: e.clientX, py: e.clientY, vx: view.x, vy: view.y };
    setDragging(true);
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag) return;
    setView((v) => ({
      ...v,
      x: drag.vx + (e.clientX - drag.px),
      y: drag.vy + (e.clientY - drag.py),
    }));
  };

  const endDrag = () => {
    dragRef.current = null;
    setDragging(false);
  };

  // --- Search highlight ----------------------------------------------------
  const [filter, setFilter] = useState("");
  const query = filter.trim().toLowerCase();
  const isMatch = useCallback(
    (name: string) => !query || name.toLowerCase().includes(query),
    [query],
  );

  // --- Export --------------------------------------------------------------
  const serializeSvg = useCallback((): string | null => {
    const svg = svgRef.current;
    if (!svg || !layout) return null;
    const clone = svg.cloneNode(true) as SVGSVGElement;
    clone.setAttribute("xmlns", "http://www.w3.org/2000/svg");
    clone.setAttribute("xmlns:xlink", "http://www.w3.org/1999/xlink");
    clone.setAttribute("width", String(layout.width));
    clone.setAttribute("height", String(layout.height));
    const bg = document.createElementNS("http://www.w3.org/2000/svg", "rect");
    bg.setAttribute("x", "0");
    bg.setAttribute("y", "0");
    bg.setAttribute("width", String(layout.width));
    bg.setAttribute("height", String(layout.height));
    bg.setAttribute("fill", "#ffffff");
    clone.insertBefore(bg, clone.firstChild);
    const xml = new XMLSerializer().serializeToString(clone);
    return `<?xml version="1.0" encoding="UTF-8"?>\n${xml}`;
  }, [layout]);

  const exportBaseName = useCallback((): string => {
    const parts = [snapshot?.database, snapshot?.schema].filter(
      (p): p is string => !!p,
    );
    const scope = (parts.length > 0 ? parts.join("-") : "schema")
      .replace(/[^\w.-]+/g, "_")
      .slice(0, 60);
    const stamp = new Date().toISOString().slice(0, 10);
    return `er-diagram-${scope}-${stamp}`;
  }, [snapshot]);

  const exportSvg = useCallback(async () => {
    try {
      const svgText = serializeSvg();
      if (!svgText) return;
      const path = await save({
        title: "Save ER diagram (SVG)",
        defaultPath: `${exportBaseName()}.svg`,
        filters: [{ name: "SVG image", extensions: ["svg"] }],
      });
      if (!path) return;
      await api.writeTextFile(path, svgText);
      toast("success", `Saved ER diagram (SVG)\n${path}`);
    } catch (err) {
      toast("error", `SVG export failed: ${errorText(err)}`);
    }
  }, [serializeSvg, exportBaseName, toast]);

  const exportPng = useCallback(async () => {
    try {
      const svgText = serializeSvg();
      if (!svgText || !layout) return;
      const url = URL.createObjectURL(
        new Blob([svgText], { type: "image/svg+xml;charset=utf-8" }),
      );
      try {
        const image = await new Promise<HTMLImageElement>((resolve, reject) => {
          const img = new Image();
          img.onload = () => resolve(img);
          img.onerror = () => reject(new Error("failed to rasterize the diagram"));
          img.src = url;
        });
        const maxSide = 16000;
        const scale = clamp(
          Math.min(2, maxSide / layout.width, maxSide / layout.height),
          0.25,
          2,
        );
        const canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(layout.width * scale));
        canvas.height = Math.max(1, Math.round(layout.height * scale));
        const ctx = canvas.getContext("2d");
        if (!ctx) throw new Error("canvas 2D is not available");
        ctx.fillStyle = "#ffffff";
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
        const dataUrl = canvas.toDataURL("image/png");
        const base64 = dataUrl.slice(dataUrl.indexOf(",") + 1);
        const path = await save({
          title: "Save ER diagram (PNG)",
          defaultPath: `${exportBaseName()}.png`,
          filters: [{ name: "PNG image", extensions: ["png"] }],
        });
        if (!path) return;
        await api.writeBinaryFile(path, base64);
        toast("success", `Saved ER diagram (PNG)\n${path}`);
      } finally {
        URL.revokeObjectURL(url);
      }
    } catch (err) {
      toast("error", `PNG export failed: ${errorText(err)}`);
    }
  }, [serializeSvg, exportBaseName, layout, toast]);

  // --- Render helpers ------------------------------------------------------
  const scopeLabel = [scope.database, scope.schema].filter(Boolean).join(".");

  const canRender = !!sessionId && !error && !!layout && layout.boxes.length > 0;

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-sidebar">
      {/* Toolbar */}
      <div className="flex h-[34px] shrink-0 items-center gap-2 border-b border-slate-200/70 bg-sidebar px-2">
        <Network size={13} strokeWidth={1.8} className="text-slate-500" />
        <span className="text-[13px] font-semibold text-slate-700">ER Diagram</span>
        {session && (
          <span className="truncate text-[11px] text-slate-500">
            {session.name}
            {scopeLabel ? ` · ${scopeLabel}` : ""}
          </span>
        )}
        {layout && (
          <span className="hidden text-[11px] text-slate-500 lg:inline">
            {layout.stats.tables} tables · {layout.stats.columns} columns ·{" "}
            {layout.stats.foreignKeys} FKs
          </span>
        )}
        {layout?.stats.truncated && (
          <span className="rounded bg-amber-100 px-1.5 py-0.5 text-[10.5px] font-semibold text-amber-700">
            truncated at 300 tables
          </span>
        )}
        {layout && layout.stats.unresolved > 0 && (
          <span
            className="rounded bg-slate-900/[0.06] px-1.5 py-0.5 text-[10.5px] font-semibold text-slate-600"
            title="FKs referencing a table outside this scope"
          >
            {layout.stats.unresolved} external FK
            {layout.stats.unresolved === 1 ? "" : "s"}
          </span>
        )}
        <div className="ml-auto flex items-center gap-1.5">
          <label className="flex h-7 items-center gap-1.5 rounded-md bg-slate-900/[0.04] px-2">
            <Search size={12} className="text-slate-500" />
            <input
              className="w-32 bg-transparent text-[12px] text-slate-800 placeholder:text-slate-500 focus:outline-none"
              placeholder="Find table…"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          </label>
          <ToolButton title="Reload schema" onClick={() => void load()} disabled={!sessionId}>
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </ToolButton>
          <ToolButton title="Fit to view (double-click canvas)" onClick={fit} disabled={!canRender}>
            <Maximize size={12} />
          </ToolButton>
          <ToolButton title="Export as SVG" onClick={() => void exportSvg()} disabled={!canRender}>
            <FileDown size={12} />
            SVG
          </ToolButton>
          <ToolButton title="Export as PNG (2x)" onClick={() => void exportPng()} disabled={!canRender}>
            <FileImage size={12} />
            PNG
          </ToolButton>
        </div>
      </div>

      {/* Body */}
      {!sessionId ? (
        <CenteredNote
          icon={<Database size={16} strokeWidth={1.6} className="text-slate-300" />}
          title="No connection selected"
          detail="Open a connection and select a database / schema, then reopen the ER diagram."
        />
      ) : error ? (
        <CenteredNote
          icon={<Database size={16} strokeWidth={1.6} className="text-rose-400" />}
          title="Could not load the schema"
          detail={error}
          action={
            <button
              className="h-7 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover"
              onClick={() => void load()}
            >
              Retry
            </button>
          }
        />
      ) : !snapshot && loading ? (
        <CenteredNote
          icon={<RefreshCw size={16} strokeWidth={1.6} className="animate-spin text-slate-300" />}
          title="Loading schema…"
          detail="Fetching tables, columns and foreign keys."
        />
      ) : snapshot && layout && layout.boxes.length === 0 ? (
        <CenteredNote
          icon={<Database size={16} strokeWidth={1.6} className="text-slate-300" />}
          title="No tables or views in this scope"
          detail="Pick another database or schema in the connection tree and try again."
        />
      ) : layout ? (
        <div
          ref={containerRef}
          className={`relative min-h-0 flex-1 overflow-hidden ${
            dragging ? "cursor-grabbing" : "cursor-grab"
          }`}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          onDoubleClick={fit}
        >
          <div
            className="absolute left-0 top-0 origin-top-left"
            style={{
              transform: `translate(${view.x}px, ${view.y}px) scale(${view.k})`,
            }}
          >
            <svg
              ref={svgRef}
              width={layout.width}
              height={layout.height}
              viewBox={`0 0 ${layout.width} ${layout.height}`}
              fontFamily="ui-sans-serif, system-ui, -apple-system, 'Segoe UI', sans-serif"
              xmlns="http://www.w3.org/2000/svg"
            >
              <defs>
                <pattern id="erd-grid" width={24} height={24} patternUnits="userSpaceOnUse">
                  <circle cx={1} cy={1} r={1} fill="#d2d2d7" />
                </pattern>
              </defs>
              <rect x={0} y={0} width={layout.width} height={layout.height} fill="#f2f2f7" />
              <rect x={0} y={0} width={layout.width} height={layout.height} fill="url(#erd-grid)" />

              {/* FK edges first so boxes sit on top of the lines. */}
              <g>
                {layout.edges.map((edge) => (
                  <EdgePath
                    key={edge.id}
                    edge={edge}
                    active={
                      !query || isMatch(edge.from.table) || isMatch(edge.to.table)
                    }
                  />
                ))}
              </g>

              <g>
                {layout.boxes.map((box) => {
                  const matched = isMatch(box.table.name);
                  return (
                    <TableBoxG
                      key={box.table.name}
                      box={box}
                      dim={!!query && !matched}
                      highlight={!!query && matched}
                    />
                  );
                })}
              </g>
            </svg>
          </div>

          {/* Legend */}
          <div className="pointer-events-none absolute bottom-2 left-2 flex items-center gap-3 rounded-md border border-slate-200/80 bg-white px-2 py-1 text-[11px] text-slate-600 shadow-[0_1px_3px_rgba(0,0,0,0.08)]">
            <span className="flex items-center gap-1">
              <LegendIcon kind="pk" /> PK
            </span>
            <span className="flex items-center gap-1">
              <LegendIcon kind="fk" /> FK
            </span>
            <span className="flex items-center gap-1">
              <LegendIcon kind="edge" /> FK reference
            </span>
          </div>

          {/* Zoom controls */}
          <div
            className="absolute bottom-2 right-2 flex items-center gap-0.5 rounded-md border border-slate-200/80 bg-white px-0.5 py-0.5 shadow-[0_1px_3px_rgba(0,0,0,0.08)]"
            onPointerDown={(e) => e.stopPropagation()}
            onDoubleClick={(e) => e.stopPropagation()}
          >
            <button
              className="grid h-6 w-6 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700"
              title="Zoom out"
              onClick={() => zoomBy(1 / 1.25)}
            >
              <ZoomOut size={12} />
            </button>
            <span className="w-9 text-center text-[11px] tabular-nums text-slate-500">
              {Math.round(view.k * 100)}%
            </span>
            <button
              className="grid h-6 w-6 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700"
              title="Zoom in"
              onClick={() => zoomBy(1.25)}
            >
              <ZoomIn size={12} />
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// SVG pieces
// ---------------------------------------------------------------------------

function EdgePath({ edge, active }: { edge: ErdEdge; active: boolean }) {
  const stroke = active ? "#8e8e93" : "#d2d2d7";
  const opacity = active ? 0.9 : 0.2;
  return (
    <g>
      <title>
        {`${edge.from.table}.${edge.from.column} → ${edge.to.table}.${edge.to.column}`}
      </title>
      <path
        d={edge.d}
        fill="none"
        stroke={stroke}
        strokeWidth={1.2}
        opacity={opacity}
      />
      <circle cx={edge.source.x} cy={edge.source.y} r={2.4} fill={stroke} opacity={opacity} />
      <path
        d="M -4 -2.8 L 4 0 L -4 2.8 Z"
        fill={stroke}
        opacity={opacity}
        transform={`translate(${edge.target.x} ${edge.target.y}) rotate(${edge.arrowAngle})`}
      />
    </g>
  );
}

function TableBoxG({
  box,
  dim,
  highlight,
}: {
  box: ErdBox;
  dim: boolean;
  highlight: boolean;
}) {
  const table = box.table;
  // Neutral header strip (matches bg-sidebar); kind is conveyed by the icon/title.
  const header = "#f2f2f7";
  const instance = table.name;
  return (
    <g opacity={dim ? 0.22 : 1}>
      <title>
        {`${instance}${table.comment ? ` — ${table.comment}` : ""}\n` +
          `${table.columns.length} column${table.columns.length === 1 ? "" : "s"}` +
          (table.rows_estimate != null
            ? ` · ~${table.rows_estimate.toLocaleString()} rows`
            : "")}
      </title>
      <rect
        x={box.x}
        y={box.y}
        width={box.width}
        height={box.height}
        rx={6}
        fill="#ffffff"
        stroke={highlight ? "#007aff" : "#e5e5ea"}
        strokeWidth={highlight ? 2 : 1}
      />
      <rect
        x={box.x}
        y={box.y}
        width={box.width}
        height={BOX_HEADER_HEIGHT}
        rx={6}
        fill={header}
      />
      <rect
        x={box.x}
        y={box.y + BOX_HEADER_HEIGHT - 8}
        width={box.width}
        height={8}
        fill={header}
      />
      <text
        x={box.x + 10}
        y={box.y + 19.5}
        fill="#1d1d1f"
        fontSize={12}
        fontWeight={600}
      >
        {truncate(table.name, 26)}
      </text>
      {table.rows_estimate != null && (
        <text
          x={box.x + box.width - 8}
          y={box.y + 19.5}
          textAnchor="end"
          fill="#8e8e93"
          fontSize={9}
        >
          ~{compactCount(table.rows_estimate)}
        </text>
      )}
      {box.columns.map((col, i) => {
        const rowY = box.y + BOX_HEADER_HEIGHT + i * COLUMN_ROW_HEIGHT;
        const centerY = rowY + COLUMN_ROW_HEIGHT / 2;
        return (
          <g key={col.name}>
            {i % 2 === 1 && (
              <rect
                x={box.x}
                y={rowY}
                width={box.width}
                height={COLUMN_ROW_HEIGHT}
                fill="#fafafc"
              />
            )}
            {col.pk && <KeyIcon x={box.x + 11} y={centerY} />}
            {col.fk && <ArrowIcon x={col.pk ? box.x + 20 : box.x + 11} y={centerY} />}
            <text
              x={box.x + 28}
              y={centerY + 3.5}
              fontSize={10.5}
              fill={col.pk ? "#1d1d1f" : "#3a3a3c"}
              fontWeight={col.pk ? 600 : 400}
            >
              {truncate(col.name, 22)}
            </text>
            <text
              x={box.x + box.width - 8}
              y={centerY + 3.5}
              textAnchor="end"
              fontSize={9}
              fill="#8e8e93"
            >
              {truncate(col.type, 18)}
            </text>
          </g>
        );
      })}
      {box.hiddenColumns > 0 && (
        <text
          x={box.x + 10}
          y={
            box.y +
            BOX_HEADER_HEIGHT +
            box.columns.length * COLUMN_ROW_HEIGHT +
            11
          }
          fontSize={9}
          fontStyle="italic"
          fill="#8e8e93"
        >
          +{box.hiddenColumns} more column{box.hiddenColumns === 1 ? "" : "s"}
        </text>
      )}
    </g>
  );
}

function KeyIcon({ x, y }: { x: number; y: number }) {
  return (
    <g stroke="#e68200" strokeWidth={1.3} fill="none" strokeLinecap="round">
      <circle cx={x - 1.2} cy={y} r={2.1} />
      <line x1={x + 0.9} y1={y} x2={x + 4.6} y2={y} />
      <line x1={x + 3.2} y1={y} x2={x + 3.2} y2={y + 2.1} />
    </g>
  );
}

function ArrowIcon({ x, y }: { x: number; y: number }) {
  return (
    <g
      stroke="#8e8e93"
      strokeWidth={1.3}
      fill="none"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <line x1={x - 2.6} y1={y} x2={x + 3} y2={y} />
      <polyline points={`${x + 0.7},${y - 1.9} ${x + 3},${y} ${x + 0.7},${y + 1.9}`} />
    </g>
  );
}

function LegendIcon({ kind }: { kind: "pk" | "fk" | "edge" }) {
  return (
    <svg width={14} height={12} viewBox="0 0 14 12">
      {kind === "pk" && <KeyIcon x={7} y={6} />}
      {kind === "fk" && <ArrowIcon x={7} y={6} />}
      {kind === "edge" && (
        <>
          <path d="M 1 6 C 5 6, 9 6, 13 6" fill="none" stroke="#8e8e93" strokeWidth={1.2} />
          <path d="M 9.4 3.4 L 13 6 L 9.4 8.6 Z" fill="#8e8e93" />
        </>
      )}
    </svg>
  );
}

// ---------------------------------------------------------------------------
// Small UI pieces
// ---------------------------------------------------------------------------

function ToolButton({
  title,
  onClick,
  disabled,
  children,
}: {
  title: string;
  onClick: () => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      className="flex h-7 items-center gap-1 rounded-md border border-slate-300 bg-white px-2 text-[12.5px] text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-40"
      title={title}
      onClick={onClick}
      disabled={disabled}
    >
      {children}
    </button>
  );
}

function CenteredNote({
  icon,
  title,
  detail,
  action,
}: {
  icon: ReactNode;
  title: string;
  detail: string;
  action?: ReactNode;
}) {
  return (
    <div className="grid min-h-0 flex-1 place-items-center bg-white">
      <div className="max-w-[420px] px-6 text-center">
        <div className="mb-2 flex justify-center">{icon}</div>
        <div className="text-[13px] font-semibold text-slate-700">{title}</div>
        <div className="mt-1 whitespace-pre-wrap break-words text-[11px] leading-relaxed text-slate-500">
          {detail}
        </div>
        {action && <div className="mt-3 flex justify-center">{action}</div>}
      </div>
    </div>
  );
}
