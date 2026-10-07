// Data Generation dialog (feature pack v1 · F3 — Data Tools).
//
// Synthesizes test rows for a single table: every column gets a strategy
// (Auto by default) with inline parameters. `Preview` renders the INSERT SQL
// without touching the database; `Execute` runs it inside a transaction.
//
// Owned by the Data Tools agent — this file is the only one edited here.

import { useEffect, useState } from "react";
import {
  AlertTriangle,
  Eye,
  Loader2,
  Play,
  RefreshCw,
  RotateCcw,
} from "lucide-react";

import type {
  ColumnDef,
  GenColumnRule,
  GenMode,
  GenRequest,
  GenResult,
  GenStrategy,
  ObjectRef,
  Scope,
} from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { useStore } from "../lib/store";
import { DialogShell } from "./DialogShell";

// ---------------------------------------------------------------------------
// Shared styles (feature-pack dialog conventions)
// ---------------------------------------------------------------------------

const inputCls =
  "w-full rounded border border-slate-300 bg-white px-2 py-1 text-[11px] text-slate-700 focus:border-sky-400 focus:outline-none disabled:bg-slate-50 disabled:text-slate-400";
const miniInputCls =
  "rounded border border-slate-300 bg-white px-1 py-0.5 text-[11px] text-slate-700 focus:border-sky-400 focus:outline-none";
const labelCls = "mb-0.5 block text-[11px] font-medium text-slate-500";
const btnPrimary =
  "flex items-center gap-1.5 rounded bg-sky-600 px-3 py-1.5 text-[11px] font-medium text-white hover:bg-sky-700 disabled:cursor-not-allowed disabled:opacity-50";
const btnSecondary =
  "flex items-center gap-1.5 rounded border border-slate-300 bg-white px-3 py-1.5 text-[11px] text-slate-700 hover:bg-slate-100 disabled:cursor-not-allowed disabled:opacity-50";

// ---------------------------------------------------------------------------
// Column strategy model (mirrors `GenStrategy` on the backend)
// ---------------------------------------------------------------------------

type GenKind =
  | "auto"
  | "null"
  | "sequential"
  | "random_int"
  | "random_decimal"
  | "random_string"
  | "lorem"
  | "email"
  | "full_name"
  | "phone"
  | "company"
  | "city"
  | "country"
  | "address"
  | "bool"
  | "date_recent"
  | "date_time_recent"
  | "uuid"
  | "pattern"
  | "pick"
  | "constant";

const GEN_KINDS: { value: GenKind; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "null", label: "Null" },
  { value: "sequential", label: "Sequential" },
  { value: "random_int", label: "Random int" },
  { value: "random_decimal", label: "Random decimal" },
  { value: "random_string", label: "Random string" },
  { value: "lorem", label: "Lorem" },
  { value: "email", label: "Email" },
  { value: "full_name", label: "Full name" },
  { value: "phone", label: "Phone" },
  { value: "company", label: "Company" },
  { value: "city", label: "City" },
  { value: "country", label: "Country" },
  { value: "address", label: "Address" },
  { value: "bool", label: "Bool" },
  { value: "date_recent", label: "Date (recent)" },
  { value: "date_time_recent", label: "DateTime (recent)" },
  { value: "uuid", label: "UUID" },
  { value: "pattern", label: "Pattern" },
  { value: "pick", label: "Pick" },
  { value: "constant", label: "Constant" },
];

type RuleState = {
  kind: GenKind;
  start: string;
  step: string;
  min: string;
  max: string;
  scale: string;
  minLen: string;
  maxLen: string;
  words: string;
  days: string;
  template: string;
  values: string;
  value: string;
};

