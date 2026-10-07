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
        className="flex max-h-[88vh] w-full flex-col overflow-hidden rounded-lg border border-slate-300 bg-white shadow-xl"
        style={{ maxWidth: width }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-9 shrink-0 items-center justify-between border-b border-slate-200 bg-[#f1f5f9] px-3">
          <span className="text-[12px] font-semibold text-slate-700">
            {title}
          </span>
          <button
            className="rounded p-1 text-slate-400 hover:bg-slate-200 hover:text-slate-700"
            onClick={onClose}
            title="Close"
          >
            <X size={13} />
          </button>
        </div>
        <div className="custom-scrollbar min-h-0 flex-1 overflow-auto">
          {children}
        </div>
        {footer && (
          <div className="flex h-11 shrink-0 items-center justify-end gap-2 border-t border-slate-200 bg-[#f8fafc] px-3">
            {footer}
          </div>
        )}
      </div>
    </div>
  );
}
