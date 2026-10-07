// BI Dashboard panel (feature tab): a multi-tile dashboard persisted in
// localStorage. Each tile runs its own SQL through `api.fetchRows`, aggregates
// on the client and renders a hand-rolled SVG chart (see `charts.tsx`).

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  BarChart3,
  ChevronDown,
  Copy,
  LineChart as LineChartIcon,
  Loader2,
  Maximize2,
  Minimize2,
  Pencil,
  PieChart as PieChartIcon,
  Plus,
  RefreshCw,
  Table2,
  Trash2,
  X,
} from "lucide-react";

import type { CollectedRows, SessionInfo } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { plainText } from "../lib/format";
import type { Tab } from "../lib/store";
import { useStore } from "../lib/store";
import {
  BarChart,
  LineChart,
  MiniTable,
  PieChart,
  SERIES_COLORS,
  toNumber,
  type ChartSeries,
} from "./charts";

// ---------------------------------------------------------------------------
// Types & persistence
// ---------------------------------------------------------------------------

type BiChartType = "bar" | "line" | "pie" | "table";
type BiAgg = "count" | "sum" | "avg" | "min" | "max";

type BiTile = {
  id: string;
  title: string;
  sessionId: string;
  sql: string;
  chart: BiChartType;
  x: string;
  y: string[];
  agg: BiAgg;
  width: 1 | 2;
  autoRefreshSec: number;
  params: Record<string, string>;
};

type TilesFile = { tiles: BiTile[] };

const BI_KEY = "mieosql.bi.v1";
const MAX_Y_COLUMNS = 3;
const MAX_GROUP_CATEGORIES = 80;
const MAX_TABLE_ROWS = 100;
const MAX_TABLE_COLUMNS = 12;

const CHART_TYPES: BiChartType[] = ["bar", "line", "pie", "table"];
const AGGS: BiAgg[] = ["count", "sum", "avg", "min", "max"];

const CHART_LABELS: Record<BiChartType, string> = {
  bar: "Bar",
  line: "Line",
  pie: "Pie",
  table: "Table",
};

const AGG_LABELS: Record<BiAgg, string> = {
  count: "Count",
  sum: "Sum",
  avg: "Average",
  min: "Min",
  max: "Max",
};

const INPUT_CLS =
  "rounded border border-slate-300 bg-white px-2 py-1 text-[11px] text-slate-700 focus:border-sky-400 focus:outline-none";
const PRIMARY_BTN =
  "flex items-center gap-1 rounded bg-sky-600 px-3 py-1.5 text-[11px] font-medium text-white hover:bg-sky-700 disabled:cursor-not-allowed disabled:opacity-50";
const SECONDARY_BTN =
  "rounded border border-slate-300 bg-white px-3 py-1.5 text-[11px] text-slate-700 hover:bg-slate-100";

function newId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `bi_${Date.now().toString(36)}_${Math.random().toString(36).slice(2)}`;
}

function sanitizeTile(raw: unknown): BiTile | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.id !== "string" || typeof r.sql !== "string") return null;
  const params: Record<string, string> = {};
  if (r.params && typeof r.params === "object") {
    for (const [key, value] of Object.entries(r.params as Record<string, unknown>)) {
      if (typeof value === "string") params[key] = value;
    }
  }
  return {
    id: r.id,
    title: typeof r.title === "string" ? r.title : "Untitled chart",
    sessionId: typeof r.sessionId === "string" ? r.sessionId : "",
    sql: r.sql,
    chart: CHART_TYPES.includes(r.chart as BiChartType)
      ? (r.chart as BiChartType)
      : "bar",
    x: typeof r.x === "string" ? r.x : "",
    y: Array.isArray(r.y)
      ? r.y.filter((v): v is string => typeof v === "string").slice(0, MAX_Y_COLUMNS)
      : [],
    agg: AGGS.includes(r.agg as BiAgg) ? (r.agg as BiAgg) : "sum",
    width: r.width === 2 ? 2 : 1,
    autoRefreshSec:
      typeof r.autoRefreshSec === "number" && Number.isFinite(r.autoRefreshSec)
        ? Math.max(0, Math.floor(r.autoRefreshSec))
        : 0,
    params,
  };
}

