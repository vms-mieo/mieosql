// Platform chrome flags. Evaluated at import time — import this module FIRST
// so `data-window-shape` is set before any component module reads it.

export const isMacChrome =
  typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

if (isMacChrome && typeof document !== "undefined") {
  document.documentElement.dataset.windowShape = "macos";
}
