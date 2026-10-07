// Shared dialog chrome for feature-pack dialogs.
// Agents implement the dialog bodies; keep this shell stable.

import { X } from "lucide-react";

export function DialogShell({
  title,
  width = 620,
  onClose,
  children,
  footer,
}: {
  title: string;
  width?: number;
  onClose: () => void;
  children: React.ReactNode;
  footer?: React.ReactNode;
}) {
  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-slate-900/30 p-4"
      onMouseDown={onClose}
    >
      <div
        className="flex max-h-[88vh] w-full flex-col overflow-hidden rounded-xl border border-slate-300/70 bg-white shadow-[0_24px_70px_rgba(0,0,0,0.25)]"
        style={{ maxWidth: width }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-11 shrink-0 items-center justify-between border-b border-slate-200/80 px-4">
          <span className="text-[15px] font-semibold text-slate-900">
            {title}
          </span>
          <button
            className="grid h-7 w-7 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
            onClick={onClose}
            title="Close"
          >
            <X size={14} />
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