function loadTiles(): BiTile[] {
  try {
    const raw = localStorage.getItem(BI_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return [];
    const tiles = (parsed as { tiles?: unknown }).tiles;
    if (!Array.isArray(tiles)) return [];
    return tiles
      .map(sanitizeTile)
      .filter((tile): tile is BiTile => tile !== null);
  } catch {
    return [];
  }
}

// ---------------------------------------------------------------------------
// SQL parameter helpers
// ---------------------------------------------------------------------------

const PLACEHOLDER_RE = /\{\{\s*([A-Za-z0-9_]+)\s*\}\}/g;

/** Unique `{{name}}` placeholders in order of appearance. */
function placeholdersIn(sql: string): string[] {
  const found: string[] = [];
  for (const match of sql.matchAll(PLACEHOLDER_RE)) {
    const name = match[1];
    if (name && !found.includes(name)) found.push(name);
  }
  return found;
}

/** Replace `{{name}}` with a quoted literal (single quotes escaped). */
function substituteParams(sql: string, params: Record<string, string>): string {
  return sql.replace(/\{\{\s*([A-Za-z0-9_]+)\s*\}\}/g, (_match, name: string) => {
    const value = params[name];
    if (!value) return "''";
    return `'${value.replace(/'/g, "''")}'`;
  });
}

/** Drop stale params that no longer appear in the SQL. */
function keepKnownParams(
  params: Record<string, string>,
  sql: string,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const name of placeholdersIn(sql)) out[name] = params[name] ?? "";
  return out;
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

function aggregate(values: number[], agg: BiAgg, count: number): number {
  if (agg === "count") return count;
  if (values.length === 0) return NaN;
  switch (agg) {
    case "sum": {
      let acc = 0;
      for (const v of values) acc += v;
      return acc;
    }
    case "avg": {
      let acc = 0;
      for (const v of values) acc += v;
      return acc / values.length;
    }
    case "min": {
      let min = Infinity;
      for (const v of values) min = Math.min(min, v);
      return min;
    }
    case "max": {
      let max = -Infinity;
      for (const v of values) max = Math.max(max, v);
      return max;
    }
    default:
      return count;
  }
}

type TileChartData =
  | {
      kind: "series";
      series: ChartSeries[];
      rowCount: number;
      truncatedGroups: boolean;
    }
  | { kind: "table"; columns: string[]; rows: string[][]; rowCount: number }
  | { kind: "empty" };

function buildTileChart(data: CollectedRows | null, tile: BiTile): TileChartData {
  if (!data || data.columns.length === 0) return { kind: "empty" };

  if (tile.chart === "table") {
    const columns = data.columns.slice(0, MAX_TABLE_COLUMNS).map((c) => c.name);
    const rows = data.rows
      .slice(0, MAX_TABLE_ROWS)
      .map((row) => columns.map((_, j) => plainText(row[j] ?? { t: "null" })));
    return { kind: "table", columns, rows, rowCount: data.rows.length };
  }

  if (data.rows.length === 0) return { kind: "empty" };
  const names = data.columns.map((c) => c.name);
  const xIndex = names.indexOf(tile.x);
  if (xIndex < 0) return { kind: "empty" };
  const yIndexes = tile.y
    .map((name) => names.indexOf(name))
    .filter((index) => index >= 0);

  // Group rows by the formatted X value (insertion order is kept).
  const groups = new Map<string, { count: number; values: number[][] }>();
  for (const row of data.rows) {
    const label = plainText(row[xIndex] ?? { t: "null" }) || "(null)";
    let group = groups.get(label);
    if (!group) {
      group = { count: 0, values: yIndexes.map(() => []) };
      groups.set(label, group);
    }
    group.count += 1;
    yIndexes.forEach((colIndex, k) => {
      const n = toNumber(row[colIndex] ?? { t: "null" });
      if (n !== null) group.values[k]?.push(n);
    });
  }

  const entries = [...groups.entries()];
  const truncatedGroups = entries.length > MAX_GROUP_CATEGORIES;
  const visible = truncatedGroups ? entries.slice(0, MAX_GROUP_CATEGORIES) : entries;

  const series: ChartSeries[] = [];
  if (yIndexes.length === 0) {
    series.push({
      name: tile.agg === "count" ? "count(*)" : "rows",
      color: SERIES_COLORS[0],
      points: visible.map(([label, group]) => ({ label, value: group.count })),
    });
  } else {
    yIndexes.forEach((colIndex, k) => {
      series.push({
        name: names[colIndex] ?? `y${k + 1}`,
        color: SERIES_COLORS[k % SERIES_COLORS.length] ?? "#0ea5e9",
        points: visible.map(([label, group]) => ({
          label,
          value: aggregate(group.values[k] ?? [], tile.agg, group.count),
        })),
      });
    });
  }

  return { kind: "series", series, rowCount: data.rows.length, truncatedGroups };
}

function formatClock(ts: number): string {
  return new Date(ts).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

export function BiPanel({ tab }: { tab: Tab }) {
  const sessions = useStore((s) => s.sessions);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const tabs = useStore((s) => s.tabs);
  const activeTabId = useStore((s) => s.activeTabId);

  const [tiles, setTiles] = useState<BiTile[]>(loadTiles);
  const [form, setForm] = useState<{ mode: "new" | "edit"; tile: BiTile } | null>(
    null,
  );

  useEffect(() => {
    try {
      const payload: TilesFile = { tiles };
      localStorage.setItem(BI_KEY, JSON.stringify(payload));
    } catch {
      /* storage may be unavailable / full — the dashboard keeps working in memory */
    }
  }, [tiles]);

  const updateTile = useCallback((id: string, patch: Partial<BiTile>) => {
    setTiles((ts) => ts.map((t) => (t.id === id ? { ...t, ...patch } : t)));
  }, []);

  const removeTile = useCallback((id: string) => {
    setTiles((ts) => ts.filter((t) => t.id !== id));
  }, []);

  const duplicateTile = useCallback((id: string) => {
    setTiles((ts) => {
      const source = ts.find((t) => t.id === id);
      if (!source) return ts;
      return [
        ...ts,
        { ...source, id: newId(), title: `${source.title} (copy)` },
      ];
    });
  }, []);

  const openNew = useCallback(() => {
    const prefill = findPrefill(tabs, activeTabId);
    const sessionId =
      prefill?.sessionId ??
      tab.sessionId ??
      activeSessionId ??
      sessions[0]?.id ??
      "";
    setForm({
      mode: "new",
      tile: {
        id: newId(),
        title: "New chart",
        sessionId,
        sql: prefill?.sql ?? "",
        chart: "bar",
        x: "",
        y: [],
        agg: "sum",
        width: 1,
        autoRefreshSec: 0,
        params: {},
      },
    });
  }, [tabs, activeTabId, tab.sessionId, activeSessionId, sessions]);

  const saveForm = useCallback((tile: BiTile) => {
    setTiles((ts) => {
      const exists = ts.some((t) => t.id === tile.id);
      return exists ? ts.map((t) => (t.id === tile.id ? tile : t)) : [...ts, tile];
    });
    setForm(null);
  }, []);

  return (
    <div className="relative flex min-h-0 flex-1 flex-col bg-[#f8fafc]">
      <div className="flex h-[34px] shrink-0 items-center gap-2 border-b border-slate-200 bg-[#f1f5f9] px-2">
        <BarChart3 size={13} className="text-cyan-600" />
        <span className="text-[12px] font-semibold text-slate-700">BI Dashboard</span>
        <span className="text-[11px] text-slate-400">
          {tiles.length} tile{tiles.length === 1 ? "" : "s"} · saved locally
        </span>
        <button className={`ml-auto ${PRIMARY_BTN}`} onClick={openNew}>
          <Plus size={11} />
          Add chart
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto p-3">
        {tiles.length === 0 ? (
          <div className="grid h-full place-items-center">
            <div className="max-w-[380px] text-center">
              <BarChart3 size={26} className="mx-auto text-slate-300" />
              <div className="mt-2 text-[12px] font-semibold text-slate-600">
                No charts yet
              </div>
              <p className="mt-1 text-[11px] leading-relaxed text-slate-400">
                Build a dashboard from any SQL query. Tiles refresh on demand or
                on a schedule, and parameterize with{" "}
                <code className="font-mono">{"{{name}}"}</code>.
              </p>
              <button className={`mt-3 inline-flex ${PRIMARY_BTN}`} onClick={openNew}>
                <Plus size={11} />
                Add chart
              </button>
            </div>
          </div>
        ) : (
          <div className="grid grid-cols-2 gap-3">
            {tiles.map((tile) => (
              <BiTileView
                key={tile.id}
                tile={tile}
                onUpdate={(patch) => updateTile(tile.id, patch)}
                onRemove={() => removeTile(tile.id)}
                onDuplicate={() => duplicateTile(tile.id)}
                onEdit={() => setForm({ mode: "edit", tile })}
              />
            ))}
          </div>
        )}
      </div>

      {form && (
        <ChartForm
          key={`${form.mode}:${form.tile.id}`}
          mode={form.mode}
          initial={form.tile}
          sessions={sessions}
          onCancel={() => setForm(null)}
          onSave={saveForm}
        />
      )}
    </div>
  );
}

/** Prefill from the active result's SQL when there is one. */
function findPrefill(
  tabs: Tab[],
  activeTabId: string | null,
): { sql: string; sessionId: string | null } | null {
  const fromTab = (t: Tab): { sql: string; sessionId: string | null } | null => {
    if (t.kind !== "query") return null;
    const result = t.results[t.activeResult];
    if (!result || !result.sql.trim() || result.status !== "done") return null;
    return { sql: result.sql, sessionId: t.sessionId };
  };

  const active = tabs.find((t) => t.id === activeTabId);
  const fromActive = active ? fromTab(active) : null;
  if (fromActive) return fromActive;

  for (let i = tabs.length - 1; i >= 0; i--) {
    const tab = tabs[i];
    if (!tab) continue;
    const found = fromTab(tab);
    if (found) return found;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Add / edit form
// ---------------------------------------------------------------------------

function ChartForm({
  mode,
  initial,
  sessions,
  onCancel,
  onSave,
}: {
  mode: "new" | "edit";
  initial: BiTile;
  sessions: SessionInfo[];
  onCancel: () => void;
  onSave: (tile: BiTile) => void;
}) {
  const [draft, setDraft] = useState<BiTile>(initial);
  const [preview, setPreview] = useState<CollectedRows | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const placeholders = useMemo(() => placeholdersIn(draft.sql), [draft.sql]);

  const patch = (p: Partial<BiTile>) => setDraft((d) => ({ ...d, ...p }));

  const runPreview = async () => {
    if (!draft.sessionId) {
      setError("Pick a connection first.");
      return;
    }
    const sql = substituteParams(draft.sql, draft.params).trim();
    if (!sql) {
      setError("The SQL query is empty.");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const data = await api.fetchRows(draft.sessionId, sql, 200);
      setPreview(data);
      const names = data.columns.map((c) => c.name);
      setDraft((d) => ({
        ...d,
        x: names.includes(d.x) ? d.x : (names[0] ?? ""),
        y: d.y.filter((name) => names.includes(name)),
      }));
    } catch (err) {
      setPreview(null);
      setError(errorText(err));
    } finally {
      setLoading(false);
    }
  };

  const save = () => {
    if (!draft.sessionId) {
      setError("Pick a connection first.");
      return;
    }
    if (!draft.sql.trim()) {
      setError("The SQL query is empty.");
      return;
    }
    onSave({
      ...draft,
      title: draft.title.trim() || "Untitled chart",
      params: keepKnownParams(draft.params, draft.sql),
    });
  };

  const toggleY = (name: string) => {
    setDraft((d) => {
      if (d.y.includes(name)) {
        return { ...d, y: d.y.filter((n) => n !== name) };
      }
      if (d.y.length >= MAX_Y_COLUMNS) return d;
      return { ...d, y: [...d.y, name] };
    });
  };

  const previewColumnNames = preview ? preview.columns.map((c) => c.name) : [];
  const chipColumns =
    previewColumnNames.length > 0
      ? previewColumnNames
      : Array.from(new Set([draft.x, ...draft.y].filter(Boolean)));

  return (
    <div className="absolute inset-0 z-30 flex items-start justify-center overflow-y-auto bg-slate-900/25 p-6">
      <div className="w-full max-w-[760px] rounded-lg border border-slate-200 bg-white shadow-2xl">
        <header className="flex items-center justify-between border-b border-slate-200 px-3 py-2">
          <span className="text-[12px] font-semibold text-slate-700">
            {mode === "edit" ? "Edit chart" : "Add chart"}
          </span>
          <button
            className="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-600"
            onClick={onCancel}
            title="Close"
          >
            <X size={13} />
          </button>
        </header>

        <div className="space-y-3 px-3 py-3">
          <div className="flex flex-wrap gap-2">
            <Field label="Title" className="min-w-[200px] flex-1">
              <input
                className={`${INPUT_CLS} w-full`}
                value={draft.title}
                onChange={(e) => patch({ title: e.target.value })}
                placeholder="Chart title"
              />
            </Field>
            <Field label="Connection" className="min-w-[180px]">
              <select
                className={INPUT_CLS}
                value={draft.sessionId}
                onChange={(e) => patch({ sessionId: e.target.value })}
              >
                <option value="">— select —</option>
                {sessions.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                    {s.database ? ` · ${s.database}` : ""}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Auto refresh (s)">
              <input
                type="number"
                min={0}
                className={`${INPUT_CLS} w-24`}
                value={draft.autoRefreshSec}
                onChange={(e) =>
                  patch({
                    autoRefreshSec: Math.max(
                      0,
                      Math.floor(Number(e.target.value) || 0),
                    ),
                  })
                }
              />
            </Field>
          </div>

          <Field label="SQL query (use {{name}} for parameters)">
            <textarea
              className={`${INPUT_CLS} h-24 w-full resize-y font-mono`}
              value={draft.sql}
              onChange={(e) => patch({ sql: e.target.value })}
              placeholder="SELECT category, COUNT(*) AS total FROM orders GROUP BY category"
              spellCheck={false}
            />
          </Field>

          {placeholders.length > 0 && (
            <div className="flex flex-wrap items-center gap-2 rounded border border-amber-200 bg-amber-50 px-2 py-1.5">
              <span className="text-[10px] font-medium text-amber-700">
                Parameters:
              </span>
              {placeholders.map((name) => (
                <label
                  key={name}
                  className="flex items-center gap-1 text-[10px] text-slate-600"
                >
                  <span className="font-mono text-amber-700">{`{{${name}}}`}</span>
                  <input
                    className="w-28 rounded border border-slate-300 bg-white px-1.5 py-0.5 text-[10px]"
                    value={draft.params[name] ?? ""}
                    onChange={(e) =>
                      patch({ params: { ...draft.params, [name]: e.target.value } })
                    }
                    placeholder="value"
                  />
                </label>
              ))}
            </div>
          )}

          <div className="flex flex-wrap items-center gap-2">
            <button
              className={PRIMARY_BTN}
              onClick={() => void runPreview()}
              disabled={loading || !draft.sessionId}
            >
              {loading ? (
                <Loader2 size={11} className="animate-spin" />
              ) : (
                <RefreshCw size={11} />
              )}
              Run preview
            </button>
            {preview && (
              <span className="text-[10px] text-slate-500">
                {preview.rows.length.toLocaleString()} rows ·{" "}
                {preview.columns.length} columns · {preview.stats.elapsed_ms} ms
              </span>
            )}
            {error && <span className="text-[10px] text-rose-600">{error}</span>}
          </div>

          {preview && (
            <div className="max-h-[150px] overflow-hidden rounded border border-slate-200">
              <MiniTable
                columns={preview.columns
                  .slice(0, 8)
                  .map((c) => c.name)}
                rows={preview.rows.slice(0, 5).map((row) =>
                  preview.columns
                    .slice(0, 8)
                    .map((_, j) => plainText(row[j] ?? { t: "null" })),
                )}
                maxRows={5}
              />
            </div>
          )}

          <div className="flex flex-wrap gap-2">
            <Field label="Chart type">
              <select
                className={INPUT_CLS}
                value={draft.chart}
                onChange={(e) => patch({ chart: e.target.value as BiChartType })}
              >
                {CHART_TYPES.map((type) => (
                  <option key={type} value={type}>
                    {CHART_LABELS[type]}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="X axis (category)">
              <select
                className={INPUT_CLS}
                value={draft.x}
                onChange={(e) => patch({ x: e.target.value })}
                disabled={draft.chart === "table" || chipColumns.length === 0}
              >
                {draft.x === "" && <option value="">— pick a column —</option>}
                {chipColumns.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Aggregate">
              <select
                className={INPUT_CLS}
                value={draft.agg}
                onChange={(e) => patch({ agg: e.target.value as BiAgg })}
              >
                {AGGS.map((agg) => (
                  <option key={agg} value={agg}>
                    {AGG_LABELS[agg]}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Width">
              <select
                className={INPUT_CLS}
                value={draft.width}
                onChange={(e) => patch({ width: e.target.value === "2" ? 2 : 1 })}
              >
                <option value={1}>1x</option>
                <option value={2}>2x</option>
              </select>
            </Field>
          </div>

          {draft.chart !== "table" && chipColumns.length > 0 && (
            <div>
              <div className="mb-1 text-[10px] font-medium text-slate-500">
                Y columns{" "}
                <span className="font-normal text-slate-400">
                  (up to {MAX_Y_COLUMNS} — leave empty to chart row count)
                </span>
              </div>
              <div className="flex flex-wrap gap-1.5">
                {chipColumns.map((name) => {
                  const selected = draft.y.includes(name);
                  const disabled = !selected && draft.y.length >= MAX_Y_COLUMNS;
                  return (
                    <button
                      key={name}
                      className={`rounded-full border px-2 py-0.5 text-[10px] ${
                        selected
                          ? "border-sky-500 bg-sky-100 text-sky-700"
                          : "border-slate-300 bg-white text-slate-600 hover:bg-slate-100"
                      } disabled:cursor-not-allowed disabled:opacity-40`}
                      onClick={() => toggleY(name)}
                      disabled={disabled}
                    >
                      {name}
                    </button>
                  );
                })}
              </div>
            </div>
          )}

          {!preview && (
            <p className="text-[10px] text-slate-400">
              Run a preview to load the result columns, then pick the X axis and
              value columns.
            </p>
          )}
        </div>

        <footer className="flex items-center justify-end gap-2 border-t border-slate-200 px-3 py-2">
          <button className={SECONDARY_BTN} onClick={onCancel}>
            Cancel
          </button>
          <button className={PRIMARY_BTN} onClick={save}>
            Save chart
          </button>
        </footer>
      </div>
    </div>
  );
}

function Field({
  label,
  className,
  children,
}: {
  label: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <label className={`block ${className ?? ""}`}>
      <span className="mb-0.5 block text-[10px] font-medium text-slate-500">
        {label}
      </span>
      {children}
    </label>
  );
}

// ---------------------------------------------------------------------------
// Tile
// ---------------------------------------------------------------------------

type TileState = {
  loading: boolean;
  data: CollectedRows | null;
  error: string | null;
  elapsedMs: number | null;
  updatedAt: number | null;
};

function BiTileView({
  tile,
  onUpdate,
  onRemove,
  onDuplicate,
  onEdit,
}: {
  tile: BiTile;
  onUpdate: (patch: Partial<BiTile>) => void;
  onRemove: () => void;
  onDuplicate: () => void;
  onEdit: () => void;
}) {
  const [state, setState] = useState<TileState>({
    loading: true,
    data: null,
    error: null,
    elapsedMs: null,
    updatedAt: null,
  });
  const [nonce, setNonce] = useState(0);
  const [menuOpen, setMenuOpen] = useState(false);
  const [draftParams, setDraftParams] = useState<Record<string, string>>(
    tile.params,
  );
  const menuRef = useRef<HTMLDivElement | null>(null);

  const placeholders = useMemo(() => placeholdersIn(tile.sql), [tile.sql]);
  const effectiveSql = useMemo(
    () => substituteParams(tile.sql, tile.params),
    [tile.sql, tile.params],
  );

  // Re-sync param inputs when the tile is edited from the form.
  useEffect(() => {
    setDraftParams(tile.params);
  }, [tile.id, tile.params]);

  const load = useCallback(async () => {
    if (!tile.sessionId) {
      setState({
        loading: false,
        data: null,
        error: "No connection selected for this chart",
        elapsedMs: null,
        updatedAt: null,
      });
      return;
    }
    setState((s) => ({ ...s, loading: true, error: null }));
    const started = performance.now();
    try {
      const data = await api.fetchRows(tile.sessionId, effectiveSql, 5000);
      setState({
        loading: false,
        data,
        error: null,
        elapsedMs: data.stats.elapsed_ms ?? Math.round(performance.now() - started),
        updatedAt: Date.now(),
      });
    } catch (err) {
      setState({
        loading: false,
        data: null,
        error: errorText(err),
        elapsedMs: null,
        updatedAt: Date.now(),
      });
    }
  }, [tile.sessionId, effectiveSql]);

  useEffect(() => {
    void load();
  }, [load, nonce]);

  // Auto refresh: one interval per tile, cleaned up on change / unmount.
  useEffect(() => {
    if (!(tile.autoRefreshSec > 0)) return;
    const timer = window.setInterval(() => {
      setNonce((n) => n + 1);
    }, tile.autoRefreshSec * 1000);
    return () => window.clearInterval(timer);
  }, [tile.autoRefreshSec]);

  // Close the tile menu on outside clicks.
  useEffect(() => {
    if (!menuOpen) return;
    const onDocDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", onDocDown);
    return () => document.removeEventListener("mousedown", onDocDown);
  }, [menuOpen]);

  const chartData = useMemo(() => buildTileChart(state.data, tile), [state.data, tile]);

  const commitParams = () => {
    const keys = new Set([
      ...Object.keys(tile.params),
      ...Object.keys(draftParams),
    ]);
    for (const key of keys) {
      if ((tile.params[key] ?? "") !== (draftParams[key] ?? "")) {
        onUpdate({ params: draftParams });
        return;
      }
    }
  };

  const ChartIcon =
    tile.chart === "bar"
      ? BarChart3
      : tile.chart === "line"
        ? LineChartIcon
        : tile.chart === "pie"
          ? PieChartIcon
          : Table2;

  return (
    <div
      className={`flex min-h-0 flex-col overflow-hidden rounded-lg border border-slate-200 bg-white shadow-sm ${
        tile.width === 2 ? "col-span-2" : ""
      }`}
    >
      {/* Header */}
      <div className="flex items-center gap-1.5 border-b border-slate-200 bg-[#f8fafc] px-2.5 py-1.5">
        <ChartIcon size={12} className="shrink-0 text-cyan-600" />
        <span
          className="min-w-0 flex-1 truncate text-[12px] font-semibold text-slate-700"
          title={tile.title}
        >
          {tile.title}
        </span>
        {tile.autoRefreshSec > 0 && (
          <span className="rounded bg-emerald-50 px-1.5 py-0.5 text-[9px] text-emerald-600">
            auto {tile.autoRefreshSec}s
          </span>
        )}
        <button
          className="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-600"
          title="Refresh now"
          onClick={() => setNonce((n) => n + 1)}
        >
          <RefreshCw size={11} className={state.loading ? "animate-spin" : ""} />
        </button>
        <div className="relative" ref={menuRef}>
          <button
            className="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-600"
            title="Chart menu"
            onClick={() => setMenuOpen((v) => !v)}
          >
            <ChevronDown size={12} />
          </button>
          {menuOpen && (
            <div className="absolute right-0 top-full z-20 mt-1 w-40 rounded border border-slate-200 bg-white py-1 text-[11px] shadow-lg">
              <MenuItem
                icon={<Pencil size={11} />}
                label="Edit chart"
                onClick={() => {
                  setMenuOpen(false);
                  onEdit();
                }}
              />
              <MenuItem
                icon={
                  tile.width === 2 ? <Minimize2 size={11} /> : <Maximize2 size={11} />
                }
                label={tile.width === 2 ? "Size 1x" : "Size 2x"}
                onClick={() => {
                  setMenuOpen(false);
                  onUpdate({ width: tile.width === 2 ? 1 : 2 });
                }}
              />
              <MenuItem
                icon={<Copy size={11} />}
                label="Duplicate"
                onClick={() => {
                  setMenuOpen(false);
                  onDuplicate();
                }}
              />
              <MenuItem
                icon={<Trash2 size={11} />}
                label="Remove"
                danger
                onClick={() => {
                  setMenuOpen(false);
                  onRemove();
                }}
              />
            </div>
          )}
        </div>
      </div>

      {/* Parameters */}
      {placeholders.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5 border-b border-slate-100 bg-amber-50/60 px-2.5 py-1">
          {placeholders.map((name) => (
            <label
              key={name}
              className="flex items-center gap-1 text-[10px] text-slate-600"
              title={`{{${name}}}`}
            >
              <span className="font-mono text-amber-700">{name}</span>
              <input
                className="w-24 rounded border border-slate-300 bg-white px-1 py-0.5 text-[10px]"
                value={draftParams[name] ?? ""}
                onChange={(e) =>
                  setDraftParams((d) => ({ ...d, [name]: e.target.value }))
                }
                onBlur={commitParams}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitParams();
                }}
              />
            </label>
          ))}
        </div>
      )}

      {/* Body */}
      <div className="relative min-h-[150px] flex-1 px-1.5 py-1">
        {state.error ? (
          <div className="grid h-full min-h-[140px] place-items-center px-4 text-center text-[11px] text-rose-600">
            {state.error}
          </div>
        ) : !state.data ? (
          <div className="grid h-full min-h-[140px] place-items-center gap-1 text-[11px] text-slate-400">
            {state.loading ? (
              <span className="flex items-center gap-1.5">
                <Loader2 size={12} className="animate-spin" /> Loading…
              </span>
            ) : (
              "No data"
            )}
          </div>
        ) : chartData.kind === "table" ? (
          <div className="max-h-[260px] overflow-auto">
            <MiniTable columns={chartData.columns} rows={chartData.rows} />
          </div>
        ) : chartData.kind === "series" ? (
          tile.chart === "bar" ? (
            <BarChart series={chartData.series} />
          ) : tile.chart === "line" ? (
            <LineChart series={chartData.series} />
          ) : (
            <PieChart data={chartData.series[0]?.points ?? []} />
          )
        ) : (
          <div className="grid h-full min-h-[140px] place-items-center text-[11px] text-slate-400">
            No data — check the X/Y fields
          </div>
        )}
        {state.loading && state.data && (
          <Loader2
            size={12}
            className="absolute right-2 top-1 animate-spin text-sky-500"
          />
        )}
      </div>

      {/* Footer */}
      <div className="flex items-center gap-1.5 border-t border-slate-100 px-2.5 py-1 text-[10px] text-slate-400">
        <span>{(state.data?.rows.length ?? 0).toLocaleString()} rows</span>
        <span>·</span>
        <span>{state.elapsedMs != null ? `${state.elapsedMs} ms` : "–"}</span>
        <span>·</span>
        <span>
          updated {state.updatedAt ? formatClock(state.updatedAt) : "–"}
        </span>
        {chartData.kind === "series" && chartData.truncatedGroups && (
          <span className="ml-auto text-amber-600">first {MAX_GROUP_CATEGORIES} groups</span>
        )}
      </div>
    </div>
  );
}

function MenuItem({
  icon,
  label,
  danger,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  danger?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      className={`flex w-full items-center gap-2 px-2.5 py-1 text-left hover:bg-slate-100 ${
        danger ? "text-rose-600" : "text-slate-700"
      }`}
      onClick={onClick}
    >
      {icon}
      {label}
    </button>
  );
}
