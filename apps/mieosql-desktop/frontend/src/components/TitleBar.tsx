import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";

const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function TitleBar() {
  const onMinimize = () => {
    if (isTauri) void getCurrentWindow().minimize();
  };
  const onToggleMaximize = () => {
    if (isTauri) void getCurrentWindow().toggleMaximize();
  };
  const onClose = () => {
    if (isTauri) void getCurrentWindow().close();
  };

  return (
    <header
      className="flex h-[30px] shrink-0 select-none items-center justify-between bg-[#2d3748] pr-2 pl-3 text-slate-200"
      data-tauri-drag-region
    >
      <div className="flex items-center gap-2" data-tauri-drag-region>
        <div
          className="grid h-4 w-4 place-items-center rounded-full bg-amber-500 text-[10px] font-black text-white"
          data-tauri-drag-region
        >
          M
        </div>
        <span
          className="text-[11px] font-medium tracking-wide"
          data-tauri-drag-region
        >
          MieoSQL
        </span>
        <span className="text-[10px] text-slate-400" data-tauri-drag-region>
          multi-engine database client
        </span>
      </div>
      <div className="flex items-center gap-1 text-slate-400">
        <button
          className="rounded px-2 py-0.5 transition hover:bg-slate-700 hover:text-white"
          onClick={onMinimize}
          title="Minimize"
        >
          <Minus size={11} />
        </button>
        <button
          className="rounded px-2 py-0.5 transition hover:bg-slate-700 hover:text-white"
          onClick={onToggleMaximize}
          title="Maximize"
        >
          <Square size={10} />
        </button>
        <button
          className="rounded px-2 py-0.5 transition hover:bg-red-600 hover:text-white"
          onClick={onClose}
          title="Close"
        >
          <X size={13} />
        </button>
      </div>
    </header>
  );
}
