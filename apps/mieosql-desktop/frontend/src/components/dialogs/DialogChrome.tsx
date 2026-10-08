// Shared chrome + style tokens for the object-management dialogs
// (new database/table, import, backup, restore). Matches the feature-pack
// DialogShell conventions (backdrop, rounded-xl panel, header/footer) with the
// compact 13px type scale of the object dialogs.

import { useEffect, type ReactNode } from "react";
import { AlertTriangle, X } from "lucide-react";

export const DLG_INPUT =
  "h-8 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-800 placeholder:text-slate-400 focus:ring-2 focus:ring-accent/40 focus:outline-none disabled:bg-slate-50 disabled:text-slate-400";
export const DLG_LABEL = "mb-1 block text-[12px] font-medium text-slate-600";
export const DLG_BTN =
  "flex h-8 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-3 text-[13px] text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50";
export const DLG_BTN_PRIMARY =
  "flex h-8 items-center gap-1.5 rounded-md bg-accent px-3 text-[13px] font-medium text-white hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50";
export const DLG_PRE =
  "custom-scrollbar max-h-[220px] overflow-auto rounded-md border border-slate-200 bg-slate-50 p-2.5 font-code text-[12px] leading-relaxed whitespace-pre text-slate-700";

export function ObjectDialogShell({
  title,
  width = 560,
  busy = false,
  onClose,
  children,
  footer,
}: {
  title: string;
  /** Panel max-width in px. */
  width?: number;
  /** While busy the dialog cannot be dismissed (Escape/backdrop/X are blocked). */
  busy?: boolean;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}) {
  // Escape closes whenever the dialog is not running.
  useEffect(() => {
    if (busy) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  return (
    <div
      className="fixed inset-0 z-[70] grid place-items-center bg-black/30 p-4"
      onMouseDown={() => {
        if (!busy) onClose();
      }}
    >
      <div
        className="flex max-h-[88vh] w-full flex-col overflow-hidden rounded-xl border border-slate-300/70 bg-white shadow-[0_24px_70px_rgba(0,0,0,0.25)]"
        style={{ maxWidth: width }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-11 shrink-0 items-center justify-between border-b border-slate-200/80 px-4">
          <span className="text-[13px] font-semibold text-slate-900">{title}</span>
          <button
            className="grid h-7 w-7 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600 disabled:opacity-40"
            onClick={onClose}
            disabled={busy}
            title="Close (Esc)"
          >
            <X size={14} strokeWidth={1.7} />
          </button>
        </div>
        <div className="custom-scrollbar min-h-0 flex-1 overflow-auto">
          {children}
        </div>
        {footer && (
          <div className="flex h-12 shrink-0 items-center justify-end gap-2 border-t border-slate-200/80 bg-sidebar px-4">
            {footer}
          </div>
        )}
      </div>
    </div>
  );
}

/** Red inline error banner (API failures, validation summaries). */
export function DialogError({ text }: { text: string }) {
  return (
    <div className="flex items-start gap-1.5 rounded-md border border-rose-200 bg-rose-50 p-2.5 text-[12.5px] whitespace-pre-wrap text-rose-600">
      <AlertTriangle size={13} strokeWidth={1.8} className="mt-0.5 shrink-0" />
      <span className="min-w-0 break-words">{text}</span>
    </div>
  );
}

/** Amber informational banner (unsupported engine, ignored flags, warnings). */
export function DialogNotice({ children }: { children: ReactNode }) {
  return (
    <div className="flex items-start gap-1.5 rounded-md border border-amber-200 bg-amber-50 p-2.5 text-[12.5px] text-amber-700">
      <AlertTriangle size={13} strokeWidth={1.8} className="mt-0.5 shrink-0" />
      <div className="min-w-0">{children}</div>
    </div>
  );
}
