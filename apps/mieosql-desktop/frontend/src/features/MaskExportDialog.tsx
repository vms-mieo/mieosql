// Masked export dialog (feature pack v1 · F3 — Data Tools).
//
// Exports the active query result with per-column masking rules. The backend
// re-runs the recorded SQL and streams every batch through the rules, so the
// file never contains the original values.
//
// Owned by the Data Tools agent — this file is the only one edited here.

import { useEffect, useState } from "react";
import { AlertTriangle, FileDown, Loader2, ShieldCheck } from "lucide-react";
import { save } from "@tauri-apps/plugin-dialog";

import type {
  ColumnMeta,
  ExportFormat,
  MaskColumnRule,
  MaskStrategy,
  Value,
} from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { formatBytes, plainText } from "../lib/format";
import { EXPORT_FORMATS, exportFormatLabel, useStore } from "../lib/store";
import { DialogShell } from "./DialogShell";

// ---------------------------------------------------------------------------
// Shared styles (feature-pack dialog conventions)
// ---------------------------------------------------------------------------

const inputCls =
  "h-7 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-700 focus:ring-2 focus:ring-accent/40 focus:outline-none disabled:bg-slate-50 disabled:text-slate-400";
const miniInputCls =
  "h-6 rounded-md border border-slate-300 bg-white px-1.5 text-[11.5px] text-slate-700 focus:ring-2 focus:ring-accent/40 focus:outline-none";
const labelCls = "mb-1 block text-[12px] font-medium text-slate-500";
const btnPrimary =
  "flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50";
const btnSecondary =
  "flex h-7 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50";

const FORMAT_EXT: Record<ExportFormat, string> = {
  csv: "csv",
  tsv: "tsv",
  json: "json",
  json_lines: "jsonl",
  sql: "sql",
  xlsx: "xlsx",
};

// ---------------------------------------------------------------------------
// Masking rule model (mirrors `MaskStrategy` on the backend)
// ---------------------------------------------------------------------------

type MaskKind =
  | "none"
  | "null"
  | "redact"
  | "fixed"
  | "email"
  | "phone"
  | "name"
  | "company"
  | "city"
  | "address"
  | "random_string"
  | "hash"
  | "random_int"
  | "random_decimal"
  | "date_recent";

const MASK_KINDS: { value: MaskKind; label: string }[] = [
  { value: "none", label: "None" },
  { value: "null", label: "Null" },
  { value: "redact", label: "Redact" },
  { value: "fixed", label: "Fixed" },
  { value: "email", label: "Email" },
  { value: "phone", label: "Phone" },
  { value: "name", label: "Name" },
  { value: "company", label: "Company" },
  { value: "city", label: "City" },
  { value: "address", label: "Address" },
  { value: "random_string", label: "Random string" },
  { value: "hash", label: "Hash" },
  { value: "random_int", label: "Random int" },
  { value: "random_decimal", label: "Random decimal" },
  { value: "date_recent", label: "Date (recent)" },
];

type MaskRuleState = {
  kind: MaskKind;
  keepStart: string;
  keepEnd: string;
  fixedValue: string;
  minLen: string;
  maxLen: string;
  salt: string;
  min: string;
  max: string;
  scale: string;
  days: string;
};

function emptyMaskRule(): MaskRuleState {
  return {
    kind: "none",
    keepStart: "4",
    keepEnd: "0",
    fixedValue: "",
    minLen: "8",
    maxLen: "16",
    salt: "",
    min: "1",
    max: "100000",
    scale: "2",
    days: "90",
  };
}

function intOr(raw: string, fallback: number): number {
  const n = Number.parseInt(raw, 10);
  return Number.isFinite(n) ? n : fallback;
}

function numOr(raw: string, fallback: number): number {
  const n = Number(raw);
  return Number.isFinite(n) ? n : fallback;
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n));
}

/** Parse the optional seed input; `null` means "time based". */
function parseSeed(raw: string): { ok: true; value: number | null } | { ok: false } {
  const trimmed = raw.trim();
  if (!trimmed) return { ok: true, value: null };
  const n = Number(trimmed);
  if (!Number.isSafeInteger(n) || n < 0) return { ok: false };
  return { ok: true, value: n };
}

