import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

export type ContextMenuItem =
  | { kind: "separator" }
  | { kind: "label"; label: string }
  | {
      kind: "item";
      label: string;
      icon?: ReactNode;
      shortcut?: string;
      danger?: boolean;
      disabled?: boolean;
      onClick: () => void;
    };

const VIEWPORT_PADDING = 4;

/**
 * A menu rendered at fixed screen coordinates (right-click / context menus),
 * styled like PopupMenu but positioned at the cursor instead of a trigger.
 * Closes on outside mousedown, Escape, resize, scroll and window blur.
 */
export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });

  // Clamp inside the viewport once the menu has been measured.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    setPos({
      x: Math.max(
        VIEWPORT_PADDING,
        Math.min(x, window.innerWidth - rect.width - VIEWPORT_PADDING),
      ),
      y: Math.max(
        VIEWPORT_PADDING,
        Math.min(y, window.innerHeight - rect.height - VIEWPORT_PADDING),
      ),
    });
  }, [x, y, items]);

  useEffect(() => {
    const onMouseDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const close = () => onClose();
    document.addEventListener("mousedown", onMouseDown, true);
    document.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    // Capture phase: closes the menu wherever the scroll happens.
    window.addEventListener("scroll", close, true);
    return () => {
      document.removeEventListener("mousedown", onMouseDown, true);
      document.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      role="menu"
      className="fixed z-50 min-w-[190px] overflow-hidden rounded-md border border-slate-300/70 bg-white py-1 shadow-[0_10px_34px_rgba(0,0,0,0.16)]"
      style={{ left: pos.x, top: pos.y }}
      onContextMenu={(e) => e.preventDefault()}
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
            className={`group flex w-full items-center gap-2 px-3 py-[5px] text-left text-[13px] ${
              item.disabled
                ? "cursor-default text-slate-300"
                : item.danger
                  ? "text-rose-600 hover:bg-rose-500 hover:text-white"
                  : "text-slate-800 hover:bg-accent hover:text-white"
            }`}
            onClick={() => {
              onClose();
              item.onClick();
            }}
          >
            {item.icon && (
              <span
                className={`grid w-3.5 shrink-0 place-items-center ${
                  item.disabled ? "" : "text-slate-500 group-hover:text-white"
                }`}
              >
                {item.icon}
              </span>
            )}
            <span className="flex-1 truncate">{item.label}</span>
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
  );
}
