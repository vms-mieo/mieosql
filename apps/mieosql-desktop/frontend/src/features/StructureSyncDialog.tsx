// Structure Synchronization: compare table schemas between two sessions and
// apply the generated migration script. F4 (Sync & Transfer) feature agent.

import { useState } from "react";
import type { ReactNode } from "react";
import {
  AlertTriangle,
  Check,
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
  StructureSyncPlan,
  SyncApplyResult,
  SyncEndpoint,
  SyncStatementKind,
} from "../lib/bindings";
import { DialogShell } from "./DialogShell";
import {
  EndpointPicker,
  engineLabel,
  pickSourceEndpoint,
  pickTargetEndpoint,
} from "./EndpointPicker";

type TableDiff = StructureSyncPlan["diffs"][number];
type ColumnDiff = TableDiff["columns"][number];

const INPUT_CLS =
  "h-7 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-800 placeholder:text-slate-400 focus:ring-2 focus:ring-accent/40 focus:outline-none";
const LABEL_CLS =
  "mb-1 block text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase";
const BTN_SECONDARY =
  "flex h-7 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50 disabled:opacity-50";
const BTN_PRIMARY =
  "flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover disabled:opacity-50";

const KIND_LABEL: Record<SyncStatementKind, string> = {
  create_table: "create table",
  drop_table: "drop table",
  add_column: "add column",
  drop_column: "drop column",
  alter_column: "alter column",
  add_index: "add index",
  drop_index: "drop index",
  insert: "insert",
  update: "update",
  delete: "delete",
};

const STATUS_META: Record<
  TableDiff["status"],
  { text: string; label: string; badge: string }
> = {
  added: {
    text: "text-emerald-600",
    label: "only in source",
    badge: "bg-emerald-50 text-emerald-700",
  },
  removed: {
    text: "text-rose-600",
    label: "only in target",
    badge: "bg-rose-50 text-rose-700",
  },
  changed: {
    text: "text-amber-600",
    label: "differs",
    badge: "bg-amber-50 text-amber-700",
  },
  same: {
    text: "text-slate-500",
    label: "in sync",
    badge: "bg-slate-900/[0.06] text-slate-600",
  },
};

function StatusIcon({ status }: { status: TableDiff["status"] }) {
  if (status === "added") return <Plus size={12} />;
  if (status === "removed") return <Minus size={12} />;
  if (status === "changed") return <PenLine size={12} />;
  return <Check size={12} />;
}

function columnDiffText(cd: ColumnDiff): { text: string; cls: string } {
  if (cd.change === "added") {
    return {
      text: `+ ${cd.name}${cd.source_type ? ` ${cd.source_type}` : ""}`,
      cls: "bg-emerald-50 px-1.5 py-px text-emerald-700",
    };
  }
  if (cd.change === "removed") {
    return {
      text: `- ${cd.name}${cd.target_type ? ` ${cd.target_type}` : ""}`,
      cls: "bg-rose-50 px-1.5 py-px text-rose-700",
    };
  }
  return {
    text: `~ ${cd.name}: ${cd.source_type ?? "?"} → ${cd.target_type ?? "?"}`,
    cls: "bg-amber-50 px-1.5 py-px text-amber-700",
  };
}

function fkLineCls(line: string): string {
  if (line.startsWith("+")) return "bg-emerald-50 px-1.5 py-px text-emerald-700";
  if (line.startsWith("-")) return "bg-rose-50 px-1.5 py-px text-rose-700";
  return "bg-amber-50 px-1.5 py-px text-amber-700";
}

function Chip({
  children,
  className,
}: {
  children: ReactNode;
  className: string;
}) {
  return (
    <span
      className={`rounded px-1.5 py-0.5 text-[10.5px] font-semibold ${className}`}
    >
      {children}
    </span>
  );
}