/** `null` = "None": no rule for this column. */
function buildMaskStrategy(rule: MaskRuleState): MaskStrategy | null {
  switch (rule.kind) {
    case "none":
      return null;
    case "null":
      return { kind: "null" };
    case "redact":
      return {
        kind: "redact",
        keep_start: Math.max(0, intOr(rule.keepStart, 4)),
        keep_end: Math.max(0, intOr(rule.keepEnd, 0)),
      };
    case "fixed":
      return { kind: "fixed", value: rule.fixedValue };
    case "email":
      return { kind: "email" };
    case "phone":
      return { kind: "phone" };
    case "name":
      return { kind: "name" };
    case "company":
      return { kind: "company" };
    case "city":
      return { kind: "city" };
    case "address":
      return { kind: "address" };
    case "random_string": {
      const min = Math.max(0, intOr(rule.minLen, 8));
      return {
        kind: "random_string",
        min_len: min,
        max_len: Math.max(min, intOr(rule.maxLen, 16)),
      };
    }
    case "hash": {
      const salt = rule.salt.trim();
      return { kind: "hash", salt: salt.length > 0 ? salt : null };
    }
    case "random_int": {
      const min = intOr(rule.min, 1);
      return { kind: "random_int", min, max: Math.max(min, intOr(rule.max, min)) };
    }
    case "random_decimal": {
      const min = numOr(rule.min, 0);
      return {
        kind: "random_decimal",
        min,
        max: Math.max(min, numOr(rule.max, 100)),
        scale: clamp(intOr(rule.scale, 2), 0, 10),
      };
    }
    case "date_recent":
      return { kind: "date_recent", days: Math.max(1, intOr(rule.days, 90)) };
  }
}

function collectRules(
  columns: ColumnMeta[],
  ruleMap: Record<string, MaskRuleState>,
): MaskColumnRule[] {
  const out: MaskColumnRule[] = [];
  for (const col of columns) {
    const rule = ruleMap[col.name];
    if (!rule || rule.kind === "none") continue;
    const strategy = buildMaskStrategy(rule);
    if (strategy) out.push({ column: col.name, strategy });
  }
  return out;
}

// ---------------------------------------------------------------------------
// Rule parameter editors
// ---------------------------------------------------------------------------

