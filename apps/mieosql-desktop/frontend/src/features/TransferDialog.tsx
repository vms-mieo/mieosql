// Data Transfer: copy a table (optionally a column subset / WHERE / LIMIT)
// from a source session to a target session. F4 feature agent.

import { useEffect, useState } from "react";
import {
  AlertTriangle,
  ArrowRight,
  Loader2,
  RefreshCw,
} from "lucide-react";

import { api, errorText } from "../lib/api";
import { useStore } from "../lib/store";
import type {
  ColumnDef,
  SyncEndpoint,
  TransferMode,
  TransferRequest,
  TransferResult,
} from "../lib/bindings";
import { DialogShell } from "./DialogShell";
import {
  EndpointPicker,
  engineLabel,
  pickSourceEndpoint,
  pickTargetEndpoint,
} from "./EndpointPicker";

const INPUT_CLS =
  "h-7 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-800 focus:ring-2 focus:ring-accent/40 focus:outline-none disabled:bg-slate-50 disabled:text-slate-400";
const LABEL_CLS =
  "mb-1 block text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase";
const BTN_SECONDARY =
  "flex h-7 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50 disabled:opacity-50";
const BTN_PRIMARY =
  "flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover disabled:opacity-50";

const MODES: Array<{
  value: TransferMode;
  label: string;
  description: string;
  destructive: boolean;
}> = [
  {
    value: "create_append",
    label: "Create + Append",
    description:
      "Create the target table when missing (from the source definition), then insert all rows.",
    destructive: false,
  },
  {
    value: "append",
    label: "Append",
    description:
      "Insert rows into the existing target table; fails when the table is missing or columns do not match.",
    destructive: false,
  },
  {
    value: "truncate_append",
    label: "Truncate + Append",
    description:
      "Delete every row of the target table first, then insert. Destructive.",
    destructive: true,
  },
  {
    value: "drop_create",
    label: "Drop + Create",
    description:
      "Drop the target table, recreate it, then insert. Destructive.",
    destructive: true,
  },
];

