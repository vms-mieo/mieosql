import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";

import { useStore } from "../lib/store";

export function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismissToast);

  if (toasts.length === 0) return null;

  return (
    <div className="pointer-events-none fixed right-4 bottom-8 z-[100] flex w-[340px] flex-col gap-2">
      {toasts.map((t) => (
        <div
          key={t.id}
          className="pointer-events-auto flex items-start gap-2.5 rounded-lg border border-slate-200 bg-white px-3 py-2.5 shadow-[0_8px_30px_rgba(0,0,0,0.14)]"
        >
          {t.level === "error" ? (
            <AlertCircle size={14} className="mt-0.5 shrink-0 text-rose-500" />
          ) : t.level === "success" ? (
            <CheckCircle2
              size={14}
              className="mt-0.5 shrink-0 text-emerald-500"
            />
          ) : (
            <Info size={14} className="mt-0.5 shrink-0 text-slate-400" />
          )}
          <span className="flex-1 text-[13px] leading-snug break-words whitespace-pre-wrap text-slate-800">
            {t.text}
          </span>
          <button
            className="grid h-5 w-5 shrink-0 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
            onClick={() => dismiss(t.id)}
          >
            <X size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}
