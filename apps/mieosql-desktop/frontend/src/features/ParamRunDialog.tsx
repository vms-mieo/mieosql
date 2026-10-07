// Parameter query dialog: collects values for `?` / `$n` placeholders and
// hands them to `store.runWithParams`. Opened from the store's `paramDialog`
// state (see `requestRun`).

import { useMemo, useState } from "react";

import { useStore } from "../lib/store";
import {
  encodeTypedInput,
  findPlaceholders,
  type ParamType,
  type PlaceholderInfo,
} from "../lib/sql-params";
import { DialogShell } from "./DialogShell";

const TYPE_OPTIONS: Array<{ value: ParamType; label: string }> = [
  { value: "auto", label: "Auto" },
  { value: "text", label: "Text" },
  { value: "int", label: "Int" },
  { value: "decimal", label: "Decimal" },
  { value: "bool", label: "Bool" },
  { value: "null", label: "Null" },
];

/** Postgres `$n` can repeat; one input per unique parameter index. */
function uniquePlaceholders(list: PlaceholderInfo[]): PlaceholderInfo[] {
  const seen = new Set<number>();
  const out: PlaceholderInfo[] = [];
  for (const info of list) {
    if (seen.has(info.index)) continue;
    seen.add(info.index);
    out.push(info);
  }
  return out;
}

export function ParamRunDialog({ onClose }: { onClose: () => void }) {
  const paramDialog = useStore((s) => s.paramDialog);
  const tabs = useStore((s) => s.tabs);
  const sessions = useStore((s) => s.sessions);
  const runWithParams = useStore((s) => s.runWithParams);
  const closeParamDialog = useStore((s) => s.closeParamDialog);

  const tab = paramDialog
    ? (tabs.find((t) => t.id === paramDialog.tabId) ?? null)
    : null;
  const engine = tab
    ? (sessions.find((s) => s.id === tab.sessionId)?.engine ?? null)
    : null;

  const placeholders = useMemo(
    () => (paramDialog ? uniquePlaceholders(findPlaceholders(engine, paramDialog.sql)) : []),
    [engine, paramDialog],
  );

  const [inputs, setInputs] = useState<Record<number, string>>({});
  const [types, setTypes] = useState<Record<number, ParamType>>({});
  const [running, setRunning] = useState(false);

  const run = async () => {
    if (!paramDialog || running) return;
    setRunning(true);
    try {
      const values = placeholders.map((_, i) =>
        encodeTypedInput(inputs[i] ?? "", types[i] ?? "auto"),
      );
      await runWithParams(paramDialog.tabId, paramDialog.sql, values);
    } finally {
      setRunning(false);
      closeParamDialog();
      onClose();
    }
  };

  const label = (info: PlaceholderInfo) =>
    engine === "postgres" ? info.token : `? #${info.index}`;

  return (
    <DialogShell
      title="Query Parameters"
      width={560}
      onClose={() => {
        closeParamDialog();
        onClose();
      }}
      footer={
        <>
          <button
            className="h-7 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50"
            onClick={() => {
              closeParamDialog();
              onClose();
            }}
          >
            Cancel
          </button>
          <button
            className="h-7 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover disabled:opacity-50"
            disabled={!paramDialog || placeholders.length === 0 || running}
            onClick={() => void run()}
          >
            Run
          </button>
        </>
      }
    >
      {!paramDialog ? (
        <div className="p-6 text-[13px] text-slate-500">
          No parameterized query is waiting to run.
        </div>
      ) : (
        <div className="space-y-3 p-3">
          <div className="flex items-center justify-between text-[11px] text-slate-500">
            <span>
              {placeholders.length} parameter{placeholders.length === 1 ? "" : "s"}
              {engine ? ` · ${engine}` : ""}
            </span>
            <span className="text-slate-400">Enter runs the query</span>
          </div>

          <pre className="custom-scrollbar max-h-24 overflow-auto rounded-md border border-slate-200 bg-sidebar p-2 font-code text-[12.5px] leading-snug whitespace-pre-wrap text-slate-700">
            {paramDialog.sql}
          </pre>

          <div className="space-y-1.5">
            {placeholders.map((info, i) => (
              <div key={`${info.index}-${info.position}`} className="flex items-center gap-2">
                <span className="w-14 shrink-0 rounded bg-slate-900/[0.06] px-1.5 py-0.5 text-center font-code text-[11px] font-medium text-slate-600">
                  {label(info)}
                </span>
                <select
                  className="h-7 w-[84px] shrink-0 rounded-md border border-slate-300 bg-white px-1.5 text-[12.5px] text-slate-700 focus:ring-2 focus:ring-accent/40 focus:outline-none"
                  value={types[i] ?? "auto"}
                  onChange={(e) => setTypes((m) => ({ ...m, [i]: e.target.value as ParamType }))}
                >
                  {TYPE_OPTIONS.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {opt.label}
                    </option>
                  ))}
                </select>
                <input
                  className="h-7 min-w-0 flex-1 rounded-md border border-slate-300 bg-white px-2 font-code text-[12.5px] text-slate-800 focus:ring-2 focus:ring-accent/40 focus:outline-none"
                  placeholder={
                    (types[i] ?? "auto") === "null" ? "(NULL)" : `Value for ${label(info)}`
                  }
                  disabled={(types[i] ?? "auto") === "null"}
                  value={inputs[i] ?? ""}
                  onChange={(e) => setInputs((m) => ({ ...m, [i]: e.target.value }))}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      void run();
                    }
                  }}
                />
              </div>
            ))}
          </div>

          <p className="text-[11px] text-slate-400">
            Auto infers numbers, booleans and NULL; pick an explicit type to force one.
          </p>
        </div>
      )}
    </DialogShell>
  );
}
