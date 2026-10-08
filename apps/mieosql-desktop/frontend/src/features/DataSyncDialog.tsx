// Data Synchronization: diff rows between two tables (possibly on different
// sessions) and apply insert/update/delete statements. F4 feature agent.

import { useState } from "react";
import {
  AlertTriangle,
  Loader2,
  Minus,
  PenLine,
  Play,
  Plus,
} from "lucide-react";

import { api, errorText } from "../lib/api";
import { confirmDialog } from "../lib/dialog";
import { useStore } from "../lib/store";
import type {
  DataSyncApplyResult,
  DataSyncPlan,
  DataSyncRequest,
  SyncEndpoint,
} from "../lib/bindings";
import { DialogShell } from "./DialogShell";
import {
  EndpointPicker,
  engineLabel,
  pickSourceEndpoint,
  pickTargetEndpoint,
} from "./EndpointPicker";

const INPUT_CLS =
  "h-7 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-800 placeholder:text-slate-400 focus:ring-2 focus:ring-accent/40 focus:outline-none";
const LABEL_CLS =
  "mb-1 block text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase";
const BTN_SECONDARY =
  "flex h-7 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50 disabled:opacity-50";
const BTN_PRIMARY =
  "flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover disabled:opacity-50";

const MAX_SHOWN_PER_GROUP = 200;

const GROUPS = [
  {
    key: "insert",
    label: "Insert",
    text: "text-emerald-600",
    badge: "bg-emerald-50 text-emerald-700",
  },
  {
    key: "update",
    label: "Update",
    text: "text-amber-600",
    badge: "bg-amber-50 text-amber-700",
  },
  {
    key: "delete",
    label: "Delete",
    text: "text-rose-600",
    badge: "bg-rose-50 text-rose-700",
  },
] as const;

function splitList(raw: string): string[] {
  return raw
    .split(",")
    .map((v) => v.trim())
    .filter(Boolean);
}

