// SQL script runner: picks a .sql file, shows what will run in a query tab
// and executes the statements sequentially. Progress is reported with coarse
// toasts (every 25 statements) to avoid toast spam. v1 has no cancel: each
// statement is executed as an independent one-statement batch and the run
// stops at the first failure, leaving the remaining statements untouched.

import { open } from "@tauri-apps/plugin-dialog";

import { api, errorText } from "./api";
import { useStore } from "./store";
import { confirmUnsafeWrites } from "./store";

/** Report progress every N executed statements. */
const PROGRESS_EVERY = 25;

export type ScriptRunResult = {
  total: number;
  applied: number;
  failed: number;
  elapsedMs: number;
  /** 1-based statement number the run stopped at, or null when complete. */
  stoppedAt: number | null;
};

/** Execute `statements` one by one, sequentially, stopping at the first error. */
export async function runScript(
  tabId: string,
  sessionId: string,
  statements: string[],
): Promise<ScriptRunResult> {
  const total = statements.length;
  if (!(await confirmUnsafeWrites(statements.join("\n")))) {
    useStore.getState().toast("info", "Script run cancelled");
    return { total, applied: 0, failed: 0, elapsedMs: 0, stoppedAt: null };
  }
  const startedAt = Date.now();
  let applied = 0;
  let failed = 0;
  let stoppedAt: number | null = null;

  // Keep the script visible while it runs (the caller created this tab).
  useStore.getState().setActiveTab(tabId);
  useStore.getState().toast("info", `Running script — ${total} statements`);

  for (let i = 0; i < total; i++) {
    const statement = statements[i]!;
    try {
      const result = await api.executeBatch(sessionId, [statement], false);
      applied += result.applied;
      failed += result.failed;
      if (result.failed > 0) {
        stoppedAt = i + 1;
        useStore
          .getState()
          .toast(
            "error",
            `Statement ${i + 1}/${total} failed: ${result.errors[0] ?? "unknown error"}`,
          );
        break;
      }
    } catch (err) {
      failed += 1;
      stoppedAt = i + 1;
      useStore
        .getState()
        .toast("error", `Statement ${i + 1}/${total} failed: ${errorText(err)}`);
      break;
    }
    const done = i + 1;
    if (done % PROGRESS_EVERY === 0 && done < total) {
      useStore.getState().toast("info", `Running script… ${done}/${total}`);
    }
  }

  const elapsedMs = Date.now() - startedAt;
  if (stoppedAt === null) {
    useStore
      .getState()
      .toast(
        "success",
        `Script finished — ${applied} statement${applied === 1 ? "" : "s"} applied in ${elapsedMs} ms`,
      );
  } else {
    useStore
      .getState()
      .toast(
        "error",
        `Script stopped at statement ${stoppedAt}/${total} — ${applied} applied before the error`,
      );
  }
  return { total, applied, failed, elapsedMs, stoppedAt };
}

/**
 * File-menu action: choose a .sql file, parse it into statements, open them in
 * a query tab (so the user sees exactly what runs) and start the runner.
 */
export async function runScriptFile(): Promise<void> {
  const store = useStore.getState();
  const activeTab = store.tabs.find((t) => t.id === store.activeTabId);
  const sessionId = activeTab?.sessionId ?? store.activeSessionId;
  if (!sessionId) {
    store.toast("error", "Connect to a database first");
    return;
  }

  let path: string | null = null;
  try {
    path = await open({
      title: "Open SQL File",
      multiple: false,
      filters: [{ name: "SQL", extensions: ["sql"] }],
    });
  } catch (err) {
    store.toast("error", errorText(err));
    return;
  }
  if (!path) return; // cancelled

  let statements: string[];
  let warnings: string[];
  try {
    const parsed = await api.scriptStatements(path);
    statements = parsed.statements;
    warnings = parsed.warnings;
  } catch (err) {
    store.toast("error", errorText(err));
    return;
  }

  if (warnings.length > 0) {
    useStore
      .getState()
      .toast("info", `Script warnings (${warnings.length}): ${warnings[0]}`);
  }
  if (statements.length === 0) {
    useStore.getState().toast("info", "No executable statements found in the file");
    return;
  }

  const tabId = useStore
    .getState()
    .newQueryTab(sessionId, `${statements.join(";\n\n")};\n`);
  void runScript(tabId, sessionId, statements);
}
