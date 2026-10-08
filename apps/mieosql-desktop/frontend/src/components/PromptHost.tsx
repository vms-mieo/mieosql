import { useEffect, useState } from "react";
import { X } from "lucide-react";

import { resolvePrompt, usePromptRequest } from "../lib/dialog";

/**
 * Host for `promptDialog()` — a small in-app replacement for
 * `window.prompt`, which is not implemented in the Tauri webview.
 * Mounted once in `App`.
 */
export function PromptHost() {
  const req = usePromptRequest();
  const [value, setValue] = useState("");

  useEffect(() => {
    if (req) setValue(req.defaultValue);
  }, [req]);

  if (!req) return null;

  const submit = () => resolvePrompt(value);
  const cancel = () => resolvePrompt(null);

  return (
    <div
      className="fixed inset-0 z-[90] grid place-items-center bg-black/40"
      onMouseDown={cancel}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={req.title}
        className="w-[400px] max-w-[92vw] overflow-hidden rounded-xl border border-slate-300/70 bg-white shadow-[0_24px_70px_rgba(0,0,0,0.25)]"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit();
          else if (e.key === "Escape") cancel();
        }}
      >
        <div className="flex h-11 shrink-0 items-center justify-between border-b border-slate-200/80 px-4">
          <span className="text-[15px] font-semibold text-slate-900">
            {req.title}
          </span>
          <button
            className="grid h-7 w-7 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
            title="Cancel"
            onClick={cancel}
          >
            <X size={14} strokeWidth={1.7} />
          </button>
        </div>
        <div className="space-y-3 p-4">
          <p className="text-[13px] leading-relaxed whitespace-pre-wrap text-slate-700">
            {req.message}
          </p>
          <input
            autoFocus
            className="h-7 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] placeholder:text-slate-400 focus:ring-2 focus:ring-accent/40 focus:outline-none"
            type={req.password ? "password" : "text"}
            value={value}
            placeholder={req.placeholder}
            onChange={(e) => setValue(e.target.value)}
            onFocus={(e) => e.target.select()}
          />
        </div>
        <div className="flex h-12 shrink-0 items-center justify-end gap-2 border-t border-slate-200/80 bg-sidebar px-4">
          <button
            className="flex h-7 items-center rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50"
            onClick={cancel}
          >
            Cancel
          </button>
          <button
            className="flex h-7 items-center rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover"
            onClick={submit}
          >
            {req.okLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
