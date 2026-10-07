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
    <div className="flex min-h-0 flex-1 flex-col bg-[#eef2f7]">
      {/* sub-tab bar + timing */}
      <div className="flex h-[28px] shrink-0 items-center justify-between bg-[#e9edf2] px-2 select-none">
        <div className="flex h-full items-center">
          {tab.results.map((r, i) => (
            <button
              key={r.queryId}
              className={`relative flex h-full items-center gap-1.5 px-3 text-[11px] ${
                i === tab.activeResult && subTab === "result"
                  ? "bg-white font-medium text-sky-700"
                  : "text-slate-500 hover:bg-white/60 hover:text-slate-700"
              }`}
              onClick={() => {
                store.setActiveResult(tab.id, i);
                setSubTab("result");
              }}
            >
              {i === tab.activeResult && subTab === "result" && (
                <span className="absolute top-0 right-0 left-0 h-[2px] bg-sky-500" />
              )}
              <Grid3x3 size={12} />
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
                    ? "text-sky-600"
                    : "text-slate-300 hover:text-slate-500"
                }`}
                onClick={(e) => {
                  e.stopPropagation();
                  store.togglePin(tab.id, i);
                }}
              >
                <Pin size={10} />
              </span>
            </button>
          ))}
          <button
            className={`flex h-full items-center gap-1.5 px-3 text-[11px] ${
              subTab === "messages"
                ? "bg-white font-medium text-sky-700"
                : "text-slate-500 hover:bg-white/60 hover:text-slate-700"
            }`}
            onClick={() => setSubTab("messages")}
          >
            <Terminal size={12} />
            Messages
            {messageCount > 0 && (
              <span
                className={`rounded-full px-1 text-[9px] ${
                  errorCount > 0
                    ? "bg-rose-100 text-rose-700"
                    : "bg-slate-200 text-slate-600"
                }`}
              >
                {messageCount}
              </span>
            )}
          </button>
          <button
            className={`flex h-full items-center gap-1.5 px-3 text-[11px] ${
              subTab === "status"
                ? "bg-white font-medium text-sky-700"
                : "text-slate-500 hover:bg-white/60 hover:text-slate-700"
            }`}
            onClick={() => setSubTab("status")}
          >
            <ScrollText size={12} />
            Status
          </button>
        </div>
        <div className="flex items-center gap-2 pr-1 text-[10px] text-slate-500">
          {result && (
            <>
              {result.status === "running" ? (
                <span className="flex items-center gap-1 text-sky-600">
                  <RotateCw size={11} className="animate-spin" />
                  running…
                </span>
              ) : (
                <span className="flex items-center gap-1">
                  <CheckCircle2
                    size={11}
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
                    <AlertTriangle size={11} /> limit reached
                  </span>
                </>
              )}
            </>
          )}
        </div>
      </div>

      {/* toolbar */}
      <div className="flex h-[30px] shrink-0 items-center justify-between bg-[#f1f5f9] px-2 text-[11px]">
        <div className="flex items-center gap-1.5">
          <button
            className="rounded p-1 text-slate-500 hover:bg-white hover:text-slate-700 disabled:opacity-40"
            title="Re-run"
            disabled={running || !tab.sessionId}
            onClick={rerun}
          >
            <RotateCw size={13} />
          </button>
          {running && (
            <button
              className="rounded p-1 text-rose-500 hover:bg-white"
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
              className="flex items-center gap-1 rounded px-2 py-0.5 text-slate-600 hover:bg-white disabled:opacity-40"
              disabled={!result || result.rows.length === 0}
              onClick={() => setExportOpen((v) => !v)}
            >
              <FileDown size={13} className="text-teal-600" />
              Export
            </button>
            {exportOpen && (
              <div className="absolute top-full left-0 z-40 mt-1 w-52 rounded border border-slate-300 bg-white py-1 shadow-lg">
                {EXPORT_FORMATS.map((format) => (
                  <button
                    key={format}
                    className="flex w-full items-center gap-2 px-3 py-1 text-left hover:bg-sky-50"
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
                  className="flex w-full items-center gap-2 px-3 py-1 text-left hover:bg-sky-50"
                  onClick={() => {
                    setExportOpen(false);
                    copyData("csv");
                  }}
                >
                  <Copy size={11} /> Copy as CSV
                </button>
                <button
                  className="flex w-full items-center gap-2 px-3 py-1 text-left hover:bg-sky-50"
                  onClick={() => {
                    setExportOpen(false);
                    copyData("json");
                  }}
                >
                  <Copy size={11} /> Copy as JSON lines
                </button>
              </div>
            )}
          </div>
          <div className="ml-1 flex items-center rounded bg-white px-2 py-0.5 ring-1 ring-slate-200">
            <Filter size={11} className="mr-1 text-slate-400" />
            <input
              className="w-44 border-none bg-transparent p-0 text-[11px] placeholder:text-slate-400 focus:outline-none"
              placeholder="Filter loaded rows…"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          </div>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-[10px] text-slate-500">
            {matchIndices
              ? `${matchIndices.length.toLocaleString()} / ${(result?.rows.length ?? 0).toLocaleString()} rows`
              : `${(result?.rows.length ?? 0).toLocaleString()} rows`}
          </span>
          <div className="flex items-center gap-0.5 rounded bg-white p-0.5 ring-1 ring-slate-200">
            <button
              className="rounded p-0.5 text-slate-400 hover:bg-slate-100 hover:text-slate-600"
              title="First row"
              onClick={() => gridApiRef.current?.scrollToTop()}
            >
              <ChevronsLeft size={13} />
            </button>
            <button
              className="rounded p-0.5 text-slate-400 hover:bg-slate-100 hover:text-slate-600"
              title="Scroll up"
              onClick={() => gridApiRef.current?.scrollBy(-220)}
            >
              <ChevronLeft size={13} />
            </button>
            <button
              className="rounded p-0.5 text-slate-400 hover:bg-slate-100 hover:text-slate-600"
              title="Scroll down"
              onClick={() => gridApiRef.current?.scrollBy(220)}
            >
              <ChevronRight size={13} />
            </button>
            <button
              className="rounded p-0.5 text-slate-400 hover:bg-slate-100 hover:text-slate-600"
              title="Last row"
              onClick={() => gridApiRef.current?.scrollToBottom()}
            >
              <ChevronsRight size={13} />
            </button>
          </div>
        </div>
      </div>

      {/* content */}
      {subTab === "result" && (
        <>
          {!result ? (
            <div className="grid flex-1 place-items-center bg-white text-slate-400">
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
      return <FileSpreadsheet size={11} className="text-emerald-600" />;
    case "tsv":
      return <FileText size={11} className="text-teal-600" />;
    case "json":
    case "json_lines":
      return <FileJson size={11} className="text-violet-600" />;
    case "sql":
      return <Database size={11} className="text-sky-600" />;
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
    <div className="flex-1 overflow-auto bg-[#fff7f7] p-4">
      <div className="flex items-start gap-2 rounded border border-rose-200 bg-white p-3">
        <AlertTriangle size={16} className="mt-0.5 shrink-0 text-rose-500" />
        <div className="min-w-0">
          <div className="font-code text-[11px] text-rose-600">{error.code}</div>
          <div className="mt-0.5 text-[12px] font-medium text-slate-800">
            {error.message}
          </div>
          {error.detail && (
            <div className="mt-1 font-code text-[11px] whitespace-pre-wrap text-slate-600">
              {error.detail}
            </div>
          )}
          {error.hint && (
            <div className="mt-1 text-[11px] text-slate-500">
              Hint: {error.hint}
            </div>
          )}
          {error.db_code && (
            <div className="mt-1 font-code text-[10px] text-slate-400">
              code: {error.db_code}
            </div>
          )}
          <button
            className="mt-2 text-[11px] text-sky-600 hover:underline"
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
    <div className="custom-scrollbar flex-1 overflow-auto bg-white p-3 font-code text-[11px]">
      {messages.length === 0 && (
        <div className="text-slate-400">Nothing here yet.</div>
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
          <span className="w-[52px] shrink-0 text-[10px] text-slate-400">
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
    <div className="custom-scrollbar flex-1 overflow-auto bg-white p-4">
      <div className="grid grid-cols-[180px_1fr] gap-x-4 gap-y-1.5 text-[11px]">
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
