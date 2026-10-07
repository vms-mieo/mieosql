import { useEffect, useRef, useState } from "react";

export type MenuItem =
  | { kind: "separator" }
  | { kind: "label"; label: string }
  | {
      kind: "item";
      label: string;
      shortcut?: string;
      disabled?: boolean;
      danger?: boolean;
      onClick: () => void;
    };

/**
 * Apple-style popover menu: white surface, hairline border, soft shadow and
 * an accent-blue highlight on the hovered row (like macOS menu bar menus).
 */
export function PopupMenu({
  items,
  align = "left",
  width,
  trigger,
}: {
  items: MenuItem[];
  align?: "left" | "right";
  width?: number;
  trigger: (state: { open: boolean; toggle: () => void }) => React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDocClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDocClick);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative">
      {trigger({ open, toggle: () => setOpen((v) => !v) })}
      {open && (
        <div
          role="menu"
          className={`absolute z-50 mt-1 min-w-[220px] overflow-hidden rounded-md border border-slate-300/70 bg-white py-1 shadow-[0_10px_34px_rgba(0,0,0,0.16)] ${
            align === "right" ? "right-0" : "left-0"
          }`}
          style={width ? { width } : undefined}
        >
          {items.map((item, i) =>
            item.kind === "separator" ? (
              <div key={i} className="my-1 h-px bg-slate-200" />
            ) : item.kind === "label" ? (
              <div
                key={i}
                className="px-3 pt-1.5 pb-0.5 text-[10.5px] font-semibold tracking-[0.06em] text-slate-400 uppercase"
              >
                {item.label}
              </div>
            ) : (
              <button
                key={i}
                role="menuitem"
                disabled={item.disabled}
                className={`group flex w-full items-center justify-between gap-8 px-3 py-[5px] text-left text-[13px] ${
                  item.disabled
                    ? "cursor-default text-slate-300"
                    : item.danger
                      ? "text-rose-600 hover:bg-rose-500 hover:text-white"
                      : "text-slate-800 hover:bg-accent hover:text-white"
                }`}
                onClick={() => {
                  setOpen(false);
                  item.onClick();
                }}
              >
                <span className="truncate">{item.label}</span>
                {item.shortcut && (
                  <span
                    className={`shrink-0 font-code text-[11px] ${
                      item.disabled
                        ? "text-slate-300"
                        : "text-slate-400 group-hover:text-white/75"
                    }`}
                  >
                    {item.shortcut}
                  </span>
                )}
              </button>
            ),
          )}
        </div>
      )}
    </div>
  );
}