function parsePositive(raw: string, fallback: number): number {
  const n = Number.parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

function StatCard({
  label,
  value,
  className,
}: {
  label: string;
  value: string | number;
  className: string;
}) {
  return (
    <div className={`rounded-md border px-2 py-1.5 ${className}`}>
      <div className="text-[11px] text-slate-500">{label}</div>
      <div className="text-[15px] font-semibold tabular-nums">{value}</div>
    </div>
  );
}

export function DataSyncDialog({ onClose }: { onClose: () => void }) {
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
  const [table, setTable] = useState(() => selected?.table ?? "");
  const [keyColumns, setKeyColumns] = useState("");
  const [compareColumns, setCompareColumns] = useState("");
  const [sampleLimit, setSampleLimit] = useState("100000");
  const [maxStatements, setMaxStatements] = useState("2000");
  const [fullScan, setFullScan] = useState(false);

  const [plan, setPlan] = useState<DataSyncPlan | null>(null);
  const [applyResult, setApplyResult] = useState<DataSyncApplyResult | null>(
    null,
  );
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const buildRequest = (): DataSyncRequest | null => {
    if (!source || !target) {
      setError("Select a source and a target session first.");
      return null;
    }
    if (!table.trim()) {
      setError("Enter the table to synchronize.");
      return null;
    }
    return {
      source,
      target,
      table: table.trim(),
      key_columns: splitList(keyColumns),
      compare_columns: splitList(compareColumns),
      sample_limit: parsePositive(sampleLimit, 100000),
      max_statements: parsePositive(maxStatements, 2000),
      full_scan: fullScan,
    };
  };

  const runCompare = async () => {
    const request = buildRequest();
    if (!request) return;
    setLoading(true);
    setError(null);
    setPlan(null);
    setApplyResult(null);
    try {
      const result = await api.dataSyncPlan(request);
      setPlan(result);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setLoading(false);
    }
  };

  const onApply = async () => {
    if (!plan) return;
    const request = buildRequest();
    if (!request) return;
    const { to_insert, to_update, to_delete } = plan.summary;
    const targetName =
      sessions.find((s) => s.id === request.target.session_id)?.name ??
      request.target.session_id;
    const message =
      `Apply this data sync to "${targetName}"?\n\n` +
      `${to_insert} insert(s), ${to_update} update(s), ${to_delete} delete(s) ` +
      `(from the current preview — the diff is re-computed on the backend).`;
    if (
      !(await confirmDialog(message, { title: "Apply data sync", kind: "warning" }))
    )
      return;
    setApplying(true);
    setError(null);
    try {
      const result = await api.applyDataSync(request);
      setApplyResult(result);
      if (result.failed > 0) {
        toast(
          "error",
          `Data sync: ${result.inserted} inserted, ${result.updated} updated, ${result.deleted} deleted, ${result.failed} failed`,
        );
      } else {
        toast(
          "success",
          `Data sync applied: ${result.inserted} inserted, ${result.updated} updated, ${result.deleted} deleted in ${result.elapsed_ms} ms`,
        );
      }
    } catch (err) {
      setError(errorText(err));
      toast("error", errorText(err));
    } finally {
      setApplying(false);
    }
  };

  const summary = plan?.summary ?? null;

  return (
    <DialogShell
      title="Data Synchronization"
      width={900}
      onClose={onClose}
      footer={
        <>
          <button className={BTN_SECONDARY} onClick={onClose}>
            Close
          </button>
          <button
            className={BTN_SECONDARY}
            onClick={() => void runCompare()}
            disabled={loading || applying}
          >
            {loading ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Play size={12} />
            )}
            Compare
          </button>
          <button
            className={BTN_PRIMARY}
            onClick={() => void onApply()}
            disabled={!plan || applying}
            title="Apply the current form to the target (the diff is re-computed on the backend)"
          >
            {applying ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Play size={12} />
            )}
            Apply
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        <div className="grid grid-cols-2 gap-2">
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

        <div className="grid grid-cols-4 gap-2">
          <div className="col-span-2">
            <label className={LABEL_CLS}>Table</label>
            <input
              className={INPUT_CLS}
              value={table}
              placeholder="users"
              onChange={(e) => setTable(e.target.value)}
            />
          </div>
          <div>
            <label className={LABEL_CLS}>Key columns</label>
            <input
              className={INPUT_CLS}
              value={keyColumns}
              placeholder="id"
              onChange={(e) => setKeyColumns(e.target.value)}
            />
            <div className="mt-1 text-[11px] text-slate-500">
              empty = source table primary key
            </div>
          </div>
          <div>
            <label className={LABEL_CLS}>Compare columns</label>
            <input
              className={INPUT_CLS}
              value={compareColumns}
              placeholder="(all non-key)"
              onChange={(e) => setCompareColumns(e.target.value)}
            />
            <div className="mt-1 text-[11px] text-slate-500">
              empty = all non-key columns
            </div>
          </div>
        </div>

        <div className="grid grid-cols-4 items-start gap-2">
          <div>
            <label className={LABEL_CLS}>Sample limit</label>
            <input
              className={INPUT_CLS}
              type="number"
              min={1}
              value={sampleLimit}
              onChange={(e) => setSampleLimit(e.target.value)}
            />
          </div>
          <div>
            <label className={LABEL_CLS}>Max statements</label>
            <input
              className={INPUT_CLS}
              type="number"
              min={1}
              value={maxStatements}
              onChange={(e) => setMaxStatements(e.target.value)}
            />
          </div>
          <label className="mt-4 flex items-center gap-1.5 text-[13px] text-slate-700">
            <input
              type="checkbox"
              checked={fullScan}
              onChange={(e) => setFullScan(e.target.checked)}
            />
            Full scan (accurate counts)
          </label>
          {source && target && (
            <div className="mt-4 text-right text-[11px] text-slate-500">
              {engineLabel(
                sessions.find((s) => s.id === source.session_id)?.engine ??
                  "mysql",
              )}{" "}
              →{" "}
              {engineLabel(
                sessions.find((s) => s.id === target.session_id)?.engine ??
                  "mysql",
              )}
            </div>
          )}
        </div>

        {error && (
          <div className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-[12.5px] whitespace-pre-wrap text-rose-700">
            {error}
          </div>
        )}

        {loading && (
          <div className="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12.5px] text-slate-500">
            <Loader2 size={12} className="animate-spin" />
            Diffing rows…
          </div>
        )}

        {plan && summary && (
          <>
            <div className="grid grid-cols-4 gap-2">
              <StatCard
                label="Source rows"
                value={summary.source_rows.toLocaleString()}
                className="border-slate-200 bg-slate-50 text-slate-700"
              />
              <StatCard
                label="Target rows"
                value={summary.target_rows.toLocaleString()}
                className="border-slate-200 bg-slate-50 text-slate-700"
              />
              <StatCard
                label="Insert"
                value={summary.to_insert.toLocaleString()}
                className="border-emerald-200 bg-emerald-50 text-emerald-700"
              />
              <StatCard
                label="Update"
                value={summary.to_update.toLocaleString()}
                className="border-amber-200 bg-amber-50 text-amber-700"
              />
              <StatCard
                label="Delete"
                value={summary.to_delete.toLocaleString()}
                className="border-rose-200 bg-rose-50 text-rose-700"
              />
              <StatCard
                label="Unchanged"
                value={summary.unchanged.toLocaleString()}
                className="border-slate-200 bg-white text-slate-500"
              />
              <StatCard
                label="Truncated"
                value={summary.truncated ? "yes" : "no"}
                className={
                  summary.truncated
                    ? "border-amber-200 bg-amber-50 text-amber-700"
                    : "border-slate-200 bg-white text-slate-500"
                }
              />
              <StatCard
                label="Key columns"
                value={plan.key_columns.join(", ") || "—"}
                className="border-slate-200 bg-slate-50 text-slate-700"
              />
            </div>

            {plan.warnings.length > 0 && (
              <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-[12.5px] text-amber-700">
                <div className="flex items-center gap-1.5 font-medium">
                  <AlertTriangle size={12} className="shrink-0" />
                  Warnings
                </div>
                <ul className="mt-1 ml-5 list-disc space-y-0.5">
                  {plan.warnings.map((w, i) => (
                    <li key={i}>{w}</li>
                  ))}
                </ul>
              </div>
            )}

            <div className="space-y-2">
              {GROUPS.map((group) => {
                const statements = plan.statements.filter(
                  (s) => s.kind === group.key,
                );
                if (statements.length === 0) return null;
                const shown = statements.slice(0, MAX_SHOWN_PER_GROUP);
                return (
                  <div
                    key={group.key}
                    className="overflow-hidden rounded-md border border-slate-200 bg-white"
                  >
                    <div className="flex items-center gap-2 border-b border-slate-200/70 bg-sidebar px-2.5 py-1.5">
                      <span
                        className={`flex items-center gap-1 rounded px-1 py-px text-[10.5px] font-semibold ${group.badge}`}
                      >
                        {group.key === "insert" && <Plus size={10} />}
                        {group.key === "update" && <PenLine size={10} />}
                        {group.key === "delete" && <Minus size={10} />}
                        {group.label}
                      </span>
                      <span className="text-[11px] text-slate-500">
                        {statements.length} statement(s)
                      </span>
                      {statements.length > MAX_SHOWN_PER_GROUP && (
                        <span className="text-[11px] text-slate-500">
                          showing first {MAX_SHOWN_PER_GROUP}
                        </span>
                      )}
                    </div>
                    <pre className="custom-scrollbar max-h-40 overflow-auto p-3 font-code text-[12.5px] leading-5">
                      {shown.map((stmt, i) => (
                        <div key={i} className={group.text}>
                          {stmt.sql}
                        </div>
                      ))}
                    </pre>
                  </div>
                );
              })}
              {plan.statements.length === 0 && (
                <div className="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12.5px] text-slate-500">
                  No row differences found — source and target are identical
                  (within the sample limit).
                </div>
              )}
            </div>

            {applyResult && (
              <div
                className={`rounded-md border px-3 py-2 text-[12.5px] ${
                  applyResult.failed > 0
                    ? "border-rose-200 bg-rose-50 text-rose-700"
                    : "border-emerald-200 bg-emerald-50 text-emerald-700"
                }`}
              >
                <div className="font-medium">
                  Inserted {applyResult.inserted} · updated{" "}
                  {applyResult.updated} · deleted {applyResult.deleted} · failed{" "}
                  {applyResult.failed} · {applyResult.elapsed_ms} ms
                  {applyResult.truncated
                    ? " · truncated (statements beyond the cap were not applied)"
                    : ""}
                </div>
                {applyResult.errors.length > 0 && (
                  <ul className="mt-1 ml-4 list-disc space-y-0.5">
                    {applyResult.errors.map((e, i) => (
                      <li key={i} className="font-code whitespace-pre-wrap">
                        {e}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
          </>
        )}
      </div>
    </DialogShell>
  );
}
