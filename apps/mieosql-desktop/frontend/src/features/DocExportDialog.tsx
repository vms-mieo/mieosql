// Data dictionary (documentation) export dialog (feature pack v1 · F3 — Data Tools).
//
// Exports Markdown / HTML documentation for every table (and optionally view)
// in the selected scope: columns, indexes, foreign keys, DDL, comments.
//
// Owned by the Data Tools agent — this file is the only one edited here.

import { useState } from "react";
import { AlertTriangle, FileCode2, FileText, Loader2 } from "lucide-react";
import { save } from "@tauri-apps/plugin-dialog";

import type { DocFormat, DocOptions, DocSummary, Scope } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { formatBytes } from "../lib/format";
import { useStore } from "../lib/store";
import { DialogShell } from "./DialogShell";

// ---------------------------------------------------------------------------
// Shared styles (feature-pack dialog conventions)
// ---------------------------------------------------------------------------

const inputCls =
  "h-7 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-800 placeholder:text-slate-500 focus:ring-2 focus:ring-accent/40 focus:outline-none disabled:bg-slate-100 disabled:text-slate-400";
const labelCls = "mb-1 block text-[11px] font-medium text-slate-500";
const btnPrimary =
  "flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white transition-colors hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50";
const btnSecondary =
  "flex h-7 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 transition-colors hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50";

function Checkbox({
  label,
  checked,
  onChange,
  hint,
}: {
  label: string;
  checked: boolean;
  onChange: (next: boolean) => void;
  hint?: string;
}) {
  return (
    <label className="flex items-center gap-1.5 text-[13px] text-slate-700">
      <input
        type="checkbox"
        className="accent-accent"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span>{label}</span>
      {hint && <span className="text-[11px] text-slate-500">{hint}</span>}
    </label>
  );
}

// ---------------------------------------------------------------------------
// Dialog
// ---------------------------------------------------------------------------

