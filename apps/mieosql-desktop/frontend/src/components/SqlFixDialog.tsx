// "Fix SQL Syntax" dialog — shows the applied heuristic fixes and a
// before/after preview; the user explicitly applies the repaired text.

import { CheckCircle2, Wand2 } from "lucide-react";

import type { SqlFix } from "../lib/sql-fix";
import { DLG_BTN, DLG_BTN_PRIMARY, ObjectDialogShell } from "./dialogs/DialogChrome";

export function SqlFixDialog({
  original,
  fixed,
  fixes,
  notes,
  onApply,
  onClose,
}: {
  original: string;
  fixed: string;
  fixes: SqlFix[];
  notes: string[];
  onApply: () => void;
  onClose: () => void;
}) {
  const pre =
    "custom-scrollbar max-h-[300px] overflow-auto rounded-md border border-slate-200 bg-slate-50 p-2.5 font-code text-[12px] leading-relaxed whitespace-pre-wrap text-slate-700";

  return (
    <ObjectDialogShell
      title="Fix SQL Syntax"
      width={880}
      onClose={onClose}
      footer={
        <>
          <button className={DLG_BTN} onClick={onClose}>
            Cancel
          </button>
          <button className={DLG_BTN_PRIMARY} onClick={onApply}>
            <Wand2 size={13} strokeWidth={1.8} />
            Apply to editor
          </button>
        </>
      }
    >
      <div className="space-y-3.5 p-4">
        <div>
          <div className="mb-1.5 text-[12px] font-semibold tracking-[0.04em] text-slate-600 uppercase">
            Changes ({fixes.length})
          </div>
          <ul className="space-y-1">
            {fixes.map((f, i) => (
              <li key={i} className="flex items-start gap-1.5 text-[12.5px] text-slate-700">
                <CheckCircle2
                  size={13}
                  strokeWidth={1.8}
                  className="mt-0.5 shrink-0 text-emerald-500"
                />
                <span>{f.message}</span>
              </li>
            ))}
          </ul>
        </div>

        {notes.length > 0 && (
          <div className="rounded-md border border-amber-200 bg-amber-50 p-2.5 text-[12.5px] text-amber-700">
            {notes.map((n, i) => (
              <div key={i} className="flex items-start gap-1.5">
                <span className="mt-[3px] inline-block h-1.5 w-1.5 shrink-0 rounded-full bg-amber-500" />
                <span>{n}</span>
              </div>
            ))}
          </div>
        )}

        <div className="grid grid-cols-2 gap-3">
          <div>
            <div className="mb-1 text-[12px] font-semibold text-slate-600">Before</div>
            <pre className={pre}>{original}</pre>
          </div>
          <div>
            <div className="mb-1 text-[12px] font-semibold text-slate-600">After</div>
            <pre className={pre}>{fixed}</pre>
          </div>
        </div>
      </div>
    </ObjectDialogShell>
  );
}
