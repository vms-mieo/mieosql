// Restore dialog: execute a .sql script statement by statement.
// Opened via `store.openObjectDialog("restore", { sessionId })`.
//
// Statements run one `executeBatch` call at a time so the loop stays
// cancellable between statements and stops at the first error.

import { useRef, useState } from "react";
import { FileCode2, FolderOpen, Loader2, Play, Square } from "lucide-react";
import { open as openFile } from "@tauri-apps/plugin-dialog";

import type { ScriptStatements } from "../../lib/dto";
import { api, errorText } from "../../lib/api";
import { useStore } from "../../lib/store";
import {
  DLG_BTN,
  DLG_BTN_PRIMARY,
  DLG_LABEL,
  DialogError,
  DialogNotice,
  ObjectDialogShell,
} from "./DialogChrome";

function shortPath(path: string, max = 70): string {
  if (path.length <= max) return path;
  return `…${path.slice(path.length - max + 1)}`;
}

function firstLine(sql: string, max = 120): string {
  const line = sql.split("\n").find((l) => l.trim().length > 0) ?? sql;
  return line.length > max ? `${line.slice(0, max)}…` : line;
}

export function RestoreDialog({
  sessionId,
  onClose,
}: {
  sessionId: string | null;
  onClose: () => void;
}) {
  const toast = useStore((s) => s.toast);

  const [path, setPath] = useState<string | null>(null);
  const [script, setScript] = useState<ScriptStatements | null>(null);
  const [loadingScript, setLoadingScript] = useState(false);
  const [running, setRunning] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [done, setDone] = useState(0);
  const [current, setCurrent] = useState(0);
  const [failedAt, setFailedAt] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const cancelRef = useRef(false);

  const pickFile = async () => {
    setError(null);
    let picked: string | null = null;
    try {
      const selection = await openFile({
        title: "Choose SQL script",
        multiple: false,
        filters: [{ name: "SQL script", extensions: ["sql"] }],
      });
      picked = typeof selection === "string" ? selection : null;
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!picked) return; // cancelled

    setPath(picked);
    setScript(null);
    setDone(0);
    setCurrent(0);
    setFailedAt(null);
    setLoadingScript(true);
    try {
      setScript(await api.scriptStatements(picked));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setLoadingScript(false);
    }
  };

  const total = script?.statements.length ?? 0;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;
  const canRun = sessionId !== null && total > 0 && !running;

  const run = async () => {
    if (!sessionId || !script || total === 0) return;
    cancelRef.current = false;
    setRunning(true);
    setStopping(false);
    setDone(0);
    setCurrent(0);
    setFailedAt(null);
    setError(null);
    const startedAt = Date.now();
    let ok = 0;
    let failure: string | null = null;
    let failedNumber: number | null = null;

    for (let i = 0; i < script.statements.length; i++) {
      if (cancelRef.current) break;
      setCurrent(i + 1);
      try {
        // One statement per call keeps the loop simple and cancellable.
        const res = await api.executeBatch(sessionId, [script.statements[i]!], false);
        if (res.failed > 0) {
          failure = res.errors[0] ?? `Statement ${i + 1} failed.`;
          failedNumber = i + 1;
          break;
        }
        ok += 1;
        setDone(ok);
      } catch (err) {
        failure = errorText(err);
        failedNumber = i + 1;
        break;
      }
    }

    const cancelled = cancelRef.current;
    setRunning(false);
    setStopping(false);
    if (failure !== null) {
      setFailedAt(failedNumber);
      setError(failure);
      const statement = failedNumber !== null ? script.statements[failedNumber - 1] : undefined;
      if (statement) {
        setError(`Statement ${failedNumber}: ${failure}\n${firstLine(statement)}`);
      }
      toast(
        "error",
        `Restore stopped at statement ${failedNumber ?? "?"} — ${ok.toLocaleString()} applied`,
      );
    } else if (cancelled) {
      toast("info", `Restore cancelled — ${ok.toLocaleString()} statement(s) applied`);
    } else {
      toast(
        "success",
        `Restore complete — ${ok.toLocaleString()} statement(s) applied in ${
          Date.now() - startedAt
        } ms`,
      );
    }
  };

  return (
    <ObjectDialogShell
      title="Restore SQL Script"
      width={640}
      busy={running}
      onClose={onClose}
      footer={
        <>
          <button
            className={DLG_BTN}
            onClick={() => {
              if (running) {
                cancelRef.current = true;
                setStopping(true);
              } else {
                onClose();
              }
            }}
            disabled={stopping}
          >
            {running && <Square size={11} strokeWidth={1.8} className="text-rose-500" />}
            {running ? (stopping ? "Stopping…" : "Stop") : "Cancel"}
          </button>
          <button
            className={DLG_BTN_PRIMARY}
            onClick={() => void run()}
            disabled={!canRun}
          >
            {running ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <Play size={13} strokeWidth={1.8} />
            )}
            {running ? "Running…" : "Run Script"}
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        {!sessionId && (
          <DialogError text="Open a connection before restoring a script." />
        )}

        {/* ------------------------------------------------------- source */}
        <div>
          <label className={DLG_LABEL}>SQL script *</label>
          <div className="flex items-center gap-2">
            <button
              className={DLG_BTN}
              onClick={() => void pickFile()}
              disabled={running || loadingScript}
            >
              {loadingScript ? (
                <Loader2 size={13} className="animate-spin" />
              ) : (
                <FolderOpen size={13} strokeWidth={1.8} />
              )}
              Choose File…
            </button>
            <span
              className={`min-w-0 flex-1 truncate font-code text-[12px] ${
                path ? "text-slate-700" : "text-slate-400"
              }`}
              title={path ?? undefined}
            >
              {path ? shortPath(path) : "No script selected"}
            </span>
          </div>
        </div>

        {/* ---------------------------------------------------- statement */}
        {script && (
          <div className="flex items-start gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12.5px] text-slate-600">
            <FileCode2 size={14} strokeWidth={1.7} className="mt-px shrink-0 text-slate-500" />
            <div>
              <span className="font-medium text-slate-800">
                {total.toLocaleString()} statement(s)
              </span>{" "}
              parsed from the script. Statements run one at a time and stop at
              the first error.
            </div>
          </div>
        )}

        {script && total === 0 && (
          <DialogNotice>No executable statements found in this file.</DialogNotice>
        )}

        {script && script.warnings.length > 0 && (
          <DialogNotice>
            <ul className="list-inside list-disc space-y-0.5">
              {script.warnings.slice(0, 5).map((w, i) => (
                <li key={i}>{w}</li>
              ))}
              {script.warnings.length > 5 && (
                <li className="list-none text-amber-600">
                  …and {script.warnings.length - 5} more warning(s)
                </li>
              )}
            </ul>
          </DialogNotice>
        )}

        {/* ----------------------------------------------------- progress */}
        {(running || done > 0 || failedAt !== null) && total > 0 && (
          <div>
            <div className="mb-1 flex items-center justify-between text-[11.5px] text-slate-500">
              <span>
                {current.toLocaleString()} / {total.toLocaleString()} statement(s)
              </span>
              <span>{pct}%</span>
            </div>
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-slate-200">
              <div
                className={`h-full rounded-full transition-[width] duration-150 ${
                  failedAt !== null ? "bg-rose-500" : "bg-accent"
                }`}
                style={{ width: `${pct}%` }}
              />
            </div>
          </div>
        )}

        {failedAt !== null && (
          <div className="text-[12px] text-rose-600">
            Stopped at statement {failedAt.toLocaleString()} of{" "}
            {total.toLocaleString()} — {done.toLocaleString()} applied.
          </div>
        )}

        {error && <DialogError text={error} />}
      </div>
    </ObjectDialogShell>
  );
}
