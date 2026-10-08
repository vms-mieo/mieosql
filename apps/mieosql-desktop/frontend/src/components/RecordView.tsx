import { useState } from "react";
import {
  Check,
  ChevronLeft,
  ChevronRight,
  Copy,
  Maximize2,
} from "lucide-react";

import type { ColumnMeta, Value } from "../lib/bindings";
import { copyText, formatCell, kindBadge, plainText } from "../lib/format";
import { useStore } from "../lib/store";

const NULL_VALUE: Value = { t: "null" };

type Props = {
  columns: ColumnMeta[];
  rows: Value[][];
  rowIndex: number;
  onRowChange: (i: number) => void;
  onViewCell?: (row: number, col: number) => void;
};

function prettyJson(value: Value): string {
  if (value.t === "json") {
    try {
      return JSON.stringify(value.v, null, 2);
    } catch {
      return plainText(value);
    }
  }
  return plainText(value);
}

function clampRow(i: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(Math.max(0, i), total - 1);
}

export function RecordView({ columns, rows, rowIndex, onRowChange, onViewCell }: Props) {
  const toast = useStore((s) => s.toast);
  const total = rows.length;
  const safe = clampRow(rowIndex, total);
  const [jump, setJump] = useState<string | null>(null);
  const [copied, setCopied] = useState<number | null>(null);

  const commitJump = () => {
    if (jump === null) return;
    const n = Number.parseInt(jump, 10);
    if (Number.isFinite(n)) onRowChange(clampRow(n - 1, total));
    setJump(null);
  };

  const copyField = (ci: number, value: Value) => {
    const text = value.t === "json" ? prettyJson(value) : plainText(value);
    void copyText(text).then((ok) => {
      if (ok) {
        setCopied(ci);
        window.setTimeout(() => setCopied((c) => (c === ci ? null : c)), 1200);
      } else {
        toast("error", "Copy failed");
      }
    });
  };

  if (total === 0) {
    return (
      <div className="grid flex-1 place-items-center bg-surface text-[13px] text-slate-500">
        Query returned no rows
      </div>
    );
  }

  const row = rows[safe] ?? [];

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-surface">
      {/* row navigator */}
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-slate-200/70 bg-sidebar px-3 text-[12.5px] select-none">
        <button
          className="grid h-7 w-7 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700 disabled:opacity-40"
          title="Previous row"
          disabled={safe <= 0}
          onClick={() => onRowChange(safe - 1)}
        >
          <ChevronLeft size={14} strokeWidth={1.8} />
        </button>
        <button
          className="grid h-7 w-7 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700 disabled:opacity-40"
          title="Next row"
          disabled={safe >= total - 1}
          onClick={() => onRowChange(safe + 1)}
        >
          <ChevronRight size={14} strokeWidth={1.8} />
        </button>
        <span className="ml-1 text-slate-500">Row</span>
        <input
          className="h-6 w-14 rounded-md border border-slate-200 bg-white px-1.5 text-center text-[12.5px] text-slate-800 focus:border-accent focus:outline-none"
          value={jump ?? String(safe + 1)}
          inputMode="numeric"
          onChange={(e) => setJump(e.target.value.replace(/[^0-9]/g, ""))}
          onKeyDown={(e) => {
            if (e.key === "Enter") commitJump();
            else if (e.key === "Escape") setJump(null);
          }}
          onBlur={commitJump}
          aria-label="Go to row"
        />
        <span className="text-slate-500">/ {total.toLocaleString()}</span>
        <span className="ml-auto text-[11.5px] text-slate-400">
          {columns.length} column{columns.length === 1 ? "" : "s"}
        </span>
      </div>

      {/* vertical form */}
      <div className="custom-scrollbar min-h-0 flex-1 overflow-auto">
        <div className="mx-auto flex max-w-[760px] flex-col gap-2 px-4 py-3">
          {columns.map((col, ci) => {
            const value = row[ci] ?? NULL_VALUE;
            const view = formatCell(value);
            const isNull = value.t === "null";
            const isJson = value.t === "json";
            const text = value.t === "text" ? value.v : null;
            const longText =
              text !== null && (text.length > 200 || text.includes("\n"));
            const lines = text !== null ? text.split("\n").length : 0;
            return (
              <div
                key={`${col.ordinal}-${col.name}`}
                className="rounded-lg border border-slate-200/80 bg-white px-3 py-2"
              >
                <div className="flex items-center gap-2">
                  <span className="truncate text-[12.5px] font-medium text-slate-700">
                    {col.name}
                  </span>
                  <span className="shrink-0 rounded bg-slate-900/[0.06] px-1 text-[10.5px] font-semibold text-slate-600">
                    {kindBadge(col.kind)}
                  </span>
                  <span className="truncate text-[11px] text-slate-400">
                    {col.native_type}
                  </span>
                  <span className="ml-auto flex shrink-0 items-center gap-0.5">
                    {onViewCell && (
                      <button
                        className="grid h-6 w-6 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.05] hover:text-slate-600"
                        title="Open full value"
                        onClick={() => onViewCell(safe, ci)}
                      >
                        <Maximize2 size={12} strokeWidth={1.8} />
                      </button>
                    )}
                    <button
                      className="grid h-6 w-6 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.05] hover:text-slate-600"
                      title={`Copy ${col.name}`}
                      onClick={() => copyField(ci, value)}
                    >
                      {copied === ci ? (
                        <Check size={12} strokeWidth={2} className="text-emerald-500" />
                      ) : (
                        <Copy size={12} strokeWidth={1.8} />
                      )}
                    </button>
                  </span>
                </div>
                <div className="mt-1">
                  {isNull ? (
                    <span className="text-[12.5px] text-slate-400 italic">
                      {view.text}
                    </span>
                  ) : isJson ? (
                    <pre className="custom-scrollbar max-h-56 overflow-auto rounded-md bg-slate-900/[0.03] p-2 font-code text-[12.5px] whitespace-pre-wrap text-slate-700">
                      {prettyJson(value)}
                    </pre>
                  ) : longText && text !== null ? (
                    <textarea
                      readOnly
                      className="custom-scrollbar block max-h-56 w-full resize-y rounded-md bg-slate-900/[0.03] p-2 font-code text-[12.5px] text-slate-800 focus:outline-none"
                      rows={Math.min(10, Math.max(3, lines))}
                      value={text}
                      spellCheck={false}
                    />
                  ) : (
                    <span
                      className={`block text-[12.5px] ${
                        view.tone === "number" || view.tone === "bool"
                          ? "font-code text-slate-800"
                          : view.tone === "temporal"
                            ? "font-code text-slate-500"
                            : view.tone === "binary"
                              ? "font-code text-slate-600"
                              : view.tone === "unsupported"
                                ? "text-amber-600 italic"
                                : "text-slate-800"
                      } ${view.align === "right" ? "text-right" : ""} break-words whitespace-pre-wrap`}
                    >
                      {view.text}
                    </span>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
