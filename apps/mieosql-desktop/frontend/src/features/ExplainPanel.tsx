// F2 — Visual Explain panel.
//
// Renders the `ExplainPlan` produced by the backend: a header with engine,
// format, timing and cost; a collapsible plan tree with relative cost bars;
// and a raw-output view. The plan comes from `tab.featureProps`
// ({ sessionId, sql, analyze? }) and re-runs on demand.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Code2,
  Loader2,
  RefreshCw,
  TriangleAlert,
} from "lucide-react";

import type {
  Engine,
  ExplainFormat,
  ExplainNode,
  ExplainPlan,
} from "../lib/bindings";
import { api, errorText } from "../lib/api";
import type { Tab } from "../lib/store";
import { useStore } from "../lib/store";

type ExplainFeatureProps = {
  sessionId?: string;
  sql?: string;
  analyze?: boolean;
};

const ENGINE_LABEL: Record<Engine, string> = {
  mysql: "MySQL",
  mariadb: "MariaDB",
  postgres: "PostgreSQL",
  sqlite: "SQLite",
  sql_server: "SQL Server",
  mongodb: "MongoDB",
  redis: "Redis",
  oracle: "Oracle",
  snowflake: "Snowflake",
};

const FORMAT_LABEL: Record<ExplainFormat, string> = {
  json: "JSON",
  text: "Text",
  query_plan: "Query plan",
};

