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
      className="fixed inset-0 z-[90] grid place-items-center bg-black/40"
      onMouseDown={onClose}
    >
      <div
        className="flex max-h-[70vh] w-[640px] max-w-[90vw] flex-col rounded-xl border border-slate-300/70 bg-white shadow-[0_24px_70px_rgba(0,0,0,0.25)]"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-10 shrink-0 items-center justify-between border-b border-slate-200/80 px-3.5">
          <div className="flex items-center gap-2 text-[12px]">
            <span className="text-[13px] font-semibold text-slate-900">
              {column?.name ?? "value"}
            </span>
            <span className="rounded bg-slate-900/[0.06] px-1 text-[10.5px] font-semibold text-slate-600">
              {column ? kindBadge(column.kind) : "?"}
            </span>
            <span className="text-[11px] text-slate-500">
              row {rowNumber}
              {column ? ` · ${column.native_type}` : ""}
            </span>
          </div>
          <button
            className="grid h-6 w-6 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
            onClick={onClose}
          >
            <X size={14} />
          </button>
        </div>
        <pre className="custom-scrollbar min-h-[80px] flex-1 overflow-auto bg-white p-3.5 font-code text-[12.5px] whitespace-pre-wrap text-slate-800">
          {body}
        </pre>
        <div className="flex shrink-0 items-center justify-end gap-2 border-t border-slate-200/80 bg-sidebar px-3.5 py-2.5">
          {value.t === "bytes" && (
            <span className="mr-auto text-[11px] text-slate-500">
              binary shown as base64
            </span>
          )}
          <button
            className="h-7 rounded-md bg-accent px-3 text-[13px] font-medium text-white hover:bg-accent-hover"
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
            className="h-7 rounded-md border border-slate-300 bg-white px-3 text-[13px] text-slate-700 hover:bg-slate-50"
            onClick={onClose}
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
