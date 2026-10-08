import { useSyncExternalStore } from "react";
import { confirm, message } from "@tauri-apps/plugin-dialog";

/**
 * Native-dialog replacements for the Tauri webview.
 *
 * Background: `tauri-plugin-dialog` injects an init script that overrides
 * `window.confirm` with an async call to `plugin:dialog|confirm` — a command
 * that no longer exists backend-side (v2 only exposes `message`/`open`/`save`).
 * Every `window.confirm(...)` therefore rejects with
 * "dialog.confirm not allowed. Command not found" AND returns a truthy
 * Promise, so `if (!window.confirm(...)) return;` never blocks — destructive
 * actions would run without any confirmation. `window.prompt` is not
 * polyfilled at all and silently returns null on macOS (WKWebView).
 *
 * Always use these helpers instead of `window.confirm`/`window.prompt`/
 * `window.alert`.
 */

export function isTauri(): boolean {
  return (
    typeof window !== "undefined" && "__TAURI_INTERNALS__" in window
  );
}

export type ConfirmKind = "info" | "warning" | "error";

/** Ok/Cancel question. Returns true only when the user confirms. */
export async function confirmDialog(
  msg: string,
  opts?: { title?: string; kind?: ConfirmKind },
): Promise<boolean> {
  if (isTauri()) {
    try {
      return await confirm(msg, opts);
    } catch {
      return false;
    }
  }
  return window.confirm(msg);
}

/** Fire-and-forget informational dialog. */
export async function alertDialog(msg: string, title?: string): Promise<void> {
  if (isTauri()) {
    try {
      await message(msg, title ? { title } : undefined);
    } catch {
      /* best-effort */
    }
    return;
  }
  window.alert(msg);
}

// ---------------------------------------------------------------------------
// Text prompt (the dialog plugin has no prompt API, so this is in-app).
// ---------------------------------------------------------------------------

export type PromptOptions = {
  title?: string;
  okLabel?: string;
  password?: boolean;
  placeholder?: string;
};

export type PromptRequest = {
  id: number;
  message: string;
  defaultValue: string;
  title: string;
  okLabel: string;
  password: boolean;
  placeholder: string;
  resolve: (value: string | null) => void;
};

let promptSeq = 1;
let currentPrompt: PromptRequest | null = null;
const promptListeners = new Set<() => void>();

function emitPrompt() {
  promptListeners.forEach((l) => l());
}

function subscribePrompt(fn: () => void): () => void {
  promptListeners.add(fn);
  return () => {
    promptListeners.delete(fn);
  };
}

function getPromptSnapshot(): PromptRequest | null {
  return currentPrompt;
}

/**
 * Ask for a text value. Resolves with the typed string, or `null` when the
 * user cancels. Never uses `window.prompt` (broken in the Tauri webview).
 */
export function promptDialog(
  msg: string,
  defaultValue = "",
  opts?: PromptOptions,
): Promise<string | null> {
  if (!isTauri()) {
    return Promise.resolve(window.prompt(msg, defaultValue));
  }
  return new Promise<string | null>((resolve) => {
    currentPrompt = {
      id: promptSeq++,
      message: msg,
      defaultValue,
      title: opts?.title ?? "Input",
      okLabel: opts?.okLabel ?? "OK",
      password: opts?.password ?? false,
      placeholder: opts?.placeholder ?? "",
      resolve,
    };
    emitPrompt();
  });
}

/** React binding for the prompt host. */
export function usePromptRequest(): PromptRequest | null {
  return useSyncExternalStore(subscribePrompt, getPromptSnapshot);
}

/** Resolve the active prompt (called by the host component). */
export function resolvePrompt(value: string | null) {
  const req = currentPrompt;
  currentPrompt = null;
  emitPrompt();
  req?.resolve(value);
}
