// Backup dialog: dump one or more tables (schema + optional data) to a .sql
// file. Opened via `store.openObjectDialog("backup", { sessionId, scope })`;
// when no scope is passed the session's current database is used.

import { useEffect, useState } from "react";
import { Database, Download, Loader2, RefreshCw } from "lucide-react";
import { save as saveFile } from "@tauri-apps/plugin-dialog";

import type { Scope } from "../../lib/bindings";
import type { BackupSummary } from "../../lib/dto";
import { api, errorText } from "../../lib/api";
import { formatBytes } from "../../lib/format";
import { useStore } from "../../lib/store";
import {
  DLG_BTN,
  DLG_BTN_PRIMARY,
  DLG_LABEL,
  DialogError,
  ObjectDialogShell,
} from "./DialogChrome";

function stamp(): string {
  return new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-");
}

function sanitize(part: string): string {
  return part.replace(/[^\w.-]+/g, "_").slice(0, 40);
}

export function BackupDialog({
  sessionId,
  scope: scopeProp,
  onClose,
}: {
  sessionId: string | null;
  scope?: Scope;
  onClose: () => void;
}) {
  const session = useStore(
    (s) => s.sessions.find((x) => x.id === sessionId) ?? null,
  );
  const toast = useStore((s) => s.toast);

  // Fallback: the session's current database. An empty scope object (as sent
  // by the ribbon/menu actions) counts as "not provided".
  const hasScope = Boolean(scopeProp?.database || scopeProp?.schema);
  const scope: Scope = hasScope
    ? scopeProp!
    : { database: session?.database ?? undefined };
  const scopeKey = `${scope.database ?? ""}|${scope.schema ?? ""}`;

  const [tables, setTables] = useState<string[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [includeData, setIncludeData] = useState(true);
  const [path, setPath] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [summary, setSummary] = useState<BackupSummary | null>(null);

  const loadTables = async () => {
    if (!sessionId) {
      setTables([]);
      setSelected(new Set());
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const objects = await api.listObjects(sessionId, scope, "table", null);
      const names = objects.map((o) => o.name);
      setTables(names);
      setSelected(new Set(names));
    } catch (err) {
      setTables([]);
      setSelected(new Set());
      setError(errorText(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    setSummary(null);
    void loadTables();
    // Re-load when the session or the effective scope changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, scopeKey]);

  const allChecked = tables.length > 0 && selected.size === tables.length;
  const someChecked = selected.size > 0 && selected.size < tables.length;

  const toggleAll = () =>
    setSelected(allChecked ? new Set() : new Set(tables));

  const toggle = (name: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });

  const chooseOutput = async () => {
    const db = scope.database ?? session?.database ?? "database";
    let picked: string | null = null;
    try {
      picked = await saveFile({
        title: "Save SQL backup",
        defaultPath: `mieosql-backup-${sanitize(db)}-${stamp()}.sql`,
        filters: [{ name: "SQL backup", extensions: ["sql"] }],
      });
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!picked) return; // cancelled
    setPath(picked);
    setError(null);
  };

  const canRun =
    sessionId !== null && selected.size > 0 && path !== null && !running;

  const run = async () => {
    if (!sessionId || !path) return;
    setRunning(true);
    setError(null);
    setSummary(null);
    try {
      const all = selected.size === tables.length;
      const res = await api.dumpBackup(
        sessionId,
        scope,
        path,
        includeData,
        all ? null : [...selected],
      );
      setSummary(res);
      const summaryText =
        `${res.tables.toLocaleString()} table(s) · ${res.rows.toLocaleString()} rows · ` +
        `${formatBytes(res.bytes)} in ${res.elapsed_ms} ms`;
      if (res.errors.length > 0) {
        setError(res.errors.join("\n"));
        toast("error", `Backup finished with errors — ${summaryText}`);
      } else {
        toast("success", `Backup complete — ${summaryText}\n${path}`);
      }
    } catch (err) {
      setError(errorText(err));
      toast("error", errorText(err));
    } finally {
      setRunning(false);
    }
  };

  return (
    <ObjectDialogShell
      title="Backup Database"
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
              <Download size={13} strokeWidth={1.8} />
            )}
            Backup
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        <div className="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12px] text-slate-600">
          <Database size={13} strokeWidth={1.7} className="shrink-0 text-slate-500" />
          {session ? (
            <>
              <span className="font-medium text-slate-800">{session.name}</span>
              <span>
                · {scope.schema ?? scope.database ?? session.database ?? "default scope"}
              </span>
            </>
          ) : (
            <span>No active session.</span>
          )}
        </div>

        {!sessionId && (
          <DialogError text="Open a connection before running a backup." />
        )}

        {/* ------------------------------------------------------- tables */}
        <div>
          <div className="mb-1 flex items-center justify-between">
            <label className={DLG_LABEL}>Tables</label>
            <div className="flex items-center gap-2 text-[11.5px] text-slate-500">
              <span>
                {selected.size} / {tables.length} selected
              </span>
              <button
                className="grid h-6 w-6 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
                title="Reload tables"
                disabled={loading || running || !sessionId}
                onClick={() => void loadTables()}
              >
                <RefreshCw
                  size={12}
                  strokeWidth={1.8}
                  className={loading ? "animate-spin" : undefined}
                />
              </button>
            </div>
          </div>
          <div className="custom-scrollbar max-h-[220px] overflow-auto rounded-md border border-slate-200 bg-white">
            {loading ? (
              <div className="flex items-center gap-2 px-3 py-3 text-[12.5px] text-slate-500">
                <Loader2 size={13} className="animate-spin" />
                Loading tables…
              </div>
            ) : tables.length === 0 ? (
              <div className="px-3 py-3 text-[12.5px] text-slate-500">
                No tables found in this scope.
              </div>
            ) : (
              <>
                <label className="flex items-center gap-2 border-b border-slate-200/70 bg-sidebar px-3 py-1.5 text-[12.5px] font-medium text-slate-700">
                  <input
                    type="checkbox"
                    className="h-3.5 w-3.5 accent-accent"
                    checked={allChecked}
                    ref={(el) => {
                      if (el) el.indeterminate = someChecked;
                    }}
                    disabled={running}
                    onChange={toggleAll}
                  />
                  Select all
                </label>
                {tables.map((name) => (
                  <label
                    key={name}
                    className="flex cursor-default items-center gap-2 px-3 py-1 text-[12.5px] text-slate-700 hover:bg-slate-900/[0.03]"
                  >
                    <input
                      type="checkbox"
                      className="h-3.5 w-3.5 accent-accent"
                      checked={selected.has(name)}
                      disabled={running}
                      onChange={() => toggle(name)}
                    />
                    <span className="truncate font-code">{name}</span>
                  </label>
                ))}
              </>
            )}
          </div>
        </div>

        <label className="flex items-center gap-2 text-[13px] text-slate-700">
          <input
            type="checkbox"
            className="h-3.5 w-3.5 accent-accent"
            checked={includeData}
            disabled={running}
            onChange={(e) => setIncludeData(e.target.checked)}
          />
          Include table data (uncheck for schema only)
        </label>

        {/* ------------------------------------------------------- output */}
        <div>
          <label className={DLG_LABEL}>Output file *</label>
          <div className="flex items-center gap-2">
            <button
              className={DLG_BTN}
              onClick={() => void chooseOutput()}
              disabled={running}
            >
              <Download size={13} strokeWidth={1.8} />
              Choose File…
            </button>
            <span
              className={`min-w-0 flex-1 truncate font-code text-[12px] ${
                path ? "text-slate-700" : "text-slate-400"
              }`}
              title={path ?? undefined}
            >
              {path ?? "No output file selected"}
            </span>
          </div>
        </div>

        {summary && summary.errors.length === 0 && (
          <div className="rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-[12.5px] text-emerald-700">
            Backed up {summary.tables.toLocaleString()} table(s) ·{" "}
            {summary.rows.toLocaleString()} rows · {formatBytes(summary.bytes)} in{" "}
            {summary.elapsed_ms} ms
          </div>
        )}

        {error && <DialogError text={error} />}
      </div>
    </ObjectDialogShell>
  );
}