function emptyRule(): RuleState {
  return {
    kind: "auto",
    start: "1",
    step: "1",
    min: "1",
    max: "100000",
    scale: "2",
    minLen: "5",
    maxLen: "20",
    words: "5",
    days: "90",
    template: "{A}{a}{n}{n}{n}",
    values: "",
    value: "",
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

/** `null` = column stays on the backend's Auto heuristic. */
function buildStrategy(rule: RuleState): GenStrategy | null {
  switch (rule.kind) {
    case "auto":
      return null;
    case "null":
      return { kind: "null" };
    case "sequential":
      return {
        kind: "sequential",
        start: intOr(rule.start, 1),
        step: intOr(rule.step, 1),
      };
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
    case "random_string": {
      const min = Math.max(0, intOr(rule.minLen, 5));
      return {
        kind: "random_string",
        min_len: min,
        max_len: Math.max(min, intOr(rule.maxLen, 20)),
      };
    }
    case "lorem":
      return { kind: "lorem", words: Math.max(1, intOr(rule.words, 5)) };
    case "email":
      return { kind: "email" };
    case "full_name":
      return { kind: "full_name" };
    case "phone":
      return { kind: "phone" };
    case "company":
      return { kind: "company" };
    case "city":
      return { kind: "city" };
    case "country":
      return { kind: "country" };
    case "address":
      return { kind: "address" };
    case "bool":
      return { kind: "bool" };
    case "date_recent":
      return { kind: "date_recent", days: Math.max(1, intOr(rule.days, 90)) };
    case "date_time_recent":
      return { kind: "date_time_recent", days: Math.max(1, intOr(rule.days, 90)) };
    case "uuid":
      return { kind: "uuid" };
    case "pattern":
      return { kind: "pattern", template: rule.template };
    case "pick":
      return {
        kind: "pick",
        values: rule.values
          .split(",")
          .map((s) => s.trim())
          .filter((s) => s.length > 0),
      };
    case "constant":
      return { kind: "constant", value: rule.value };
  }
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
    <label className="flex items-center gap-1 text-[10px] text-slate-400">
      {label}
      <input
        className={`${miniInputCls} ${width}`}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}

const hintCls = "text-[10px] text-slate-400";

function RuleParams({
  rule,
  onChange,
}: {
  rule: RuleState;
  onChange: (patch: Partial<RuleState>) => void;
}) {
  const wrap = "flex flex-wrap items-center gap-x-2 gap-y-1";
  switch (rule.kind) {
    case "auto":
      return <span className={hintCls}>pick by column type &amp; name</span>;
    case "null":
      return <span className={hintCls}>always NULL</span>;
    case "sequential":
      return (
        <div className={wrap}>
          <ParamInput label="start" value={rule.start} onChange={(v) => onChange({ start: v })} />
          <ParamInput label="step" value={rule.step} onChange={(v) => onChange({ step: v })} />
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
    case "random_string":
      return (
        <div className={wrap}>
          <ParamInput label="min len" value={rule.minLen} onChange={(v) => onChange({ minLen: v })} width="w-[44px]" />
          <ParamInput label="max len" value={rule.maxLen} onChange={(v) => onChange({ maxLen: v })} width="w-[44px]" />
        </div>
      );
    case "lorem":
      return (
        <div className={wrap}>
          <ParamInput label="words" value={rule.words} onChange={(v) => onChange({ words: v })} width="w-[48px]" />
        </div>
      );
    case "email":
      return <span className={hintCls}>fake e-mail address</span>;
    case "full_name":
      return <span className={hintCls}>first + last name</span>;
    case "phone":
      return <span className={hintCls}>fake phone number</span>;
    case "company":
      return <span className={hintCls}>fake company name</span>;
    case "city":
      return <span className={hintCls}>random city</span>;
    case "country":
      return <span className={hintCls}>random country</span>;
    case "address":
      return <span className={hintCls}>street number + street</span>;
    case "bool":
      return <span className={hintCls}>true / false</span>;
    case "date_recent":
    case "date_time_recent":
      return (
        <div className={wrap}>
          <ParamInput label="last days" value={rule.days} onChange={(v) => onChange({ days: v })} width="w-[52px]" />
        </div>
      );
    case "uuid":
      return <span className={hintCls}>random UUID v4</span>;
    case "pattern":
      return (
        <div className={wrap}>
          <input
            className={`${miniInputCls} min-w-[180px] flex-1`}
            placeholder="{A}{a}{n}{n}{n}"
            value={rule.template}
            onChange={(e) => onChange({ template: e.target.value })}
          />
          <span className={hintCls}>{"{n} {N} {a} {A} {x} {{ }}"}</span>
        </div>
      );
    case "pick":
      return (
        <div className={wrap}>
          <input
            className={`${miniInputCls} min-w-[180px] flex-1`}
            placeholder="value1, value2, value3"
            value={rule.values}
            onChange={(e) => onChange({ values: e.target.value })}
          />
          <span className={hintCls}>comma separated</span>
        </div>
      );
    case "constant":
      return (
        <div className={wrap}>
          <input
            className={`${miniInputCls} min-w-[180px] flex-1`}
            placeholder="constant value"
            value={rule.value}
            onChange={(e) => onChange({ value: e.target.value })}
          />
        </div>
      );
  }
}

function RuleRow({
  col,
  rule,
  onChange,
}: {
  col: ColumnDef;
  rule: RuleState;
  onChange: (patch: Partial<RuleState>) => void;
}) {
  const skipped = col.is_auto_increment || col.is_generated;
  return (
    <tr className={skipped ? "bg-slate-50/60" : undefined}>
      <td className="border-b border-slate-100 px-2 py-1 align-top">
        <div className="flex items-center gap-1.5">
          <span className="font-medium text-slate-700">{col.name}</span>
          {col.is_primary_key && (
            <span className="rounded bg-amber-100 px-1 text-[9px] font-semibold text-amber-700">
              PK
            </span>
          )}
        </div>
        <div className="text-[10px] text-slate-400">
          {col.native_type}
          {col.nullable ? " · nullable" : " · not null"}
        </div>
      </td>
      <td className="border-b border-slate-100 px-2 py-1 align-top">
        <select
          className={inputCls}
          value={skipped ? "auto" : rule.kind}
          disabled={skipped}
          onChange={(e) => onChange({ kind: e.target.value as GenKind })}
        >
          {GEN_KINDS.map((k) => (
            <option key={k.value} value={k.value}>
              {k.label}
            </option>
          ))}
        </select>
        {skipped && (
          <div className="mt-0.5 text-[10px] text-amber-600">
            skipped — {col.is_auto_increment ? "auto-increment" : "generated"}
          </div>
        )}
      </td>
      <td className="border-b border-slate-100 px-2 py-1 align-top">
        {skipped ? (
          <span className="text-[10px] text-slate-400">value provided by the database</span>
        ) : (
          <RuleParams rule={rule} onChange={onChange} />
        )}
      </td>
    </tr>
  );
}

// ---------------------------------------------------------------------------
// Small utilities
// ---------------------------------------------------------------------------

function useDebounced(value: string, delay: number): string {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(value), delay);
    return () => window.clearTimeout(timer);
  }, [value, delay]);
  return debounced;
}

