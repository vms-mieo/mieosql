import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";
import { AppLogo } from "./AppLogo";

const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function WindowButton({
  title,
  danger,
  onClick,
  children,
}: {
  title: string;
  danger?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      className={`grid h-[22px] w-8 place-items-center rounded-md text-slate-500 transition-colors ${
        danger
          ? "hover:bg-rose-500 hover:text-white"
          : "hover:bg-slate-900/[0.06] hover:text-slate-700"
      }`}
      onClick={onClick}
      title={title}
    >
      {children}
    </button>
  );
}

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
      className="flex h-7 shrink-0 items-center justify-between border-b border-slate-300/70 bg-window pr-1.5 pl-3 select-none"
      data-tauri-drag-region
      onDoubleClick={onToggleMaximize}
    >
      <div className="flex min-w-0 items-center gap-2" data-tauri-drag-region>
        <AppLogo size={16} radius={5} />
        <span
          className="truncate text-[13px] font-medium text-slate-900"
          data-tauri-drag-region
        >
          MieoSQL
        </span>
      </div>
      <div className="flex items-center">
        <WindowButton title="Minimize" onClick={onMinimize}>
          <Minus size={12} />
        </WindowButton>
        <WindowButton title="Maximize" onClick={onToggleMaximize}>
          <Square size={9.5} />
        </WindowButton>
        <WindowButton title="Close" danger onClick={onClose}>
          <X size={12} />
        </WindowButton>
      </div>
    </header>
  );
}
