// Data import dialog (CSV / JSON). Opened via
// `store.openObjectDialog("import", { sessionId, scope, table })`.

import { useState } from "react";
import { FileDown, FolderOpen, Loader2 } from "lucide-react";
import { open as openFile } from "@tauri-apps/plugin-dialog";

import type { Scope } from "../../lib/bindings";
import type { ImportSummary } from "../../lib/dto";
import { api, errorText } from "../../lib/api";
import { useStore } from "../../lib/store";
import {
  DLG_BTN,
  DLG_BTN_PRIMARY,
  DLG_INPUT,
  DLG_LABEL,
  DialogError,
  ObjectDialogShell,
} from "./DialogChrome";

type ImportFormat = "csv" | "json";

function detectFormat(path: string): ImportFormat {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return ext === "json" || ext === "jsonl" ? "json" : "csv";
}

function shortPath(path: string, max = 70): string {
  if (path.length <= max) return path;
  return `…${path.slice(path.length - max + 1)}`;
}

export function ImportDialog({
  sessionId,
  scope,
  table,
  onClose,
}: {
  sessionId: string | null;
  scope: Scope;
  table?: string;
  onClose: () => void;
}) {
  const toast = useStore((s) => s.toast);

  const [path, setPath] = useState<string | null>(null);
  const [format, setFormat] = useState<ImportFormat>("csv");
  const [tableName, setTableName] = useState(table ?? "");
  const [delimiter, setDelimiter] = useState(",");
  const [hasHeader, setHasHeader] = useState(true);
  const [nullMarker, setNullMarker] = useState("");
  const [mode, setMode] = useState<"append" | "truncate">("append");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ImportSummary | null>(null);

  const pickFile = async () => {
    setError(null);
    let picked: string | null = null;
    try {
      const selection = await openFile({
        title: "Choose import file",
        multiple: false,
        filters: [
          { name: "Data files", extensions: ["csv", "json", "jsonl", "txt"] },
        ],
      });
      picked = typeof selection === "string" ? selection : null;
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!picked) return; // cancelled
    setPath(picked);
    setFormat(detectFormat(picked));
    setResult(null);
  };

  const trimmedTable = tableName.trim();
  const canRun = sessionId !== null && path !== null && trimmedTable !== "" && !running;

  const run = async () => {
    if (!sessionId) {
      setError("Open a connection before importing.");
      return;
    }
    if (!path) {
      setError("Choose a file to import.");
      return;
    }
    if (!trimmedTable) {
      setError("Table name is required.");
      return;
    }
    setRunning(true);
    setError(null);
    setResult(null);
    try {
      const res = await api.importFile(sessionId, scope, trimmedTable, path, format, {
        delimiter:
          format === "csv" && delimiter.trim() !== "" ? delimiter : null,
        has_header: hasHeader,
        null_marker: nullMarker,
        mode,
        batch_size: null,
      });
      setResult(res);
      const summary =
        `${res.rows_inserted.toLocaleString()} of ${res.rows_read.toLocaleString()} rows imported` +
        ` in ${res.elapsed_ms} ms` +
        (res.rows_failed > 0
          ? ` — ${res.rows_failed.toLocaleString()} failed`
          : "");
      toast(res.rows_failed > 0 ? "error" : "success", summary);
    } catch (err) {
      setError(errorText(err));
      toast("error", errorText(err));
    } finally {
      setRunning(false);
    }
  };

  return (
    <ObjectDialogShell
      title="Import Data"
      width={640}
      busy={running}
      onClose={onClose}
      footer={
        <>
          <button className={DLG_BTN} onClick={onClose} disabled={running}>
            Cancel
          </button>
          <button
            className={DLG_BTN_PRIMARY}
            onClick={() => void run()}
            disabled={!canRun}
          >
            {running ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <FileDown size={13} strokeWidth={1.8} />
            )}
            Import
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        {!sessionId && (
          <DialogError text="Open a connection before importing data." />
        )}

        {/* ------------------------------------------------------- source */}
        <div>
          <label className={DLG_LABEL}>Source file *</label>
          <div className="flex items-center gap-2">
            <button
              className={DLG_BTN}
              onClick={() => void pickFile()}
              disabled={running}
            >
              <FolderOpen size={13} strokeWidth={1.8} />
              Choose File…
            </button>
            <span
              className={`min-w-0 flex-1 truncate font-code text-[12px] ${
                path ? "text-slate-700" : "text-slate-400"
              }`}
              title={path ?? undefined}
            >
              {path ? shortPath(path) : "No file selected"}
            </span>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className={DLG_LABEL}>Format</label>
            <select
              className={DLG_INPUT}
              value={format}
              disabled={running}
              onChange={(e) => setFormat(e.target.value as ImportFormat)}
            >
              <option value="csv">CSV / delimited text</option>
              <option value="json">JSON / JSON Lines</option>
            </select>
          </div>
          <div>
            <label className={DLG_LABEL}>Target table *</label>
            <input
              className={DLG_INPUT}
              value={tableName}
              placeholder="table_name"
              disabled={running}
              onChange={(e) => setTableName(e.target.value)}
            />
          </div>
        </div>

        {/* ------------------------------------------------------ options */}
        {format === "csv" ? (
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className={DLG_LABEL}>Delimiter</label>
              <input
                className={`${DLG_INPUT} font-code`}
                value={delimiter}
                placeholder=", ; tab | — empty = auto"
                maxLength={4}
                disabled={running}
                onChange={(e) => setDelimiter(e.target.value)}
              />
            </div>
            <div>
              <label className={DLG_LABEL}>NULL marker</label>
              <input
                className={`${DLG_INPUT} font-code`}
                value={nullMarker}
                placeholder="empty = no marker"
                disabled={running}
                onChange={(e) => setNullMarker(e.target.value)}
              />
            </div>
            <label className="flex items-center gap-2 pt-4 text-[13px] text-slate-700">
              <input
                type="checkbox"
                className="h-3.5 w-3.5 accent-accent"
                checked={hasHeader}
                disabled={running}
                onChange={(e) => setHasHeader(e.target.checked)}
              />
              First row is header
            </label>
          </div>
        ) : (
          <p className="text-[11.5px] text-slate-500">
            JSON: an array of objects, a single object, or JSON Lines
            (one object per line). Keys are matched to column names.
          </p>
        )}

        <div>
          <label className={DLG_LABEL}>Mode</label>
          <select
            className={DLG_INPUT}
            value={mode}
            disabled={running}
            onChange={(e) => setMode(e.target.value as "append" | "truncate")}
          >
            <option value="append">Append rows</option>
            <option value="truncate">Truncate table first</option>
          </select>
          {mode === "truncate" && (
            <p className="mt-1 text-[11.5px] text-amber-600">
              Every existing row in “{trimmedTable || "the table"}” is deleted
              before the import.
            </p>
          )}
        </div>

        {/* ------------------------------------------------------- result */}
        {result && (
          <div className="rounded-md border border-slate-200 bg-white p-3 text-[12.5px]">
            <div className="grid grid-cols-3 gap-2 text-center">
              <div className="rounded-md bg-sidebar py-2">
                <div className="text-[15px] font-semibold text-slate-900">
                  {result.rows_read.toLocaleString()}
                </div>
                <div className="text-[11px] text-slate-500">rows read</div>
              </div>
              <div className="rounded-md bg-emerald-50 py-2">
                <div className="text-[15px] font-semibold text-emerald-700">
                  {result.rows_inserted.toLocaleString()}
                </div>
                <div className="text-[11px] text-emerald-700/80">inserted</div>
              </div>
              <div
                className={`rounded-md py-2 ${
                  result.rows_failed > 0 ? "bg-rose-50" : "bg-sidebar"
                }`}
              >
                <div
                  className={`text-[15px] font-semibold ${
                    result.rows_failed > 0 ? "text-rose-600" : "text-slate-900"
                  }`}
                >
                  {result.rows_failed.toLocaleString()}
                </div>
                <div className="text-[11px] text-slate-500">failed</div>
              </div>
            </div>
            <div className="mt-2 text-right text-[11.5px] text-slate-500">
              {result.elapsed_ms} ms
              {result.rows_failed > 0 && result.errors.length > 0
                ? ` · ${result.errors.length} error(s)`
                : ""}
            </div>
            {result.errors.length > 0 && (
              <ul className="custom-scrollbar mt-1 max-h-[110px] list-inside list-disc overflow-auto rounded-md border border-rose-200 bg-rose-50 p-2 font-code text-[11.5px] text-rose-600">
                {result.errors.slice(0, 5).map((e, i) => (
                  <li key={i} className="break-words">
                    {e}
                  </li>
                ))}
                {result.errors.length > 5 && (
                  <li className="list-none text-rose-500">
                    …and {result.errors.length - 5} more error(s)
                  </li>
                )}
              </ul>
            )}
          </div>
        )}

        {error && <DialogError text={error} />}
      </div>
    </ObjectDialogShell>
  );
}