export function StructureSyncDialog({ onClose }: { onClose: () => void }) {
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
  const [tablesFilter, setTablesFilter] = useState("");
  const [includeIndexes, setIncludeIndexes] = useState(true);
  const [plan, setPlan] = useState<StructureSyncPlan | null>(null);
  const [applyResult, setApplyResult] = useState<SyncApplyResult | null>(null);
  const [showSame, setShowSame] = useState(false);
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const sameScope =
    !!source &&
    !!target &&
    source.session_id === target.session_id &&
    (source.scope.database ?? "") === (target.scope.database ?? "") &&
    (source.scope.schema ?? "") === (target.scope.schema ?? "");

  const sourceSession = source
    ? (sessions.find((s) => s.id === source.session_id) ?? null)
    : null;
  const targetSession = target
    ? (sessions.find((s) => s.id === target.session_id) ?? null)
    : null;

  /** Endpoints changed -> any previous plan/result no longer applies. */
  const changeSource = (endpoint: SyncEndpoint | null) => {
    setSource(endpoint);
    setPlan(null);
    setApplyResult(null);
  };
  const changeTarget = (endpoint: SyncEndpoint | null) => {
    setTarget(endpoint);
    setPlan(null);
    setApplyResult(null);
  };

  const runCompare = async () => {
    if (!source || !target) {
      setError("Select a source and a target session first.");
      return;
    }
    setLoading(true);
    setError(null);
    setPlan(null);
    setApplyResult(null);
    try {
      const result = await api.structureSyncPlan({
        source,
        target,
        tables: tablesFilter
          .split(",")
          .map((t) => t.trim())
          .filter(Boolean),
        include_indexes: includeIndexes,
      });
      setPlan(result);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setLoading(false);
    }
  };

  const onApply = async () => {
    if (!plan || !target || plan.statements.length === 0) return;
    const destructive = plan.statements.filter((s) => s.destructive).length;
    const message =
      `Apply ${plan.statements.length} statement(s) to the target session ` +
      `"${targetSession?.name ?? target.session_id}"?` +
      (destructive > 0
        ? `\n\n⚠ ${destructive} destructive statement(s) will drop data or objects.`
        : "");
    if (
      !(await confirmDialog(message, { title: "Apply structure sync", kind: "warning" }))
    )
      return;
    setApplying(true);
    setError(null);
    try {
      const result = await api.applySyncStatements(
        target.session_id,
        plan.statements.map((s) => s.sql),
      );
      setApplyResult(result);
      if (result.failed > 0) {
        toast(
          "error",
          `Structure sync applied ${result.applied} of ${
            result.applied + result.failed
          } statement(s) before failing — see the dialog for details`,
        );
      } else {
        toast(
          "success",
          `Applied ${result.applied} statement(s) in ${result.elapsed_ms} ms`,
        );
      }
    } catch (err) {
      setError(errorText(err));
      toast("error", errorText(err));
    } finally {
      setApplying(false);
    }
  };

  const changedDiffs = plan?.diffs.filter((d) => d.status !== "same") ?? [];
  const sameDiffs = plan?.diffs.filter((d) => d.status === "same") ?? [];

  return (
    <DialogShell
      title="Structure Synchronization"
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
            disabled={!plan || plan.statements.length === 0 || applying}
            title={
              plan && plan.statements.length > 0
                ? `Apply ${plan.statements.length} statement(s) to the target`
                : "Nothing to apply"
            }
          >
            {applying ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Play size={12} />
            )}
            Apply on target
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        <div className="grid grid-cols-2 gap-2">
          <EndpointPicker
            label="Source"
            value={source}
            onChange={changeSource}
            sessions={sessions}
          />
          <EndpointPicker
            label="Target"
            value={target}
            onChange={changeTarget}
            sessions={sessions}
          />
        </div>

        <div className="flex flex-wrap items-end gap-3">
          <div className="min-w-[240px] flex-1">
            <label className={LABEL_CLS}>
              Tables filter (comma separated, empty = all)
            </label>
            <input
              className={INPUT_CLS}
              value={tablesFilter}
              placeholder="users, orders"
              onChange={(e) => setTablesFilter(e.target.value)}
            />
          </div>
          <label className="flex items-center gap-1.5 pb-1 text-[13px] text-slate-700">
            <input
              type="checkbox"
              checked={includeIndexes}
              onChange={(e) => setIncludeIndexes(e.target.checked)}
            />
            Include indexes
          </label>
        </div>

        {sameScope && (
          <div className="flex items-center gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-[12.5px] text-amber-700">
            <AlertTriangle size={13} className="shrink-0" />
            Source and target point to the same session and scope — the diff
            will be empty.
          </div>
        )}

        {error && (
          <div className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-[12.5px] whitespace-pre-wrap text-rose-700">
            {error}
          </div>
        )}

        {loading && (
          <div className="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12.5px] text-slate-500">
            <Loader2 size={12} className="animate-spin" />
            Comparing schemas…
          </div>
        )}

        {plan && (
          <>
            <div className="flex flex-wrap items-center gap-1.5">
              <Chip className="bg-slate-900/[0.06] text-slate-600">
                {plan.diffs.length} table(s) compared
              </Chip>
              <Chip className="bg-emerald-50 text-emerald-700">
                +{plan.summary.tables_added} added
              </Chip>
              <Chip className="bg-rose-50 text-rose-700">
                −{plan.summary.tables_removed} removed
              </Chip>
              <Chip className="bg-amber-50 text-amber-700">
                ~{plan.summary.tables_changed} changed
              </Chip>
              <Chip className="bg-slate-900/[0.06] text-slate-600">
                {plan.summary.statements} statement(s)
              </Chip>
              {sourceSession && targetSession && (
                <span className="ml-1 text-[11px] text-slate-500">
                  {engineLabel(sourceSession.engine)} →{" "}
                  {engineLabel(targetSession.engine)}
                </span>
              )}
            </div>

            {(!plan.same_engine || plan.warnings.length > 0) && (
              <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-[12.5px] text-amber-700">
                <div className="flex items-center gap-1.5 font-medium">
                  <AlertTriangle size={12} className="shrink-0" />
                  {!plan.same_engine
                    ? "Cross-engine sync — the generated SQL is best-effort and may need review."
                    : "Warnings"}
                </div>
                {plan.warnings.length > 0 && (
                  <ul className="mt-1 ml-5 list-disc space-y-0.5">
                    {plan.warnings.map((w, i) => (
                      <li key={i}>{w}</li>
                    ))}
                  </ul>
                )}
              </div>
            )}

            <div>
              <div className="mb-1 flex items-center justify-between">
                <span className="text-[13px] font-semibold text-slate-900">
                  Table diffs
                </span>
                {sameDiffs.length > 0 && (
                  <button
                    className="text-[12px] text-accent hover:underline"
                    onClick={() => setShowSame((v) => !v)}
                  >
                    {showSame ? "Hide" : "Show"} {sameDiffs.length} identical
                    table(s)
                  </button>
                )}
              </div>
              <div className="space-y-1">
                {changedDiffs.length === 0 && !showSame && (
                  <div className="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12.5px] text-slate-500">
                    No structural differences found.
                  </div>
                )}
                {changedDiffs.map((diff) => (
                  <TableDiffCard key={diff.table} diff={diff} />
                ))}
                {showSame &&
                  sameDiffs.map((diff) => (
                    <div
                      key={diff.table}
                      className="flex items-center gap-2 rounded-md border border-slate-200/70 bg-white px-2.5 py-1.5 text-[12.5px] text-slate-500"
                    >
                      <Check size={11} />
                      <span className="font-code">{diff.table}</span>
                      <span className="text-[11px]">in sync</span>
                    </div>
                  ))}
              </div>
            </div>

            <div>
              <div className="mb-1 flex items-center justify-between">
                <span className="text-[13px] font-semibold text-slate-900">
                  Statements ({plan.statements.length})
                </span>
                {plan.statements.length > 0 && (
                  <span className="text-[11px] text-slate-500">
                    destructive statements are shown in red
                  </span>
                )}
              </div>
              {plan.statements.length === 0 ? (
                <div className="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12.5px] text-slate-500">
                  Nothing to apply — the target already matches the source.
                </div>
              ) : (
                <pre className="custom-scrollbar max-h-64 overflow-auto rounded-md border border-slate-200 bg-slate-900 p-3 font-code text-[12.5px] leading-5">
                  {plan.statements.map((stmt, i) => (
                    <div key={i} className={stmt.destructive ? "text-rose-400" : "text-slate-200"}>
                      <span className="text-slate-500">
                        {`-- [${KIND_LABEL[stmt.kind]}] ${stmt.table}${
                          stmt.destructive ? " (destructive)" : ""
                        }`}
                      </span>
                      {"\n"}
                      {stmt.sql}
                      {"\n"}
                    </div>
                  ))}
                </pre>
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
                  Applied {applyResult.applied} · failed {applyResult.failed} ·{" "}
                  {applyResult.elapsed_ms} ms
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

function TableDiffCard({ diff }: { diff: TableDiff }) {
  const meta = STATUS_META[diff.status];
  const hasBody =
    diff.columns.length > 0 ||
    diff.indexes_added.length > 0 ||
    diff.indexes_removed.length > 0 ||
    diff.fk_report.length > 0;

  return (
    <div className="overflow-hidden rounded-md border border-slate-200 bg-white">
      <div className="flex items-center gap-2 border-b border-slate-200/70 bg-sidebar px-2.5 py-1.5">
        <span className={`flex items-center gap-1 ${meta.text}`}>
          <StatusIcon status={diff.status} />
        </span>
        <span className="font-code text-[12.5px] font-medium text-slate-900">
          {diff.table}
        </span>
        <span
          className={`rounded px-1 py-px text-[10.5px] font-semibold ${meta.badge}`}
        >
          {meta.label}
        </span>
        {diff.columns.length > 0 && (
          <span className="text-[11px] text-slate-500">
            {diff.columns.length} column change(s)
          </span>
        )}
        {diff.indexes_added.length + diff.indexes_removed.length > 0 && (
          <span className="text-[11px] text-slate-500">
            {diff.indexes_added.length + diff.indexes_removed.length} index
            change(s)
          </span>
        )}
      </div>
      {hasBody && (
        <div className="space-y-px p-1.5 font-code text-[12px]">
          {diff.columns.map((cd) => {
            const { text, cls } = columnDiffText(cd);
            return (
              <div key={cd.name} className={cls}>
                {text}
              </div>
            );
          })}
          {diff.indexes_added.map((idx) => (
            <div
              key={`+${idx.name}`}
              className="bg-emerald-50 px-1.5 py-px text-emerald-700"
            >
              + index {idx.name}
              {idx.is_unique ? " (unique)" : ""} ({idx.columns.join(", ")})
            </div>
          ))}
          {diff.indexes_removed.map((idx) => (
            <div
              key={`-${idx.name}`}
              className="bg-rose-50 px-1.5 py-px text-rose-700"
            >
              - index {idx.name} ({idx.columns.join(", ")})
            </div>
          ))}
          {diff.fk_report.map((line, i) => (
            <div key={`fk${i}`} className={fkLineCls(line)}>
              {line}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
