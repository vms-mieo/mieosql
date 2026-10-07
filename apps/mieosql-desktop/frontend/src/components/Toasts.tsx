import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";

import { useStore } from "../lib/store";

export function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismissToast);

  if (toasts.length === 0) return null;

  return (
    <div className="pointer-events-none fixed right-3 bottom-8 z-[100] flex w-[340px] flex-col gap-2">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={`pointer-events-auto flex items-start gap-2 rounded border px-3 py-2 shadow-lg backdrop-blur ${
            t.level === "error"
              ? "border-rose-300 bg-rose-50/95 text-rose-800"
              : t.level === "success"
                ? "border-emerald-300 bg-emerald-50/95 text-emerald-800"
                : "border-slate-300 bg-white/95 text-slate-700"
          }`}
        >
          {t.level === "error" ? (
            <AlertCircle size={14} className="mt-0.5 shrink-0" />
          ) : t.level === "success" ? (
            <CheckCircle2 size={14} className="mt-0.5 shrink-0" />
          ) : (
            <Info size={14} className="mt-0.5 shrink-0" />
          )}
          <span className="flex-1 text-[11px] leading-snug break-words whitespace-pre-wrap">
            {t.text}
          </span>
          <button
            className="shrink-0 rounded p-0.5 opacity-60 hover:bg-black/5 hover:opacity-100"
            onClick={() => dismiss(t.id)}
          >
            <X size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}