export function DocExportDialog({ onClose }: { onClose: () => void }) {
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
  const [format, setFormat] = useState<DocFormat>("markdown");
  const [includeColumns, setIncludeColumns] = useState(true);
  const [includeIndexes, setIncludeIndexes] = useState(true);
  const [includeForeignKeys, setIncludeForeignKeys] = useState(true);
  const [includeViews, setIncludeViews] = useState(false);
  const [includeDdl, setIncludeDdl] = useState(false);
  const [title, setTitle] = useState("");

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [summary, setSummary] = useState<DocSummary | null>(null);

  const session = sessions.find((s) => s.id === sessionId) ?? null;

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
    setSummary(null);
    setError(null);
  };

  const onExport = async () => {
    if (!sessionId) {
      setError("Choose a session first.");
      return;
    }
    const isMarkdown = format === "markdown";
    const ext = isMarkdown ? "md" : "html";
    const label = isMarkdown ? "Markdown" : "HTML";

    const scope: Scope = {};
    const db = database.trim();
    const sch = schema.trim();
    if (db) scope.database = db;
    if (sch) scope.schema = sch;

    const base = (
      [db, sch].filter(Boolean).join("-") ||
      session?.name ||
      "database"
    )
      .replace(/[^\w.-]+/g, "_")
      .slice(0, 60);
    const stamp = new Date()
      .toISOString()
      .slice(0, 19)
      .replace(/[:T]/g, "-");

    let path: string | null = null;
    try {
      path = await save({
        title: `Export data dictionary (${label})`,
        defaultPath: `mieosql-datadict-${base}-${stamp}.${ext}`,
        filters: [{ name: label, extensions: [ext] }],
      });
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!path) return; // cancelled

    setBusy(true);
    setError(null);
    try {
      const options: DocOptions = {
        scope,
        format,
        include_columns: includeColumns,
        include_indexes: includeIndexes,
        include_foreign_keys: includeForeignKeys,
        include_views: includeViews,
        include_ddl: includeDdl,
      };
      const trimmedTitle = title.trim();
      if (trimmedTitle) options.title = trimmedTitle;

      const res = await api.exportDocumentation(sessionId, options, path);
      setSummary(res);
      toast(
        "success",
        `Data dictionary: ${res.tables.toLocaleString()} table(s) · ` +
          `${res.columns.toLocaleString()} column(s) · ${formatBytes(res.bytes)} ` +
          `in ${res.elapsed_ms} ms\n${res.path}`,
      );
    } catch (err) {
      setError(errorText(err));
      toast("error", errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <DialogShell
      title="Data Dictionary"
      width={680}
      onClose={onClose}
      footer={
        <>
          {summary && !busy && (
            <span
              className="mr-auto truncate text-[11px] text-slate-500"
              title={summary.path}
            >
              Last export: {summary.tables.toLocaleString()} table(s) ·{" "}
              {formatBytes(summary.bytes)} → {summary.path}
            </span>
          )}
          <button className={btnSecondary} onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className={btnPrimary}
            onClick={() => void onExport()}
            disabled={busy || !sessionId}
          >
            {busy ? (
              <Loader2 size={12} className="animate-spin" />
            ) : format === "markdown" ? (
              <FileText size={12} />
            ) : (
              <FileCode2 size={12} />
            )}
            Export…
          </button>
        </>
      }
    >
      <div className="grid grid-cols-2 gap-x-3 gap-y-3 border-b border-slate-200/80 p-4">
        <div>
          <label className={labelCls}>Session</label>
          <select
            className={inputCls}
            value={sessionId}
            onChange={(e) => onSessionChange(e.target.value)}
            disabled={sessions.length === 0 || busy}
          >
            {sessions.length === 0 && <option value="">No open connections</option>}
            {sessions.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name} ({s.engine})
              </option>
            ))}
          </select>
        </div>

        <div>
          <label className={labelCls}>Format</label>
          <select
            className={inputCls}
            value={format}
            onChange={(e) => setFormat(e.target.value as DocFormat)}
            disabled={busy}
          >
            <option value="markdown">Markdown (.md)</option>
            <option value="html">HTML (.html — self-contained)</option>
          </select>
        </div>

        <div>
          <label className={labelCls}>Database (optional)</label>
          <input
            className={inputCls}
            value={database}
            placeholder={session?.database ?? "default database"}
            onChange={(e) => setDatabase(e.target.value)}
            disabled={busy}
          />
        </div>

        <div>
          <label className={labelCls}>Schema (optional)</label>
          <input
            className={inputCls}
            value={schema}
            placeholder="default schema"
            onChange={(e) => setSchema(e.target.value)}
            disabled={busy}
          />
        </div>

        <div className="col-span-2">
          <label className={labelCls}>Document title (optional)</label>
          <input
            className={inputCls}
            value={title}
            placeholder="defaults to the scope name"
            onChange={(e) => setTitle(e.target.value)}
            disabled={busy}
          />
        </div>
      </div>

      <div className="space-y-2 p-4">
        <div className="text-[13px] font-semibold text-slate-900">Include</div>
        <div className="flex flex-wrap gap-x-6 gap-y-2">
          <Checkbox
            label="Columns"
            checked={includeColumns}
            onChange={setIncludeColumns}
          />
          <Checkbox
            label="Indexes"
            checked={includeIndexes}
            onChange={setIncludeIndexes}
          />
          <Checkbox
            label="Foreign keys"
            checked={includeForeignKeys}
            onChange={setIncludeForeignKeys}
          />
          <Checkbox label="Views" checked={includeViews} onChange={setIncludeViews} />
          <Checkbox
            label="DDL"
            hint="(slower)"
            checked={includeDdl}
            onChange={setIncludeDdl}
          />
        </div>
        <p className="text-[12px] leading-relaxed text-slate-500">
          Documents every table in the selected database/schema. Column comments are
          written as the glossary; DDL is fetched per object and can be slow on large
          schemas.
        </p>
      </div>

      {error && (
        <div className="mx-4 mb-4 flex items-start gap-2 rounded-lg bg-rose-50 px-3 py-2 text-[12.5px] whitespace-pre-wrap text-rose-600">
          <AlertTriangle size={12} className="mt-0.5 shrink-0" />
          <span>{error}</span>
        </div>
      )}

      {summary && !error && (
        <div className="mx-4 mb-4 rounded-lg bg-emerald-50 px-3 py-2 text-[12.5px] text-emerald-700">
          <div className="font-medium">
            {summary.tables.toLocaleString()} table(s) ·{" "}
            {summary.columns.toLocaleString()} column(s) ·{" "}
            {summary.indexes.toLocaleString()} index(es) ·{" "}
            {summary.foreign_keys.toLocaleString()} FK(s) ·{" "}
            {formatBytes(summary.bytes)} in {summary.elapsed_ms} ms
          </div>
          <div className="mt-1 font-code text-[11px]">{summary.path}</div>
        </div>
      )}
    </DialogShell>
  );
}