export function ExplainPanel({ tab }: { tab: Tab }) {
  const props = (tab.featureProps ?? {}) as ExplainFeatureProps;
  const sessionId = props.sessionId ?? null;
  const sql = (props.sql ?? "").trim();
  const session = useStore(
    (s) => s.sessions.find((x) => x.id === sessionId) ?? null,
  );
  const isPostgres = session?.engine === "postgres";

  const [analyze, setAnalyze] = useState<boolean>(!!props.analyze);
  const [plan, setPlan] = useState<ExplainPlan | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showRaw, setShowRaw] = useState(false);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());

  // Guards against overlapping requests and StrictMode double-mount.
  const requestSeq = useRef(0);
  const startedRef = useRef(false);

  const run = useCallback(
    async (analyzeOverride?: boolean) => {
      if (!sessionId || !sql) {
        setError(
          !sessionId
            ? "No session attached to this explain tab."
            : "No SQL to explain — open a query tab and use Explain.",
        );
        return;
      }
      const effectiveAnalyze =
        isPostgres && (analyzeOverride ?? analyze);
      const seq = ++requestSeq.current;
      setLoading(true);
      setError(null);
      try {
        const result = await api.explainQuery(sessionId, sql, effectiveAnalyze);
        if (seq !== requestSeq.current) return;
        setPlan(result);
        setCollapsed(new Set());
      } catch (err) {
        if (seq !== requestSeq.current) return;
        setPlan(null);
        setError(errorText(err));
      } finally {
        if (seq === requestSeq.current) setLoading(false);
      }
    },
    [sessionId, sql, isPostgres, analyze],
  );

  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    void run();
  }, [run]);

  const toggleAnalyze = (next: boolean) => {
    setAnalyze(next);
    void run(next);
  };

  const toggleCollapsed = useCallback((path: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  // Largest cost in the tree + the path of the most expensive node.
  const { maxCost, hottestPath } = useMemo(() => {
    if (!plan) return { maxCost: 0, hottestPath: null as string | null };
    const hot: { path: string | null; cost: number } = {
      path: null,
      cost: 0,
    };
    const visit = (node: ExplainNode, path: string) => {
      const cost = node.cost ?? 0;
      if (cost > 0 && cost > hot.cost) {
        hot.cost = cost;
        hot.path = path;
      }
      node.children.forEach((child, index) =>
        visit(child, `${path}.${index}`),
      );
    };
    visit(plan.root, "0");
    return {
      maxCost: Math.max(plan.max_cost ?? 0, hot.cost),
      hottestPath: hot.path,
    };
  }, [plan]);

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-white">
      {/* Header: engine / format / timing / cost + actions */}
      <div className="flex h-[34px] shrink-0 items-center gap-2 overflow-x-auto border-b border-slate-200 bg-[#f1f5f9] px-3 text-[11px] text-slate-600">
        <span className="shrink-0 rounded bg-slate-200/80 px-1.5 py-px font-code text-[10px] font-medium uppercase text-slate-700">
          {plan ? ENGINE_LABEL[plan.engine] : session ? ENGINE_LABEL[session.engine] : "—"}
        </span>
        {plan && (
          <span className="shrink-0 text-[10px] text-slate-400">
            {FORMAT_LABEL[plan.format]}
          </span>
        )}
        {plan && (
          <span className="shrink-0 text-[10px] text-slate-500">
            {fmtDuration(plan.elapsed_ms)}
          </span>
        )}
        {plan?.analyzed && (
          <span
            className="shrink-0 rounded bg-emerald-100 px-1.5 py-px text-[10px] font-medium text-emerald-700"
            title="Plan captured from real execution (EXPLAIN ANALYZE)"
          >
            analyzed
          </span>
        )}
        {plan && plan.total_cost != null && (
          <span className="shrink-0 text-[10px] text-slate-500">
            total cost{" "}
            <span className="font-code tabular-nums text-slate-700">
              {fmtCost(plan.total_cost)}
            </span>
          </span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-3 pl-2">
          {isPostgres && (
            <label
              className="flex cursor-pointer items-center gap-1.5 whitespace-nowrap text-[11px] text-slate-600"
              title="Run EXPLAIN ANALYZE — the query actually executes"
            >
              <input
                type="checkbox"
                className="accent-sky-600"
                checked={analyze}
                disabled={loading}
                onChange={(e) => toggleAnalyze(e.target.checked)}
              />
              Analyze (real execution)
            </label>
          )}
          <button
            className="flex items-center gap-1 whitespace-nowrap rounded border border-slate-300 bg-white px-2 py-1 text-[10px] text-slate-700 hover:bg-slate-100 disabled:cursor-not-allowed disabled:opacity-50"
            onClick={() => setShowRaw((v) => !v)}
            disabled={!showRaw && !plan?.raw}
            title={showRaw ? "Show plan tree" : "Show raw output"}
          >
            <Code2 size={11} />
            {showRaw ? "Tree" : "Raw"}
          </button>
          <button
            className="flex items-center gap-1 whitespace-nowrap rounded bg-sky-600 px-2.5 py-1 text-[10px] font-medium text-white hover:bg-sky-700 disabled:cursor-not-allowed disabled:opacity-50"
            onClick={() => void run()}
            disabled={loading}
          >
            {loading ? (
              <Loader2 size={11} className="animate-spin" />
            ) : (
              <RefreshCw size={11} />
            )}
            Re-run
          </button>
        </div>
      </div>

      {/* SQL preview */}
      {sql && (
        <div
          className="shrink-0 truncate border-b border-slate-200 bg-white px-3 py-1 font-code text-[10px] text-slate-500"
          title={sql}
        >
          {sql}
        </div>
      )}

      {/* Body */}
      {loading && !plan ? (
        <div className="grid flex-1 place-items-center">
          <div className="flex items-center gap-2 text-[11px] text-slate-500">
            <Loader2 size={13} className="animate-spin text-sky-500" />
            {isPostgres && analyze
              ? "Running EXPLAIN ANALYZE…"
              : "Explaining query…"}
          </div>
        </div>
      ) : error ? (
        <div className="m-3 flex items-start gap-2 rounded border border-rose-200 bg-rose-50 px-3 py-2 text-[11px] text-rose-700">
          <TriangleAlert size={12} className="mt-px shrink-0" />
          <div className="min-w-0 flex-1 whitespace-pre-wrap break-words">
            {error}
          </div>
          <button
            className="shrink-0 rounded border border-rose-300 bg-white px-2 py-0.5 text-[10px] text-rose-600 hover:bg-rose-100"
            onClick={() => void run()}
          >
            Retry
          </button>
        </div>
      ) : !plan ? (
        <div className="grid flex-1 place-items-center">
          <div className="text-center">
            <div className="text-[11px] font-medium text-slate-500">
              Nothing to explain
            </div>
            <div className="mt-1 text-[10px] text-slate-400">
              Open a query tab and choose Explain.
            </div>
          </div>
        </div>
      ) : showRaw ? (
        <pre className="custom-scrollbar m-0 min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words bg-white p-3 font-code text-[11px] leading-relaxed text-slate-700">
          {plan.raw ?? "No raw plan output available."}
        </pre>
      ) : (
        <div className="custom-scrollbar min-h-0 flex-1 overflow-auto">
          <PlanTree
            node={plan.root}
            path="0"
            depth={0}
            collapsed={collapsed}
            onToggle={toggleCollapsed}
            hottestPath={hottestPath}
            maxCost={maxCost}
          />
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Plan tree
// ---------------------------------------------------------------------------

function PlanTree({
  node,
  path,
  depth,
  collapsed,
  onToggle,
  hottestPath,
  maxCost,
}: {
  node: ExplainNode;
  path: string;
  depth: number;
  collapsed: Set<string>;
  onToggle: (path: string) => void;
  hottestPath: string | null;
  maxCost: number;
}) {
  const hasChildren = node.children.length > 0;
  const isCollapsed = collapsed.has(path);
  const hot = hottestPath === path;
  const pct =
    node.cost != null && maxCost > 0
      ? Math.max(2, Math.min(100, (node.cost / maxCost) * 100))
      : 0;

  // Children are displayed by descending cost when the engine reports one.
  const sorted = useMemo(
    () =>
      node.children
        .map((child, index) => ({ child, index }))
        .sort((a, b) => (b.child.cost ?? 0) - (a.child.cost ?? 0)),
    [node.children],
  );

  return (
    <>
      <div
        className={`flex items-start gap-1.5 border-b border-slate-100 py-1.5 pr-3 ${
          hot
            ? "bg-amber-50 ring-1 ring-inset ring-amber-400"
            : "hover:bg-slate-50"
        }`}
        style={{ paddingLeft: 8 + depth * 14 }}
      >
        {hasChildren ? (
          <button
            className="mt-px shrink-0 rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-600"
            title={isCollapsed ? "Expand" : "Collapse"}
            onClick={() => onToggle(path)}
          >
            {isCollapsed ? (
              <ChevronRight size={11} />
            ) : (
              <ChevronDown size={11} />
            )}
          </button>
        ) : (
          <span className="mt-px inline-block w-[15px] shrink-0" />
        )}
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
            <span
              className={`text-[11px] ${
                hot ? "font-semibold text-amber-900" : "font-medium text-slate-800"
              }`}
            >
              {node.label}
            </span>
            {node.kind && (
              <span className="rounded bg-slate-100 px-1.5 py-px font-code text-[10px] text-slate-600">
                {node.kind}
              </span>
            )}
            {node.relation && !node.label.includes(node.relation) && (
              <span className="font-code text-[10px] text-slate-400">
                {node.relation}
              </span>
            )}
            <span className="ml-auto flex items-center gap-2 text-[10px]">
              {node.rows_estimate != null && (
                <span
                  className="whitespace-nowrap tabular-nums text-slate-500"
                  title="Planner row estimate"
                >
                  ~{fmtInt(node.rows_estimate)} rows
                </span>
              )}
              {node.actual_rows != null && (
                <span
                  className="whitespace-nowrap tabular-nums text-emerald-600"
                  title="Actual rows"
                >
                  {fmtInt(node.actual_rows)} actual
                </span>
              )}
              {node.actual_ms != null && (
                <span
                  className="whitespace-nowrap tabular-nums text-slate-500"
                  title="Actual total time"
                >
                  {fmtDuration(node.actual_ms)}
                </span>
              )}
              {node.cost != null && (
                <span
                  className="flex items-center gap-1"
                  title={`Estimated cost ${fmtCost(node.cost)}`}
                >
                  <span className="whitespace-nowrap rounded bg-sky-50 px-1.5 py-px font-code tabular-nums text-sky-700">
                    {fmtCost(node.cost)}
                  </span>
                  <span className="block h-1.5 w-24 overflow-hidden rounded-full bg-slate-100">
                    <span
                      className="block h-full rounded-full bg-sky-500"
                      style={{ width: `${pct}%` }}
                    />
                  </span>
                </span>
              )}
              {hot && (
                <span className="whitespace-nowrap rounded bg-amber-200/70 px-1.5 py-px font-medium text-amber-800">
                  most expensive
                </span>
              )}
            </span>
          </div>
          {node.detail && (
            <div
              className="mt-0.5 truncate text-[10px] text-slate-500"
              title={node.detail}
            >
              {node.detail}
            </div>
          )}
        </div>
      </div>
      {hasChildren &&
        !isCollapsed &&
        sorted.map(({ child, index }) => (
          <PlanTree
            key={`${path}.${index}`}
            node={child}
            path={`${path}.${index}`}
            depth={depth + 1}
            collapsed={collapsed}
            onToggle={onToggle}
            hottestPath={hottestPath}
            maxCost={maxCost}
          />
        ))}
    </>
  );
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

function fmtInt(n: number): string {
  return Math.round(n).toLocaleString();
}

function fmtCost(n: number): string {
  if (!Number.isFinite(n)) return "—";
  return n >= 1000 ? Math.round(n).toLocaleString() : n.toFixed(2);
}

function fmtDuration(ms: number): string {
  if (!Number.isFinite(ms)) return "—";
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)} s`;
  return `${ms < 10 ? ms.toFixed(2) : Math.round(ms)} ms`;
}
