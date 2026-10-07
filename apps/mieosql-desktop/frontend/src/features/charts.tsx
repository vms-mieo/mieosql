// Hand-rolled SVG charts for the BI dashboard. No external chart library:
// everything is plain SVG that scales to the measured container width.
//
// Conventions:
// - `NaN` values are treated as "missing" and are skipped by every chart.
// - Each chart renders its own wrapper div (width measured via ResizeObserver)
//   so tiles can be any width without blurry stretching.

import {
  useEffect,
  useRef,
  useState,
  type RefObject,
} from "react";

import type { Value } from "../lib/bindings";

export type ChartPoint = { label: string; value: number };
export type ChartSeries = { name: string; color: string; points: ChartPoint[] };

// Calm Apple-like palette: the single accent blue + neutral grays. Series
// names/colors are arbitrary (grouped BI data), so hue carries no meaning and
// no semantic colors are used here.
export const SERIES_COLORS = ["#007aff", "#8e8e93", "#aeaeb2", "#6e6e73"] as const;

// Accent + neutral scale, ordered to keep adjacent slices distinguishable.
export const PIE_COLORS = [
  "#007aff", // accent
  "#8e8e93", // slate-500
  "#d2d2d7", // slate-300
  "#3a3a3c", // slate-700
  "#aeaeb2", // slate-400
  "#6e6e73", // slate-600
  "#e5e5ea", // slate-200
  "#2c2c2e", // slate-800
] as const;

const AXIS_LEFT = 54;
const AXIS_RIGHT = 14;
const AXIS_TOP = 14;
const AXIS_BOTTOM = 46;

// ---------------------------------------------------------------------------
// Value helpers (shared with BiPanel)
// ---------------------------------------------------------------------------

