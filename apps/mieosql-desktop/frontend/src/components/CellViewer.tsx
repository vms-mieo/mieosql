import { X } from "lucide-react";

import type { ColumnMeta, Value } from "../lib/bindings";
import { copyText, formatCell, kindBadge } from "../lib/format";
import { useStore } from "../lib/store";

export function CellViewer({
  column,
  value,
  rowNumber,
  onClose,
}: {
  column: ColumnMeta | undefined;
  value: Value;
  rowNumber: number;
  onClose: () => void;
}) {
  const toast = useStore((s) => s.toast);
  const view = formatCell(value);

  let body = view.title;
  if (value.t === "json") {
    try {
      body = JSON.stringify(value.v, null, 2);
    } catch {
      body = view.title;
    }
  } else if (value.t === "null") {
    body = "NULL";
  }

  return (
    <div
      className="fixed inset-0 z-[90] grid place-items-center bg-slate-900/30"
      onMouseDown={onClose}
    >
      <div
        className="flex max-h-[70vh] w-[640px] max-w-[90vw] flex-col rounded border border-slate-300 bg-white shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-9 shrink-0 items-center justify-between border-b border-slate-200 bg-[#f8fafc] px-3">
          <div className="flex items-center gap-2 text-[12px]">
            <span className="font-semibold text-slate-700">
              {column?.name ?? "value"}
            </span>
            <span className="rounded bg-slate-200 px-1 text-[10px] text-slate-500">
              {column ? kindBadge(column.kind) : "?"}
            </span>
            <span className="text-[10px] text-slate-400">
              row {rowNumber}
              {column ? ` · ${column.native_type}` : ""}
            </span>
          </div>
          <button
            className="rounded p-1 text-slate-400 hover:bg-slate-200 hover:text-slate-700"
            onClick={onClose}
          >
            <X size={14} />
          </button>
        </div>
        <pre className="custom-scrollbar min-h-[80px] flex-1 overflow-auto bg-white p-3 font-code text-[11px] whitespace-pre-wrap text-slate-800">
          {body}
        </pre>
        <div className="flex shrink-0 items-center justify-end gap-2 border-t border-slate-200 bg-[#f8fafc] px-3 py-2">
          {value.t === "bytes" && (
            <span className="mr-auto text-[10px] text-slate-400">
              binary shown as base64
            </span>
          )}
          <button
            className="rounded bg-sky-600 px-3 py-1 text-[11px] font-medium text-white hover:bg-sky-700"
            onClick={() => {
              const text =
                value.t === "json" ? body : value.t === "null" ? "" : view.title;
              void copyText(text).then((ok) =>
                toast(ok ? "success" : "error", ok ? "Copied" : "Copy failed"),
              );
            }}
          >
            Copy
          </button>
          <button
            className="rounded border border-slate-300 px-3 py-1 text-[11px] text-slate-600 hover:bg-slate-100"
            onClick={onClose}
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
