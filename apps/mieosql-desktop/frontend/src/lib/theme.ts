import { useEffect } from "react";

import { useStore } from "./store";
import type { DensitySetting, FontSizeSetting, ThemeSetting } from "./store";

/** Base UI font size (px) per `fontSize` setting. Body defaults to 13px. */
const FONT_PX: Record<FontSizeSetting, string> = {
  small: "12px",
  medium: "13px",
  large: "14px",
};

/** Grid row height per `density` setting. Comfortable matches the 30px default. */
const ROW_H: Record<DensitySetting, string> = {
  comfortable: "30px",
  compact: "26px",
};

function systemIsDark(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-color-scheme: dark)").matches
  );
}

export function resolveTheme(theme: ThemeSetting): "light" | "dark" {
  if (theme === "light") return "light";
  if (theme === "dark") return "dark";
  return systemIsDark() ? "dark" : "light";
}

/**
 * Read settings from the store and apply them to the document:
 * `data-theme` on <html>, font-size scale var, and grid density var.
 */
export function applyTheme(): void {
  if (typeof document === "undefined") return;
  const { settings } = useStore.getState();
  const root = document.documentElement;
  const resolved = resolveTheme(settings.theme);
  root.dataset.theme = resolved;
  root.style.colorScheme = resolved;
  root.style.setProperty("--mieosql-font-scale", FONT_PX[settings.fontSize]);
  root.style.setProperty("--row-h", ROW_H[settings.density]);
  root.style.setProperty("--grid-head-h", ROW_H[settings.density]);
}

/**
 * Invisible binder: applies settings on mount, re-applies on settings
 * changes, and re-resolves when the OS theme changes (theme = "system").
 * Render once near the app root (currently inside `<Ribbon />`).
 */
export function ThemeBinder() {
  useEffect(() => {
    applyTheme();
    const unsub = useStore.subscribe(applyTheme);
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      if (useStore.getState().settings.theme === "system") applyTheme();
    };
    mq.addEventListener("change", onChange);
    return () => {
      unsub();
      mq.removeEventListener("change", onChange);
    };
  }, []);
  return null;
}