/** Human short number for axes / labels. */
export function formatNumber(n: number): string {
  if (!Number.isFinite(n)) return "–";
  const abs = Math.abs(n);
  if (abs >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (abs >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (abs >= 1e4) return `${(n / 1e3).toFixed(1)}k`;
  if (Number.isInteger(n)) return n.toLocaleString("en-US");
  return Number(Math.round(n * 100) / 100).toString();
}

/**
 * Best-effort numeric coercion for chart data: number-like wire values parse,
 * empty strings / NULL / blobs return null (skipped by aggregations).
 */
export function toNumber(value: Value): number | null {
  switch (value.t) {
    case "int":
    case "uint":
    case "decimal": {
      const n = Number(value.v);
      return Number.isFinite(n) ? n : null;
    }
    case "float":
      return Number.isFinite(value.v) ? value.v : null;
    case "bool":
      return value.v ? 1 : 0;
    case "text": {
      const trimmed = value.v.trim();
      if (trimmed === "") return null;
      const n = Number(trimmed);
      return Number.isFinite(n) ? n : null;
    }
    default:
      return null;
  }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

function useContainerWidth(
  fallback = 560,
): [RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(fallback);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const w = el.clientWidth;
      if (w > 0) setWidth(w);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  return [ref, width];
}

type Scale = {
  min: number;
  max: number;
  ticks: number[];
  y: (v: number) => number;
};

function buildScale(values: number[], plotTop: number, plotBottom: number): Scale {
  let min = 0;
  let max = 0;
  for (const v of values) {
    if (v < min) min = v;
    if (v > max) max = v;
  }
  if (max === min) max = min + 1;
  const ticks: number[] = [];
  const count = 4;
  for (let i = 0; i <= count; i++) ticks.push(min + ((max - min) * i) / count);
  return {
    min,
    max,
    ticks,
    y: (v: number) => plotBottom - ((v - min) / (max - min)) * (plotBottom - plotTop),
  };
}

function truncateLabel(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, Math.max(1, max - 1))}…` : s;
}

function AxisGrid({
  scale,
  plotLeft,
  plotRight,
}: {
  scale: Scale;
  plotLeft: number;
  plotRight: number;
}) {
  const zeroY = scale.y(0);
  return (
    <g>
      {scale.ticks.map((tick, i) => {
        const y = scale.y(tick);
        return (
          <g key={i}>
            <line
              x1={plotLeft}
              y1={y}
              x2={plotRight}
              y2={y}
              className="stroke-slate-200"
              strokeWidth={1}
            />
            <text
              x={plotLeft - 6}
              y={y + 3}
              fontSize={9}
              className="fill-slate-500"
              textAnchor="end"
            >
              {formatNumber(tick)}
            </text>
          </g>
        );
      })}
      <line
        x1={plotLeft}
        y1={zeroY}
        x2={plotRight}
        y2={zeroY}
        className="stroke-slate-300"
        strokeWidth={1}
      />
    </g>
  );
}

function XLabels({
  categories,
  plotLeft,
  band,
  height,
}: {
  categories: string[];
  plotLeft: number;
  band: number;
  height: number;
}) {
  const rotate = categories.length > 6 || categories.some((c) => c.length > 8);
  const baseY = height - AXIS_BOTTOM + 14;
  return (
    <g>
      {categories.map((label, i) => {
        const x = plotLeft + i * band + band / 2;
        return rotate ? (
          <text
            key={i}
            x={x}
            y={baseY}
            fontSize={9}
            className="fill-slate-600"
            textAnchor="end"
            transform={`rotate(-32 ${x} ${baseY})`}
          >
            <title>{label}</title>
            {truncateLabel(label, 16)}
          </text>
        ) : (
          <text
            key={i}
            x={x}
            y={baseY}
            fontSize={9}
            className="fill-slate-600"
            textAnchor="middle"
          >
            <title>{label}</title>
            {truncateLabel(label, 12)}
          </text>
        );
      })}
    </g>
  );
}

function SeriesLegend({ series, x }: { series: ChartSeries[]; x: number }) {
  if (series.length < 2) return null;
  let offset = 0;
  return (
    <g transform={`translate(${x}, 12)`}>
      {series.map((s) => {
        const itemX = offset;
        offset += 108;
        return (
          <g key={s.name} transform={`translate(${itemX}, 0)`}>
            <rect x={0} y={-7} width={8} height={8} rx={2} fill={s.color} />
            <text x={12} y={0.5} fontSize={9} className="fill-slate-700">
              {truncateLabel(s.name, 13)}
            </text>
          </g>
        );
      })}
    </g>
  );
}

function EmptyChart({ height }: { height: number }) {
  return (
    <div
      className="grid place-items-center text-[11px] text-slate-500"
      style={{ height }}
    >
      No data
    </div>
  );
}

// ---------------------------------------------------------------------------
// Bar chart (grouped when several series)
// ---------------------------------------------------------------------------

export function BarChart({
  series,
  height = 200,
}: {
  series: ChartSeries[];
  height?: number;
}) {
  const [ref, width] = useContainerWidth();
  const categories = series[0]?.points.map((p) => p.label) ?? [];
  const values: number[] = [];
  for (const s of series) {
    for (const p of s.points) if (Number.isFinite(p.value)) values.push(p.value);
  }
  const hasData = categories.length > 0 && values.length > 0;

  const viewWidth = Math.max(width, 140);
  const top = series.length > 1 ? 28 : AXIS_TOP;
  const plotLeft = AXIS_LEFT;
  const plotRight = Math.max(plotLeft + 40, viewWidth - AXIS_RIGHT);
  const plotW = plotRight - plotLeft;
  const scale = buildScale(values, top, height - AXIS_BOTTOM);
  const band = plotW / Math.max(1, categories.length);
  const groupW = Math.max(2, band * 0.72);
  const barW = groupW / Math.max(1, series.length);

  return (
    <div ref={ref} className="w-full">
      {!hasData ? (
        <EmptyChart height={height} />
      ) : (
        <svg width="100%" height={height} viewBox={`0 0 ${viewWidth} ${height}`}>
          <AxisGrid scale={scale} plotLeft={plotLeft} plotRight={plotRight} />
          {series.map((s, si) => (
            <g key={s.name}>
              {s.points.map((p, i) => {
                if (!Number.isFinite(p.value)) return null;
                const x = plotLeft + i * band + (band - groupW) / 2 + si * barW;
                const y = scale.y(p.value);
                const y0 = scale.y(0);
                const rectY = Math.min(y, y0);
                const rectH = Math.max(1, Math.abs(y0 - y));
                return (
                  <rect
                    key={i}
                    x={x + 0.5}
                    y={rectY}
                    width={Math.max(1, barW - 1)}
                    height={rectH}
                    rx={1.5}
                    fill={s.color}
                  >
                    <title>
                      {series.length > 1 ? `${s.name} · ` : ""}
                      {p.label} · {formatNumber(p.value)}
                    </title>
                  </rect>
                );
              })}
            </g>
          ))}
          <XLabels categories={categories} plotLeft={plotLeft} band={band} height={height} />
          <SeriesLegend series={series} x={plotLeft} />
        </svg>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Line chart (one polyline per series, gaps on missing values)
// ---------------------------------------------------------------------------

export function LineChart({
  series,
  height = 200,
}: {
  series: ChartSeries[];
  height?: number;
}) {
  const [ref, width] = useContainerWidth();
  const categories = series[0]?.points.map((p) => p.label) ?? [];
  const values: number[] = [];
  for (const s of series) {
    for (const p of s.points) if (Number.isFinite(p.value)) values.push(p.value);
  }
  const hasData = categories.length > 0 && values.length > 0;

  const viewWidth = Math.max(width, 140);
  const top = series.length > 1 ? 28 : AXIS_TOP;
  const plotLeft = AXIS_LEFT;
  const plotRight = Math.max(plotLeft + 40, viewWidth - AXIS_RIGHT);
  const plotW = plotRight - plotLeft;
  const scale = buildScale(values, top, height - AXIS_BOTTOM);
  const band = plotW / Math.max(1, categories.length);

  const lines = series.map((s) => {
    let open = false;
    const commands: string[] = [];
    const dots: Array<{ x: number; y: number; label: string; value: number }> = [];
    s.points.forEach((p, i) => {
      if (!Number.isFinite(p.value)) {
        open = false;
        return;
      }
      const x = plotLeft + i * band + band / 2;
      const y = scale.y(p.value);
      commands.push(`${open ? "L" : "M"} ${x} ${y}`);
      open = true;
      dots.push({ x, y, label: p.label, value: p.value });
    });
    return { series: s, d: commands.join(" "), dots };
  });

  return (
    <div ref={ref} className="w-full">
      {!hasData ? (
        <EmptyChart height={height} />
      ) : (
        <svg width="100%" height={height} viewBox={`0 0 ${viewWidth} ${height}`}>
          <AxisGrid scale={scale} plotLeft={plotLeft} plotRight={plotRight} />
          {lines.map(({ series: s, d, dots }) => (
            <g key={s.name}>
              <path d={d} fill="none" stroke={s.color} strokeWidth={1.8} />
              {dots.map((dot, i) => (
                <circle key={i} cx={dot.x} cy={dot.y} r={2.5} className="fill-surface" stroke={s.color} strokeWidth={1.5}>
                  <title>
                    {series.length > 1 ? `${s.name} · ` : ""}
                    {dot.label} · {formatNumber(dot.value)}
                  </title>
                </circle>
              ))}
            </g>
          ))}
          <XLabels categories={categories} plotLeft={plotLeft} band={band} height={height} />
          <SeriesLegend series={series} x={plotLeft} />
        </svg>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Pie chart (top 10 slices + "Other", legend with %)
// ---------------------------------------------------------------------------

function collapsePie(data: ChartPoint[], maxSlices: number): ChartPoint[] {
  const positive = data.filter((d) => Number.isFinite(d.value) && d.value > 0);
  if (positive.length <= maxSlices) return positive;
  const sorted = [...positive].sort((a, b) => b.value - a.value);
  const head = sorted.slice(0, Math.max(1, maxSlices - 1));
  const rest = sorted
    .slice(Math.max(1, maxSlices - 1))
    .reduce((acc, d) => acc + d.value, 0);
  return [...head, { label: "Other", value: rest }];
}

export function PieChart({
  data,
  height = 200,
}: {
  data: ChartPoint[];
  height?: number;
}) {
  const [ref, width] = useContainerWidth();
  const slices = collapsePie(data, 10);
  const total = slices.reduce((acc, s) => acc + s.value, 0);
  const hasData = slices.length > 0 && total > 0;

  const viewWidth = Math.max(width, 140);
  const radius = Math.max(24, Math.min(height / 2 - 14, viewWidth * 0.26));
  const cx = radius + 16;
  const cy = height / 2;
  let angle = -Math.PI / 2;
  const arcs = slices.map((slice, i) => {
    const sweep = (slice.value / total) * Math.PI * 2;
    const a0 = angle;
    const a1 = angle + sweep;
    angle = a1;
    const x0 = cx + radius * Math.cos(a0);
    const y0 = cy + radius * Math.sin(a0);
    const x1 = cx + radius * Math.cos(a1);
    const y1 = cy + radius * Math.sin(a1);
    const largeArc = sweep > Math.PI ? 1 : 0;
    return {
      slice,
      color: PIE_COLORS[i % PIE_COLORS.length],
      percent: (slice.value / total) * 100,
      d: `M ${cx} ${cy} L ${x0} ${y0} A ${radius} ${radius} 0 ${largeArc} 1 ${x1} ${y1} Z`,
    };
  });

  const legendX = cx + radius + 18;
  const rowHeight = 16;
  const visibleRows = Math.max(1, Math.floor((height - 14) / rowHeight));

  return (
    <div ref={ref} className="w-full">
      {!hasData ? (
        <EmptyChart height={height} />
      ) : (
        <svg width="100%" height={height} viewBox={`0 0 ${viewWidth} ${height}`}>
          {arcs.map((arc, i) => (
            <path
              key={i}
              d={arc.d}
              fill={arc.color}
              className="stroke-surface"
              strokeWidth={1}
            >
              <title>
                {arc.slice.label}: {formatNumber(arc.slice.value)} ({arc.percent.toFixed(1)}%)
              </title>
            </path>
          ))}
          {arcs.slice(0, visibleRows).map((arc, i) => (
            <g key={i} transform={`translate(${legendX}, ${12 + i * rowHeight})`}>
              <rect x={0} y={-1} width={8} height={8} rx={2} fill={arc.color} />
              <text x={12} y={7} fontSize={9.5} className="fill-slate-700">
                <title>{arc.slice.label}</title>
                {truncateLabel(arc.slice.label, 20)}
              </text>
              <text
                x={viewWidth - 8}
                y={7}
                fontSize={9.5}
                className="fill-slate-900"
                textAnchor="end"
              >
                {formatNumber(arc.slice.value)} · {arc.percent.toFixed(1)}%
              </text>
            </g>
          ))}
          {arcs.length > visibleRows && (
            <text
              x={legendX}
              y={12 + visibleRows * rowHeight + 7}
              fontSize={9}
              className="fill-slate-500"
            >
              +{arcs.length - visibleRows} more…
            </text>
          )}
        </svg>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Mini table
// ---------------------------------------------------------------------------

export function MiniTable({
  columns,
  rows,
  maxRows = 60,
}: {
  columns: string[];
  rows: string[][];
  maxRows?: number;
}) {
  if (columns.length === 0) {
    return <div className="grid h-full place-items-center text-[11px] text-slate-400">No data</div>;
  }
  const shown = rows.slice(0, maxRows);
  return (
    <div className="h-full overflow-auto">
      <table className="w-full border-collapse text-left text-[12.5px]">
        <thead className="sticky top-0 bg-sidebar text-slate-500">
          <tr>
            {columns.map((col) => (
              <th
                key={col}
                className="whitespace-nowrap border-b border-slate-200 px-2 py-1 text-[11.5px] font-semibold"
                title={col}
              >
                {col}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {shown.map((row, i) => (
            <tr
              key={i}
              className={`hover:bg-slate-900/[0.03] ${i % 2 === 1 ? "bg-slate-50" : ""}`}
            >
              {columns.map((_, j) => (
                <td
                  key={j}
                  className="max-w-[220px] truncate border-b border-slate-200/70 px-2 py-[3px] text-slate-700"
                  title={row[j] ?? ""}
                >
                  {row[j] ?? ""}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {rows.length > maxRows && (
        <div className="px-2 py-1 text-[11px] text-slate-500">
          Showing first {maxRows.toLocaleString()} of {rows.length.toLocaleString()} rows
        </div>
      )}
      {rows.length === 0 && (
        <div className="px-2 py-3 text-[11px] text-slate-400">No rows</div>
      )}
    </div>
  );
}