// ---------------------------------------------------------------------------
// Dialog
// ---------------------------------------------------------------------------

export function DataGenDialog({ onClose }: { onClose: () => void }) {
  const sessions = useStore((s) => s.sessions);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const selected = useStore((s) => s.selected);
  const toast = useStore((s) => s.toast);

  const initialSessionId = activeSessionId ?? sessions[0]?.id ?? "";
  const initialScope =
    selected && selected.sessionId === initialSessionId ? selected.scope : {};

  const [sessionId, setSessionId] = useState(initialSessionId);
  const [database, setDatabase] = useState(initialScope.database ?? "");
  const [schema, setSchema] = useState(initialScope.schema ?? "");
  const [table, setTable] = useState("");
  const [rows, setRows] = useState("1000");
  const [seed, setSeed] = useState("");
  const [batchSize, setBatchSize] = useState("250");

  const [tables, setTables] = useState<ObjectRef[]>([]);
  const [loadingTables, setLoadingTables] = useState(false);
  const [tablesReload, setTablesReload] = useState(0);

  const [columns, setColumns] = useState<ColumnDef[]>([]);
  const [loadingColumns, setLoadingColumns] = useState(false);
  const [ruleMap, setRuleMap] = useState<Record<string, RuleState>>({});

  const [preview, setPreview] = useState<GenResult | null>(null);
  const [busy, setBusy] = useState<"preview" | "execute" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const session = sessions.find((s) => s.id === sessionId) ?? null;
  const selectedTable = tables.find((t) => t.name === table) ?? null;

  const debouncedDb = useDebounced(database.trim(), 300);
  const debouncedSchema = useDebounced(schema.trim(), 300);

  const scopeFromInputs = (db: string, sch: string): Scope => {
    const scope: Scope = {};
    if (db) scope.database = db;
    if (sch) scope.schema = sch;
    return scope;
  };

  // ------------------------------------------------------------ load tables
  useEffect(() => {
    if (!sessionId) {
      setTables([]);
      setLoadingTables(false);
      return;
    }
    let cancelled = false;
    setLoadingTables(true);
    api
      .listObjects(sessionId, scopeFromInputs(debouncedDb, debouncedSchema), "table")
      .then((objects) => {
        if (cancelled) return;
        setTables(objects);
        setError(null);
      })
      .catch((err) => {
        if (!cancelled) setError(errorText(err));
      })
      .finally(() => {
        if (!cancelled) setLoadingTables(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, debouncedDb, debouncedSchema, tablesReload]);

  // ----------------------------------------------------------- load columns
  useEffect(() => {
    if (!sessionId || !table) {
      setColumns([]);
      setRuleMap({});
      setLoadingColumns(false);
      return;
    }
    let cancelled = false;
    setLoadingColumns(true);
    api
      .tableColumns(sessionId, scopeFromInputs(debouncedDb, debouncedSchema), table)
      .then((cols) => {
        if (cancelled) return;
        setColumns(cols);
        const next: Record<string, RuleState> = {};
        for (const col of cols) next[col.name] = emptyRule();
        setRuleMap(next);
        setPreview(null);
        setError(null);
      })
      .catch((err) => {
        if (!cancelled) setError(errorText(err));
      })
      .finally(() => {
        if (!cancelled) setLoadingColumns(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, debouncedDb, debouncedSchema, table]);

  // ---------------------------------------------------------------- actions

  const onSessionChange = (id: string) => {
    setSessionId(id);
    const sel = useStore.getState().selected;
    if (sel && sel.sessionId === id) {
      setDatabase(sel.scope.database ?? "");
      setSchema(sel.scope.schema ?? "");
    } else {
      setDatabase("");
      setSchema("");
    }
    setTable("");
    setTables([]);
    setColumns([]);
    setRuleMap({});
    setPreview(null);
    setError(null);
  };

  const patchRule = (column: string, patch: Partial<RuleState>) =>
    setRuleMap((prev) => ({
      ...prev,
      [column]: { ...(prev[column] ?? emptyRule()), ...patch },
    }));

  const resetRules = () => {
    const next: Record<string, RuleState> = {};
    for (const col of columns) next[col.name] = emptyRule();
    setRuleMap(next);
  };

  const buildRequest = (mode: GenMode): GenRequest | null => {
    if (!sessionId) {
      setError("Choose a session first.");
      return null;
    }
    if (!table) {
      setError("Choose a table first.");
      return null;
    }
    const parsedSeed = parseSeed(seed);
    if (!parsedSeed.ok) {
      setError("Seed must be a non-negative integer.");
      return null;
    }
    const rules: GenColumnRule[] = [];
    for (const col of columns) {
      if (col.is_auto_increment || col.is_generated) continue;
      const rule = ruleMap[col.name];
      if (!rule || rule.kind === "auto") continue;
      const strategy = buildStrategy(rule);
      if (strategy) rules.push({ column: col.name, strategy });
    }
    return {
      scope: scopeFromInputs(database.trim(), schema.trim()),
      table,
      rows: clamp(intOr(rows, 1000), 1, 200_000),
      seed: parsedSeed.value,
      rules,
      batch_size: clamp(intOr(batchSize, 250), 1, 1000),
      mode,
    };
  };

  const runPreview = async () => {
    const request = buildRequest("preview_sql");
    if (!request || !sessionId) return;
    setBusy("preview");
    setError(null);
    try {
      const result = await api.generateData(sessionId, request);
      setPreview(result);
    } catch (err) {
      setError(errorText(err));
      toast("error", errorText(err));
    } finally {
      setBusy(null);
    }
  };

  const runExecute = async () => {
    const request = buildRequest("execute");
    if (!request || !sessionId) return;
    const ok = window.confirm(
      `Insert ${request.rows.toLocaleString()} generated rows into "${request.table}"?\n\n` +
        "The INSERT statements run inside a transaction on the current session.",
    );
    if (!ok) return;
    setBusy("execute");
    setError(null);
    try {
      const result = await api.generateData(sessionId, request);
      setPreview(result);
      const skipped =
        result.skipped_columns.length > 0
          ? ` · skipped: ${result.skipped_columns.join(", ")}`
          : "";
      toast(
        "success",
        `Generated ${result.rows.toLocaleString()} rows into ${request.table} in ${result.elapsed_ms} ms${skipped}`,
      );
    } catch (err) {
      setError(errorText(err));
      toast("error", errorText(err));
    } finally {
      setBusy(null);
    }
  };

  const skippedColumns = columns.filter(
    (c) => c.is_auto_increment || c.is_generated,
  );

  return (
    <DialogShell
      title="Data Generation"
      width={880}
      onClose={onClose}
      footer={
        <>
          <button
            className={`${btnSecondary} mr-auto`}
            onClick={resetRules}
            disabled={columns.length === 0 || busy !== null}
          >
            <RotateCcw size={12} /> Reset rules
          </button>
          <button className={btnSecondary} onClick={onClose} disabled={busy !== null}>
            Close
          </button>
          <button
            className={btnSecondary}
            onClick={() => void runPreview()}
            disabled={busy !== null || !sessionId || !table}
          >
            {busy === "preview" ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Eye size={12} />
            )}
            Preview SQL
          </button>
          <button
            className={btnPrimary}
            onClick={() => void runExecute()}
            disabled={busy !== null || !sessionId || !table}
          >
            {busy === "execute" ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Play size={12} />
            )}
            Execute
          </button>
        </>
      }
    >
      {/* ------------------------------------------------------------ form */}
      <div className="grid grid-cols-6 gap-x-3 gap-y-2 border-b border-slate-200 p-3">
        <div className="col-span-3">
          <label className={labelCls}>Session</label>
          <select
            className={inputCls}
            value={sessionId}
            onChange={(e) => onSessionChange(e.target.value)}
            disabled={sessions.length === 0 || busy !== null}
          >
            {sessions.length === 0 && <option value="">No open connections</option>}
            {sessions.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name} ({s.engine})
              </option>
            ))}
          </select>
        </div>

        <div className="col-span-3">
          <label className={labelCls}>Table</label>
          <div className="flex items-center gap-1">
            <select
              className={inputCls}
              value={table}
              onChange={(e) => setTable(e.target.value)}
              disabled={!sessionId || loadingTables || busy !== null}
            >
              <option value="">
                {loadingTables
                  ? "Loading tables…"
                  : tables.length === 0
                    ? "No tables found"
                    : "Select a table…"}
              </option>
              {tables.map((t) => (
                <option
                  key={`${t.database ?? ""}/${t.schema ?? ""}/${t.name}`}
                  value={t.name}
                >
                  {t.name}
                </option>
              ))}
            </select>
            <button
              className="shrink-0 rounded border border-slate-300 bg-white p-1 text-slate-500 hover:bg-slate-100 disabled:opacity-50"
              title="Reload tables"
              onClick={() => setTablesReload((n) => n + 1)}
              disabled={!sessionId || busy !== null}
            >
              <RefreshCw size={12} className={loadingTables ? "animate-spin" : ""} />
            </button>
          </div>
          {selectedTable?.rows_estimate != null && (
            <div className="mt-0.5 text-[10px] text-slate-400">
              ≈ {selectedTable.rows_estimate.toLocaleString()} rows currently
            </div>
          )}
        </div>

        <div className="col-span-3">
          <label className={labelCls}>Database (optional)</label>
          <input
            className={inputCls}
            value={database}
            placeholder={session?.database ?? "default database"}
            onChange={(e) => setDatabase(e.target.value)}
            disabled={busy !== null}
          />
        </div>

        <div className="col-span-3">
          <label className={labelCls}>Schema (optional)</label>
          <input
            className={inputCls}
            value={schema}
            placeholder="default schema"
            onChange={(e) => setSchema(e.target.value)}
            disabled={busy !== null}
          />
        </div>

        <div className="col-span-2">
          <label className={labelCls}>Rows (1 – 200,000)</label>
          <input
            className={inputCls}
            type="number"
            min={1}
            max={200_000}
            value={rows}
            onChange={(e) => setRows(e.target.value)}
            disabled={busy !== null}
          />
        </div>

        <div className="col-span-2">
          <label className={labelCls}>Seed (optional)</label>
          <input
            className={inputCls}
            value={seed}
            placeholder="time based"
            onChange={(e) => setSeed(e.target.value)}
            disabled={busy !== null}
          />
        </div>

        <div className="col-span-2">
          <label className={labelCls}>Batch size (1 – 1000)</label>
          <input
            className={inputCls}
            type="number"
            min={1}
            max={1000}
            value={batchSize}
            onChange={(e) => setBatchSize(e.target.value)}
            disabled={busy !== null}
          />
        </div>
      </div>

      {error && (
        <div className="flex items-start gap-1.5 whitespace-pre-wrap border-b border-rose-200 bg-rose-50 px-3 py-2 text-[11px] text-rose-600">
          <AlertTriangle size={12} className="mt-0.5 shrink-0" />
          <span>{error}</span>
        </div>
      )}

      {/* --------------------------------------------------- column rules */}
      <div className="p-3">
        <div className="mb-1 flex items-center justify-between">
          <span className="text-[12px] font-semibold text-slate-700">Column rules</span>
          <span className="text-[10px] text-slate-400">
            {columns.length} column(s)
            {skippedColumns.length > 0 &&
              ` · ${skippedColumns.length} skipped (auto-increment / generated)`}
          </span>
        </div>

        {loadingColumns ? (
          <div className="flex items-center gap-2 rounded border border-slate-200 bg-slate-50 px-3 py-4 text-[11px] text-slate-400">
            <Loader2 size={12} className="animate-spin" /> Loading columns…
          </div>
        ) : !table ? (
          <div className="rounded border border-slate-200 bg-slate-50 px-3 py-4 text-[11px] text-slate-400">
            Choose a table to configure per-column strategies.
          </div>
        ) : columns.length === 0 ? (
          <div className="rounded border border-slate-200 bg-slate-50 px-3 py-4 text-[11px] text-slate-400">
            No columns found for this table.
          </div>
        ) : (
          <div className="overflow-hidden rounded border border-slate-200">
            <table className="w-full border-collapse text-left text-[11px]">
              <thead className="bg-[#f1f5f9]">
                <tr className="text-slate-500">
                  <th className="w-[220px] border-b border-slate-200 px-2 py-1 text-[10px] font-medium">
                    Column
                  </th>
                  <th className="w-[150px] border-b border-slate-200 px-2 py-1 text-[10px] font-medium">
                    Strategy
                  </th>
                  <th className="border-b border-slate-200 px-2 py-1 text-[10px] font-medium">
                    Parameters
                  </th>
                </tr>
              </thead>
              <tbody>
                {columns.map((col) => (
                  <RuleRow
                    key={col.name}
                    col={col}
                    rule={ruleMap[col.name] ?? emptyRule()}
                    onChange={(patch) => patchRule(col.name, patch)}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}

        {skippedColumns.length > 0 && (
          <div className="mt-1.5 text-[10px] text-slate-400">
            Skipped: {skippedColumns.map((c) => c.name).join(", ")} — the database
            provides these values.
          </div>
        )}
      </div>

      {/* -------------------------------------------------------- preview */}
      {preview && (
        <div className="border-t border-slate-200 p-3">
          <div className="mb-1 flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2 text-[11px] text-slate-600">
              <span className="font-semibold">
                {preview.executed ? "Executed" : "Preview"}
              </span>
              <span>{preview.rows.toLocaleString()} row(s)</span>
              <span className="text-slate-400">{preview.elapsed_ms} ms</span>
            </div>
            {preview.truncated && (
              <span className="flex items-center gap-1 text-[10px] text-amber-600">
                <AlertTriangle size={10} />
                SQL preview truncated (first 1000 rows)
              </span>
            )}
          </div>
          <pre className="custom-scrollbar max-h-[240px] overflow-auto rounded border border-slate-800 bg-slate-900 p-2 font-mono text-[10px] leading-relaxed whitespace-pre text-slate-100">
            {preview.sql}
          </pre>
          {preview.skipped_columns.length > 0 && (
            <div className="mt-1 text-[10px] text-slate-400">
              Skipped by the backend: {preview.skipped_columns.join(", ")}
            </div>
          )}
        </div>
      )}
    </DialogShell>
  );
}
