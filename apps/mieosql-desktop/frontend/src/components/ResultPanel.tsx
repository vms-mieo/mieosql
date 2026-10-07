import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  Copy,
  Database,
  FileDown,
  FileJson,
  FileSpreadsheet,
  FileText,
  Filter,
  Grid3x3,
  Pin,
  RotateCw,
  ScrollText,
  Terminal,
} from "lucide-react";

import type { ApiError, ExportFormat } from "../lib/bindings";
import { copyText, plainText, toCsv, toJsonLines } from "../lib/format";
import type { LogMessage, ResultSet, Tab } from "../lib/store";
import { EXPORT_FORMATS, exportFormatLabel, useStore } from "../lib/store";
import type { GridApi } from "./ResultGrid";
import { ResultGrid } from "./ResultGrid";
import { CellViewer } from "./CellViewer";

type SubTab = "result" | "messages" | "status";

export function ResultPanel({ tab }: { tab: Tab }) {
  const store = useStore();
  const [subTab, setSubTab] = useState<SubTab>("result");
  const [filter, setFilter] = useState("");
  const deferredFilter = useDeferredValue(filter);
  const [viewer, setViewer] = useState<{ row: number; col: number } | null>(null);
  const [exportOpen, setExportOpen] = useState(false);
  const gridApiRef = useRef<GridApi | null>(null);

  const result: ResultSet | undefined = tab.results[tab.activeResult];
  const running = result?.status === "running";

  // Switch to Result after a new run starts.
  const lastResultCount = useRef(tab.results.length);
  useEffect(() => {
    if (tab.results.length !== lastResultCount.current) {
      lastResultCount.current = tab.results.length;
      setSubTab("result");
      setFilter("");
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

  const rerun = () => {
    if (tab.kind === "table") void store.runPreview(tab.id);
    else if (result?.sql) void store.requestRun(tab.id, result.sql);
  };

  const copyData = (kind: "csv" | "json") => {
    if (!result) return;
    const text =
      kind === "csv"
        ? toCsv(result.columns, result.rows)
        : toJsonLines(result.columns, result.rows);
    void copyText(text).then((ok) =>
      store.toast(ok ? "success" : "error", ok ? `Copied ${kind.toUpperCase()}` : "Copy failed"),
    );
  };

  const messageCount = tab.messages.length;
  const errorCount = tab.messages.filter((m) => m.level === "error").length;

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-sidebar">
      {/* sub-tab bar + timing */}
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-slate-200/80 bg-sidebar px-2 select-none">
        <div className="flex h-full items-center">
          {tab.results.map((r, i) => (
            <button
              key={r.queryId}
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
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-slate-200/80 bg-white px-3 text-[12.5px]">
        <div className="flex items-center gap-1">
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
              className="flex h-7 items-center gap-1 rounded-md px-2 text-[12.5px] font-medium text-slate-600 hover:bg-slate-900/[0.05] disabled:opacity-40"
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
              </div>
            )}
          </div>
          <div className="ml-1 flex h-7 items-center rounded-md bg-slate-900/[0.04] px-2 focus-within:ring-2 focus-within:ring-accent/40">
            <Filter size={12} strokeWidth={1.7} className="mr-1 text-slate-500" />
            <input
              className="w-44 border-none bg-transparent p-0 text-[12.5px] placeholder:text-slate-400 focus:outline-none"
              placeholder="Filter loaded rows…"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          </div>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-[12px] text-slate-500">
            {matchIndices
              ? `${matchIndices.length.toLocaleString()} / ${(result?.rows.length ?? 0).toLocaleString()} rows`
              : `${(result?.rows.length ?? 0).toLocaleString()} rows`}
          </span>
          <div className="flex items-center rounded-md bg-slate-900/[0.04] p-0.5">
            <button
              className="grid h-6 w-6 place-items-center rounded-[5px] text-slate-500 hover:bg-slate-900/[0.06] hover:text-slate-700"
              title="First row"
              onClick={() => gridApiRef.current?.scrollToTop()}
            >
              <ChevronsLeft size={12} strokeWidth={1.7} />
            </button>
            <button
              className="grid h-6 w-6 place-items-center rounded-[5px] text-slate-500 hover:bg-slate-900/[0.06] hover:text-slate-700"
              title="Scroll up"
              onClick={() => gridApiRef.current?.scrollBy(-220)}
            >
              <ChevronLeft size={12} strokeWidth={1.7} />
            </button>
            <button
              className="grid h-6 w-6 place-items-center rounded-[5px] text-slate-500 hover:bg-slate-900/[0.06] hover:text-slate-700"
              title="Scroll down"
              onClick={() => gridApiRef.current?.scrollBy(220)}
            >
              <ChevronRight size={12} strokeWidth={1.7} />
            </button>
            <button
              className="grid h-6 w-6 place-items-center rounded-[5px] text-slate-500 hover:bg-slate-900/[0.06] hover:text-slate-700"
              title="Last row"
              onClick={() => gridApiRef.current?.scrollToBottom()}
            >
              <ChevronsRight size={12} strokeWidth={1.7} />
            </button>
          </div>
        </div>
      </div>

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
              onShowMessages={() => setSubTab("messages")}
            />
          ) : (
            <ResultGrid
              result={result}
              matchIndices={matchIndices}
              onViewCell={(r, c) => setViewer({ row: r, col: c })}
              onApi={(api) => (gridApiRef.current = api)}
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
          rowNumber={viewer.row + 1}
          onClose={() => setViewer(null)}
        />
      )}
    </div>
  );
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

function ErrorPanel({
  error,
  onShowMessages,
}: {
  error: ApiError;
  onShowMessages: () => void;
}) {
  return (
    <div className="flex-1 overflow-auto bg-rose-50 p-4">
      <div className="flex items-start gap-2 rounded-md border border-rose-200 bg-white p-3">
        <AlertTriangle size={16} strokeWidth={1.7} className="mt-0.5 shrink-0 text-rose-500" />
        <div className="min-w-0">
          <div className="font-code text-[12.5px] text-rose-600">{error.code}</div>
          <div className="mt-0.5 text-[13px] font-medium text-slate-900">
            {error.message}
          </div>
          {error.detail && (
            <div className="mt-1 font-code text-[12.5px] whitespace-pre-wrap text-slate-600">
              {error.detail}
            </div>
          )}
          {error.hint && (
            <div className="mt-1 text-[13px] text-slate-500">
              Hint: {error.hint}
            </div>
          )}
          {error.db_code && (
            <div className="mt-1 font-code text-[12.5px] text-slate-400">
              code: {error.db_code}
            </div>
          )}
          <button
            className="mt-2 text-[13px] text-accent hover:underline"
            onClick={onShowMessages}
          >
            Open Messages tab
          </button>
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
    <div className="custom-scrollbar flex-1 overflow-auto bg-surface p-3 font-code text-[12.5px]">
      {messages.length === 0 && (
        <div className="text-slate-500">Nothing here yet.</div>
      )}
      {messages.map((m) => (
        <div
          key={m.id}
          className={`flex items-start gap-2 py-0.5 ${
            m.level === "error"
              ? "text-rose-600"
              : m.level === "warn"
                ? "text-amber-600"
                : "text-slate-600"
          }`}
        >
          <span className="w-[52px] shrink-0 text-[11px] text-slate-400">
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