function optionalPositive(raw: string): number | null {
  const n = Number.parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

export function TransferDialog({ onClose }: { onClose: () => void }) {
  const sessions = useStore((s) => s.sessions);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const selected = useStore((s) => s.selected);
  const toast = useStore((s) => s.toast);

  const [source, setSource] = useState<SyncEndpoint | null>(() =>
    pickSourceEndpoint(sessions, selected, activeSessionId),
  );
  const [target, setTarget] = useState<SyncEndpoint | null>(() => {
    const src = pickSourceEndpoint(sessions, selected, activeSessionId);
    return pickTargetEndpoint(sessions, src, activeSessionId);
  });
  const [sourceTable, setSourceTable] = useState(() => selected?.table ?? "");
  const [targetTable, setTargetTable] = useState(() => selected?.table ?? "");
  const [targetTouched, setTargetTouched] = useState(false);
  const [mode, setMode] = useState<TransferMode>("create_append");
  const [columns, setColumns] = useState<ColumnDef[]>([]);
  const [selectedColumns, setSelectedColumns] = useState<Set<string>>(
    new Set(),
  );
  const [loadingColumns, setLoadingColumns] = useState(false);
  const [where, setWhere] = useState("");
  const [limit, setLimit] = useState("");
  const [batchSize, setBatchSize] = useState("250");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<TransferResult | null>(null);

  const sourceSession = source
    ? (sessions.find((s) => s.id === source.session_id) ?? null)
    : null;
  const targetSession = target
    ? (sessions.find((s) => s.id === target.session_id) ?? null)
    : null;
  const activeMode = MODES.find((m) => m.value === mode)!;

  const scopeKey = source
    ? `${source.scope.database ?? ""}|${source.scope.schema ?? ""}`
    : "";
  const sourceSessionId = source?.session_id ?? "";

  // Load the source columns whenever the table (or endpoint) changes.
  useEffect(() => {
    if (!sourceSessionId || !sourceTable.trim()) {
      setColumns([]);
      setSelectedColumns(new Set());
      return;
    }
    let cancelled = false;
    setLoadingColumns(true);
    setError(null);
    api
      .tableColumns(sourceSessionId, source?.scope ?? {}, sourceTable.trim())
      .then((cols) => {
        if (cancelled) return;
        setColumns(cols);
        setSelectedColumns(new Set(cols.map((c) => c.name)));
      })
      .catch((err) => {
        if (cancelled) return;
        setColumns([]);
        setSelectedColumns(new Set());
        setError(errorText(err));
      })
      .finally(() => {
        if (!cancelled) setLoadingColumns(false);
      });
    return () => {
      cancelled = true;
    };
    // `source` identity changes on scope edits; scopeKey captures them above.
  }, [sourceSessionId, scopeKey, sourceTable]);

  const changeSourceTable = (value: string) => {
    setSourceTable(value);
    if (!targetTouched) setTargetTable(value);
  };

  const toggleColumn = (name: string) => {
    setSelectedColumns((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };

  const runTransfer = async () => {
    if (!source || !target) {
      setError("Select a source and a target session first.");
      return;
    }
    const tableName = sourceTable.trim();
    if (!tableName) {
      setError("Source table is required.");
      return;
    }
    if (columns.length > 0 && selectedColumns.size === 0) {
      setError(
        "Select at least one column to copy (or reload the column list).",
      );
      return;
    }
    if (
      activeMode.destructive &&
      !window.confirm(
        `“${activeMode.label}” is destructive: ${activeMode.description}\n\n` +
          `Source: ${sourceSession?.name ?? source.session_id}.${tableName}\n` +
          `Target: ${targetSession?.name ?? target.session_id}.${
            targetTable.trim() || tableName
          }\n\nContinue?`,
      )
    ) {
      return;
    }

    const batch = Math.min(1000, optionalPositive(batchSize) ?? 250);

    const request: TransferRequest = {
      source,
      target,
      source_table: tableName,
      target_table: targetTable.trim() || undefined,
      mode,
      columns:
        columns.length > 0 && selectedColumns.size < columns.length
          ? columns
              .filter((c) => selectedColumns.has(c.name))
              .map((c) => c.name)
          : [],
      where_clause: where.trim() || undefined,
      limit: optionalPositive(limit),
      batch_size: batch,
    };

    setRunning(true);
    setError(null);
    setResult(null);
    try {
      const res = await api.transferTable(request);
      setResult(res);
      const warningText =
        res.warnings.length > 0
          ? ` — ${res.warnings.length} warning(s)`
          : "";
      toast(
        res.warnings.length > 0 ? "info" : "success",
        `Transferred ${res.rows.toLocaleString()} row(s) to "${
          res.target_table
        }" in ${res.elapsed_ms} ms${warningText}`,
      );
    } catch (err) {
      setError(errorText(err));
      toast("error", errorText(err));
    } finally {
      setRunning(false);
    }
  };

  return (
    <DialogShell
      title="Data Transfer"
      width={820}
      onClose={onClose}
      footer={
        <>
          <button className={BTN_SECONDARY} onClick={onClose}>
            Close
          </button>
          <button
            className={BTN_PRIMARY}
            onClick={() => void runTransfer()}
            disabled={running || loadingColumns}
          >
            {running ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <ArrowRight size={13} strokeWidth={1.8} />
            )}
            Transfer
          </button>
        </>
      }
    >
      <div className="space-y-4 p-4">
        <div className="grid grid-cols-2 gap-3">
          <EndpointPicker
            label="Source"
            value={source}
            onChange={setSource}
            sessions={sessions}
          />
          <EndpointPicker
            label="Target"
            value={target}
            onChange={setTarget}
            sessions={sessions}
          />
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className={LABEL_CLS}>Source table</label>
            <input
              className={INPUT_CLS}
              value={sourceTable}
              placeholder="users"
              onChange={(e) => changeSourceTable(e.target.value)}
            />
          </div>
          <div>
            <label className={LABEL_CLS}>Target table (default: same name)</label>
            <input
              className={INPUT_CLS}
              value={targetTable}
              placeholder={sourceTable || "users_copy"}
              onChange={(e) => {
                setTargetTouched(true);
                setTargetTable(e.target.value);
              }}
            />
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className={LABEL_CLS}>Mode</label>
            <select
              className={INPUT_CLS}
              value={mode}
              onChange={(e) => setMode(e.target.value as TransferMode)}
            >
              {MODES.map((m) => (
                <option key={m.value} value={m.value}>
                  {m.label}
                  {m.destructive ? " (destructive)" : ""}
                </option>
              ))}
            </select>
            <div
              className={`mt-1 text-[11px] ${
                activeMode.destructive ? "text-rose-600" : "text-slate-500"
              }`}
            >
              {activeMode.description}
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className={LABEL_CLS}>Row limit (optional)</label>
              <input
                className={INPUT_CLS}
                type="number"
                min={1}
                value={limit}
                placeholder="all rows"
                onChange={(e) => setLimit(e.target.value)}
              />
            </div>
            <div>
              <label className={LABEL_CLS}>Batch size</label>
              <input
                className={INPUT_CLS}
                type="number"
                min={1}
                max={1000}
                value={batchSize}
                onChange={(e) => setBatchSize(e.target.value)}
              />
            </div>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className={LABEL_CLS}>WHERE clause (optional)</label>
            <input
              className={INPUT_CLS}
              value={where}
              placeholder="created_at >= '2024-01-01'"
              onChange={(e) => setWhere(e.target.value)}
            />
          </div>
          <div className="self-end pb-0.5 text-[11px] text-slate-500">
            {source && target && (
              <>
                {engineLabel(sourceSession?.engine ?? "mysql")} →{" "}
                {engineLabel(targetSession?.engine ?? "mysql")} · values are
                converted best-effort
              </>
            )}
          </div>
        </div>

        <div>
          <div className="mb-1.5 flex items-center justify-between">
            <span className={LABEL_CLS}>
              Columns ({selectedColumns.size}/{columns.length})
            </span>
            <div className="flex items-center gap-2">
              {loadingColumns && (
                <span className="flex items-center gap-1 text-[11px] text-slate-500">
                  <Loader2 size={11} className="animate-spin" />
                  loading…
                </span>
              )}
              <button
                className="rounded-md px-1.5 py-0.5 text-[11px] font-medium text-accent hover:bg-accent/10 disabled:opacity-40"
                disabled={columns.length === 0}
                onClick={() =>
                  setSelectedColumns(new Set(columns.map((c) => c.name)))
                }
              >
                All
              </button>
              <button
                className="rounded-md px-1.5 py-0.5 text-[11px] font-medium text-accent hover:bg-accent/10 disabled:opacity-40"
                disabled={columns.length === 0}
                onClick={() => setSelectedColumns(new Set())}
              >
                None
              </button>
              <span className="text-[11px] text-slate-500">
                empty list = copy every column
              </span>
            </div>
          </div>
          <div className="custom-scrollbar max-h-36 overflow-auto rounded-md border border-slate-200 bg-white p-2">
            {columns.length === 0 ? (
              <div className="px-1 py-2 text-[12.5px] text-slate-500">
                {sourceTable.trim()
                  ? "No columns loaded — check the session / table."
                  : "Enter a source table to load its columns."}
              </div>
            ) : (
              <div className="grid grid-cols-3 gap-x-3 gap-y-1">
                {columns.map((col) => (
                  <label
                    key={col.name}
                    className="flex min-w-0 cursor-pointer items-center gap-1.5 text-[12.5px] text-slate-700"
                    title={`${col.name} · ${col.native_type}`}
                  >
                    <input
                      type="checkbox"
                      className="h-3.5 w-3.5 accent-accent"
                      checked={selectedColumns.has(col.name)}
                      onChange={() => toggleColumn(col.name)}
                    />
                    <span className="truncate font-code">{col.name}</span>
                    {col.is_primary_key && (
                      <span className="shrink-0 rounded bg-amber-100 px-1 text-[10.5px] font-medium text-amber-700">
                        PK
                      </span>
                    )}
                    <span className="ml-auto shrink-0 truncate text-[11px] text-slate-500">
                      {col.native_type}
                    </span>
                  </label>
                ))}
              </div>
            )}
          </div>
          <div className="mt-1 flex items-center gap-1 text-[11px] text-slate-500">
            <RefreshCw size={11} strokeWidth={1.8} />
            columns load automatically when the source table changes
          </div>
        </div>

        {error && (
          <div className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-[12.5px] whitespace-pre-wrap text-rose-600">
            {error}
          </div>
        )}

        {result && (
          <div
            className={`rounded-md border px-3 py-2 text-[12.5px] ${
              result.warnings.length > 0
                ? "border-amber-200 bg-amber-50 text-amber-800"
                : "border-emerald-200 bg-emerald-50 text-emerald-800"
            }`}
          >
            <div className="font-medium">
              {result.rows.toLocaleString()} row(s) → “
              {result.target_table}” · {result.elapsed_ms} ms
              {result.created ? " · target table created" : ""}
              {result.truncated ? " · truncated by row limit" : ""}
            </div>
            {result.warnings.length > 0 && (
              <ul className="mt-1 ml-4 list-disc space-y-0.5">
                {result.warnings.map((w, i) => (
                  <li key={i}>
                    <AlertTriangle size={11} strokeWidth={1.8} className="mr-1 inline" />
                    {w}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}

        {selected && source && selected.sessionId !== source.session_id && (
          <p className="text-[11px] text-slate-500">
            Object selection came from another session — verify the source
            table name before transferring.
          </p>
        )}
      </div>
    </DialogShell>
  );
}