function ParamInput({
  label,
  value,
  onChange,
  width = "w-[64px]",
}: {
  label: string;
  value: string;
  onChange: (next: string) => void;
  width?: string;
}) {
  return (
    <label className="flex items-center gap-1 text-[11px] text-slate-500">
      {label}
      <input
        className={`${miniInputCls} ${width}`}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}

const hintCls = "text-[11px] text-slate-500";

function MaskRuleParams({
  rule,
  onChange,
}: {
  rule: MaskRuleState;
  onChange: (patch: Partial<MaskRuleState>) => void;
}) {
  const wrap = "flex flex-wrap items-center gap-x-2 gap-y-1";
  switch (rule.kind) {
    case "none":
      return <span className={hintCls}>column passes through unchanged</span>;
    case "null":
      return <span className={hintCls}>always NULL</span>;
    case "redact":
      return (
        <div className={wrap}>
          <ParamInput
            label="keep start"
            value={rule.keepStart}
            onChange={(v) => onChange({ keepStart: v })}
            width="w-[40px]"
          />
          <ParamInput
            label="keep end"
            value={rule.keepEnd}
            onChange={(v) => onChange({ keepEnd: v })}
            width="w-[40px]"
          />
          <span className={hintCls}>rest becomes *</span>
        </div>
      );
    case "fixed":
      return (
        <div className={wrap}>
          <input
            className={`${miniInputCls} min-w-[180px] flex-1`}
            placeholder="replacement value"
            value={rule.fixedValue}
            onChange={(e) => onChange({ fixedValue: e.target.value })}
          />
        </div>
      );
    case "email":
      return <span className={hintCls}>stable fake e-mail per value</span>;
    case "phone":
      return <span className={hintCls}>stable fake phone per value</span>;
    case "name":
      return <span className={hintCls}>stable fake name per value</span>;
    case "company":
      return <span className={hintCls}>stable fake company per value</span>;
    case "city":
      return <span className={hintCls}>stable fake city per value</span>;
    case "address":
      return <span className={hintCls}>stable fake address per value</span>;
    case "random_string":
      return (
        <div className={wrap}>
          <ParamInput
            label="min len"
            value={rule.minLen}
            onChange={(v) => onChange({ minLen: v })}
            width="w-[44px]"
          />
          <ParamInput
            label="max len"
            value={rule.maxLen}
            onChange={(v) => onChange({ maxLen: v })}
            width="w-[44px]"
          />
        </div>
      );
    case "hash":
      return (
        <div className={wrap}>
          <ParamInput
            label="salt"
            value={rule.salt}
            onChange={(v) => onChange({ salt: v })}
            width="w-[140px]"
          />
          <span className={hintCls}>deterministic pseudonym</span>
        </div>
      );
    case "random_int":
      return (
        <div className={wrap}>
          <ParamInput label="min" value={rule.min} onChange={(v) => onChange({ min: v })} width="w-[74px]" />
          <ParamInput label="max" value={rule.max} onChange={(v) => onChange({ max: v })} width="w-[74px]" />
        </div>
      );
    case "random_decimal":
      return (
        <div className={wrap}>
          <ParamInput label="min" value={rule.min} onChange={(v) => onChange({ min: v })} width="w-[70px]" />
          <ParamInput label="max" value={rule.max} onChange={(v) => onChange({ max: v })} width="w-[70px]" />
          <ParamInput label="scale" value={rule.scale} onChange={(v) => onChange({ scale: v })} width="w-[40px]" />
        </div>
      );
    case "date_recent":
      return (
        <div className={wrap}>
          <ParamInput label="last days" value={rule.days} onChange={(v) => onChange({ days: v })} width="w-[52px]" />
        </div>
      );
  }
}

function MaskRuleRow({
  col,
  rule,
  onChange,
}: {
  col: ColumnMeta;
  rule: MaskRuleState;
  onChange: (patch: Partial<MaskRuleState>) => void;
}) {
  const active = rule.kind !== "none";
  return (
    <tr className={active ? "bg-accent/10" : undefined}>
      <td className="border-b border-slate-200/70 px-2 py-1.5 align-top">
        <div className="font-code text-[12px] font-medium text-slate-700">{col.name}</div>
        <div className="text-[11px] text-slate-500">{col.native_type}</div>
      </td>
      <td className="border-b border-slate-200/70 px-2 py-1.5 align-top">
        <select
          className={inputCls}
          value={rule.kind}
          onChange={(e) => onChange({ kind: e.target.value as MaskKind })}
        >
          {MASK_KINDS.map((k) => (
            <option key={k.value} value={k.value}>
              {k.label}
            </option>
          ))}
        </select>
      </td>
      <td className="border-b border-slate-200/70 px-2 py-1.5 align-top">
        <MaskRuleParams rule={rule} onChange={onChange} />
      </td>
    </tr>
  );
}

function PreviewCell({ value }: { value: Value | undefined }) {
  if (!value || value.t === "null") {
    return <span className="text-[11px] text-slate-400 italic">NULL</span>;
  }
  const text = plainText(value);
  return (
    <span title={text.length > 60 ? text.slice(0, 400) : undefined}>
      {text.length > 60 ? `${text.slice(0, 60)}…` : text}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Dialog
// ---------------------------------------------------------------------------

export function MaskExportDialog({ onClose }: { onClose: () => void }) {
  const tabs = useStore((s) => s.tabs);
  const activeTabId = useStore((s) => s.activeTabId);
  const toast = useStore((s) => s.toast);

  const [ruleMap, setRuleMap] = useState<Record<string, MaskRuleState>>({});
  const [format, setFormat] = useState<ExportFormat>("csv");
  const [seed, setSeed] = useState("");
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [unmatched, setUnmatched] = useState<string[]>([]);
  const [lastPath, setLastPath] = useState<string | null>(null);

  const tab = tabs.find((t) => t.id === activeTabId) ?? null;
  const result = tab ? (tab.results[tab.activeResult] ?? null) : null;
  const columnKey = result ? result.columns.map((c) => c.name).join("\u0000") : "";

  // Reset the rules whenever the result shape changes (new query / result tab).
  useEffect(() => {
    const next: Record<string, MaskRuleState> = {};
    if (result) for (const col of result.columns) next[col.name] = emptyMaskRule();
    setRuleMap(next);
    setUnmatched([]);
    setError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [columnKey]);

  if (!tab || !result || !result.sql) {
    return (
      <DialogShell title="Export with Data Masking" width={780} onClose={onClose}>
        <div className="flex flex-col items-center gap-3 p-10 text-center">
          <ShieldCheck size={30} strokeWidth={1.6} className="text-slate-400" />
          <div className="text-[15px] font-semibold text-slate-900">
            No result to export
          </div>
          <p className="max-w-[400px] text-[12.5px] leading-relaxed text-slate-500">
            Run a query first, select its result tab, then reopen this dialog.
            Masking re-runs the recorded SQL and applies your rules while the rows
            are streamed to the file.
          </p>
          <button className={btnSecondary} onClick={onClose}>
            Close
          </button>
        </div>
      </DialogShell>
    );
  }

  const sessionId = tab.sessionId;
  const running = result.status === "running";
  const previewRows = result.rows.slice(0, 3);
  const rules = collectRules(result.columns, ruleMap);
  const maskedCount = rules.length;

  const onExport = async () => {
    if (!sessionId) {
      setError("This tab has no session attached.");
      return;
    }
    if (rules.length === 0) {
      setError("Choose a masking strategy for at least one column.");
      return;
    }
    const parsedSeed = parseSeed(seed);
    if (!parsedSeed.ok) {
      setError("Seed must be a non-negative integer.");
      return;
    }

    const ext = FORMAT_EXT[format];
    const label = exportFormatLabel(format);
    const base = (tab.kind === "table" ? (tab.table ?? "result") : tab.title)
      .replace(/[^\w.-]+/g, "_")
      .slice(0, 60);
    const stamp = new Date()
      .toISOString()
      .slice(0, 19)
      .replace(/[:T]/g, "-");

    let path: string | null = null;
    try {
      path = await save({
        title: `Export ${label} — masked`,
        defaultPath: `mieosql-masked-${base}-${stamp}.${ext}`,
        filters: [{ name: label, extensions: [ext] }],
      });
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!path) return; // cancelled

    setExporting(true);
    setError(null);
    setUnmatched([]);
    try {
      const res = await api.exportResultMasked(sessionId, {
        sql: result.sql,
        format,
        path,
        table: tab.kind === "table" ? (tab.table ?? undefined) : undefined,
        rules,
        seed: parsedSeed.value,
      });
      setUnmatched(res.unmatched_rules);
      setLastPath(res.stats.path);
      toast(
        "success",
        `Masked export: ${res.stats.rows.toLocaleString()} rows · ` +
          `${res.masked_columns} column(s) masked · ${formatBytes(res.stats.bytes)} ` +
          `in ${res.stats.elapsed_ms} ms\n${res.stats.path}`,
      );
    } catch (err) {
      setError(errorText(err));
      toast("error", errorText(err));
    } finally {
      setExporting(false);
    }
  };

  return (
    <DialogShell
      title="Export with Data Masking"
      width={840}
      onClose={onClose}
      footer={
        <>
          {maskedCount === 0 ? (
            <span className="mr-auto flex items-center gap-1 text-[11px] text-amber-600">
              <AlertTriangle size={12} strokeWidth={1.8} />
              Choose a strategy for at least one column
            </span>
          ) : (
            <span className="mr-auto truncate text-[11px] text-slate-500" title={lastPath ?? undefined}>
              {lastPath
                ? `Saved: ${lastPath}`
                : `${maskedCount} of ${result.columns.length} column(s) will be masked`}
            </span>
          )}
          <button className={btnSecondary} onClick={onClose} disabled={exporting}>
            Cancel
          </button>
          <button
            className={btnPrimary}
            onClick={() => void onExport()}
            disabled={exporting || maskedCount === 0 || running || !sessionId}
            title={running ? "Wait for the query to finish" : undefined}
          >
            {exporting ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <FileDown size={13} strokeWidth={1.8} />
            )}
            Export…
          </button>
        </>
      }
    >
      {/* --------------------------------------------------------- source */}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-slate-200/70 bg-sidebar px-4 py-2 text-[12px] text-slate-600">
        <span className="font-semibold text-slate-900">{tab.title}</span>
        <span>
          Result {tab.activeResult + 1} · {result.rows.length.toLocaleString()} rows
          buffered · {result.columns.length} columns
        </span>
        {result.truncated && <span className="text-amber-600">result truncated</span>}
        {running && <span className="text-accent">query still running…</span>}
        <code
          className="max-w-full truncate rounded bg-white px-1.5 py-0.5 font-code text-[11.5px] text-slate-600 ring-1 ring-slate-200"
          title={result.sql}
        >
          {result.sql.length > 140 ? `${result.sql.slice(0, 140)}…` : result.sql}
        </code>
      </div>

      {/* -------------------------------------------------------- preview */}
      <div className="p-4">
        <div className="mb-2 text-[13px] font-semibold text-slate-900">
          Result preview{" "}
          <span className="text-[11px] font-normal text-slate-500">
            (first {previewRows.length} row(s))
          </span>
        </div>
        <div className="custom-scrollbar overflow-auto rounded-md border border-slate-200 bg-white">
          <table className="border-collapse text-left text-[12.5px]">
            <thead className="bg-sidebar">
              <tr>
                {result.columns.map((c) => (
                  <th
                    key={c.name}
                    className="border-b border-slate-200/70 px-2 py-1.5 text-[11.5px] font-semibold whitespace-nowrap text-slate-500"
                  >
                    {c.name}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {previewRows.map((row, i) => (
                <tr key={i} className="hover:bg-slate-900/[0.03]">
                  {result.columns.map((c, j) => (
                    <td
                      key={c.name}
                      className="max-w-[220px] truncate border-b border-slate-200/70 px-2 py-1 whitespace-nowrap text-slate-700"
                    >
                      <PreviewCell value={row[j]} />
                    </td>
                  ))}
                </tr>
              ))}
              {previewRows.length === 0 && (
                <tr>
                  <td
                    colSpan={Math.max(1, result.columns.length)}
                    className="px-2 py-2 text-[12px] text-slate-500"
                  >
                    No rows buffered for this result.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>

      {/* ---------------------------------------------------------- rules */}
      <div className="px-4 pb-4">
        <div className="mb-2 flex items-center justify-between">
          <span className="text-[13px] font-semibold text-slate-900">Column rules</span>
          <span className="text-[11px] text-slate-500">
            matched case-insensitively against the result columns
          </span>
        </div>
        <div className="overflow-hidden rounded-md border border-slate-200">
          <table className="w-full border-collapse text-left text-[12.5px]">
            <thead className="bg-sidebar">
              <tr className="text-slate-500">
                <th className="w-[220px] border-b border-slate-200/70 px-2 py-1.5 text-[11.5px] font-semibold">
                  Column
                </th>
                <th className="w-[150px] border-b border-slate-200/70 px-2 py-1.5 text-[11.5px] font-semibold">
                  Strategy
                </th>
                <th className="border-b border-slate-200/70 px-2 py-1.5 text-[11.5px] font-semibold">
                  Parameters
                </th>
              </tr>
            </thead>
            <tbody>
              {result.columns.map((col) => (
                <MaskRuleRow
                  key={col.name}
                  col={col}
                  rule={ruleMap[col.name] ?? emptyMaskRule()}
                  onChange={(patch) =>
                    setRuleMap((prev) => ({
                      ...prev,
                      [col.name]: { ...(prev[col.name] ?? emptyMaskRule()), ...patch },
                    }))
                  }
                />
              ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* -------------------------------------------------------- options */}
      <div className="grid grid-cols-2 gap-3 border-t border-slate-200/70 p-4">
        <div>
          <label className={labelCls}>Export format</label>
          <select
            className={inputCls}
            value={format}
            onChange={(e) => setFormat(e.target.value as ExportFormat)}
            disabled={exporting}
          >
            {EXPORT_FORMATS.map((f) => (
              <option key={f} value={f}>
                {exportFormatLabel(f)}
              </option>
            ))}
          </select>
        </div>
        <div>
          <label className={labelCls}>Seed (optional)</label>
          <input
            className={inputCls}
            value={seed}
            placeholder="time based — set for reproducible fakes"
            onChange={(e) => setSeed(e.target.value)}
            disabled={exporting}
          />
        </div>
      </div>

      {/* ------------------------------------------------------- warnings */}
      {(error || unmatched.length > 0 || lastPath) && (
        <div className="space-y-2 border-t border-slate-200/70 p-4">
          {error && (
            <div className="flex items-start gap-1.5 rounded-md border border-rose-200 bg-rose-50 p-2.5 text-[12.5px] whitespace-pre-wrap text-rose-600">
              <AlertTriangle size={13} strokeWidth={1.8} className="mt-0.5 shrink-0" />
              <span>{error}</span>
            </div>
          )}
          {unmatched.length > 0 && (
            <div className="rounded-md border border-amber-200 bg-amber-50 p-2.5 text-[12.5px] text-amber-700">
              <div className="flex items-center gap-1 font-medium">
                <AlertTriangle size={12} strokeWidth={1.8} />
                {unmatched.length} rule(s) did not match any result column
              </div>
              <ul className="mt-1 list-inside list-disc text-[11.5px]">
                {unmatched.map((name) => (
                  <li key={name}>{name}</li>
                ))}
              </ul>
            </div>
          )}
          {lastPath && !error && (
            <div className="rounded-md border border-emerald-200 bg-emerald-50 px-2.5 py-2 text-[12.5px] text-emerald-700">
              Written to <span className="font-code text-[11.5px]">{lastPath}</span>
            </div>
          )}
        </div>
      )}
    </DialogShell>
  );
}
