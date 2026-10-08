// Table Designer dialog: CREATE TABLE (`mode="create"`, opened via
// `store.openObjectDialog("new-table", …)`) and ALTER TABLE for an existing
// table (`mode="edit"`, via "edit-table"). The edit mode diffs the live
// columns from `api.tableColumns` against the grid and previews/applies one
// `ALTER TABLE` statement per change (see `buildAlterTable`).

import { useEffect, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  Check,
  Loader2,
  Play,
  Plus,
  RotateCcw,
  Trash2,
} from "lucide-react";

import type { ColumnDef, Engine, Scope } from "../../lib/bindings";
import { api, errorText } from "../../lib/api";
import { confirmDialog } from "../../lib/dialog";
import { ENGINE_META } from "../../lib/engines";
import {
  buildAlterTable,
  buildCreateTable,
  buildRenameColumn,
  commonTypes,
  isValidObjectName,
  supportsCreateTable,
  type ColumnSpec,
} from "../../lib/table-ddl";
import { useStore } from "../../lib/store";
import {
  DLG_BTN,
  DLG_BTN_PRIMARY,
  DLG_INPUT,
  DLG_LABEL,
  DLG_PRE,
  DialogError,
  DialogNotice,
  ObjectDialogShell,
} from "./DialogChrome";

const CELL_INPUT =
  "h-7 w-full rounded-md border border-slate-300 bg-white px-1.5 text-[12.5px] text-slate-800 placeholder:text-slate-400 focus:ring-2 focus:ring-accent/40 focus:outline-none disabled:bg-slate-50";
const CELL_HEAD =
  "border-b border-slate-200/70 px-1.5 py-1.5 text-[11.5px] font-semibold text-slate-500";
const ICON_BTN =
  "grid h-6 w-6 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600 disabled:cursor-not-allowed disabled:opacity-30";

type ColumnRow = {
  key: number;
  name: string;
  type: string;
  notNull: boolean;
  primaryKey: boolean;
  autoIncrement: boolean;
  defaultText: string;
  /**
   * Live column name this row was loaded from (`mode="edit"` only, `null`
   * for newly added rows). A changed `name` is treated as an explicit
   * column rename (see `buildRenameColumn`).
   */
  originalName: string | null;
};

let rowSeq = 1;

function makeRow(patch?: Partial<Omit<ColumnRow, "key">>): ColumnRow {
  return {
    key: rowSeq++,
    name: "",
    type: "",
    notNull: false,
    primaryKey: false,
    autoIncrement: false,
    defaultText: "",
    originalName: null,
    ...patch,
  };
}

/** Starter columns: `id` PK + auto-increment and `name`, engine-aware types. */
function initialRows(engine: Engine | null): ColumnRow[] {
  const idType =
    engine === "sqlite" ? "INTEGER" : engine === "sql_server" ? "INT" : "BIGINT";
  const nameType =
    engine === "sqlite"
      ? "TEXT"
      : engine === "sql_server"
        ? "NVARCHAR(255)"
        : "VARCHAR(255)";
  return [
    makeRow({ name: "id", type: idType, primaryKey: true, autoIncrement: true }),
    makeRow({ name: "name", type: nameType, notNull: true }),
  ];
}

function toSpec(row: ColumnRow): ColumnSpec {
  return {
    name: row.name,
    type: row.type,
    notNull: row.notNull,
    primaryKey: row.primaryKey,
    defaultText: row.defaultText,
    autoIncrement: row.autoIncrement,
  };
}

/** Map a live catalog column to an editable grid row. */
function fromColumnDef(col: ColumnDef): ColumnRow {
  return makeRow({
    name: col.name,
    type: col.native_type,
    notNull: !col.nullable,
    primaryKey: col.is_primary_key,
    autoIncrement: col.is_auto_increment,
    defaultText: col.default_value ?? "",
    originalName: col.name,
  });
}

/** Per-row name/type checks shared by create and edit modes. */
function validateRows(rows: ColumnRow[]): string[] {
  const problems: string[] = [];
  if (rows.length === 0) problems.push("Add at least one column.");
  const seen = new Set<string>();
  rows.forEach((row, i) => {
    const col = row.name.trim();
    if (!isValidObjectName(col)) {
      problems.push(`Column ${i + 1}: name is required.`);
    } else {
      const key = col.toLowerCase();
      if (seen.has(key)) problems.push(`Duplicate column name “${col}”.`);
      seen.add(key);
    }
    if (!row.type.trim()) problems.push(`Column ${i + 1}: type is required.`);
  });
  return problems;
}

/** Editable column grid shared by create and edit modes. */
function ColumnGrid({
  rows,
  disabled,
  types,
  onPatch,
  onMove,
  onRemove,
  onAdd,
}: {
  rows: ColumnRow[];
  disabled: boolean;
  types: readonly string[];
  onPatch: (index: number, patch: Partial<ColumnRow>) => void;
  onMove: (index: number, delta: number) => void;
  onRemove: (index: number) => void;
  onAdd: () => void;
}) {
  return (
    <div>
      <label className={DLG_LABEL}>Columns</label>
      <div className="overflow-hidden rounded-md border border-slate-200">
        <table className="w-full border-collapse text-left text-[12.5px]">
          <thead className="bg-sidebar">
            <tr>
              <th className={`w-[180px] ${CELL_HEAD}`}>Name</th>
              <th className={`w-[190px] ${CELL_HEAD}`}>Type</th>
              <th className={`w-[56px] ${CELL_HEAD}`}>Not null</th>
              <th className={`w-[44px] ${CELL_HEAD}`}>PK</th>
              <th className={`w-[66px] ${CELL_HEAD}`}>Auto inc</th>
              <th className={CELL_HEAD}>Default</th>
              <th className={`w-[96px] ${CELL_HEAD}`}>Order</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row, i) => (
              <tr key={row.key}>
                <td className="border-b border-slate-200/70 px-1.5 py-1 align-middle">
                  <input
                    className={CELL_INPUT}
                    value={row.name}
                    placeholder="column"
                    disabled={disabled}
                    onChange={(e) => onPatch(i, { name: e.target.value })}
                  />
                </td>
                <td className="border-b border-slate-200/70 px-1.5 py-1 align-middle">
                  <input
                    className={CELL_INPUT}
                    value={row.type}
                    placeholder="VARCHAR(255)"
                    list="table-designer-types"
                    disabled={disabled}
                    onChange={(e) => onPatch(i, { type: e.target.value })}
                  />
                </td>
                <td className="border-b border-slate-200/70 px-1.5 py-1 text-center align-middle">
                  <input
                    type="checkbox"
                    className="h-3.5 w-3.5 accent-accent"
                    checked={row.notNull}
                    disabled={disabled}
                    title="NOT NULL"
                    onChange={(e) => onPatch(i, { notNull: e.target.checked })}
                  />
                </td>
                <td className="border-b border-slate-200/70 px-1.5 py-1 text-center align-middle">
                  <input
                    type="checkbox"
                    className="h-3.5 w-3.5 accent-accent"
                    checked={row.primaryKey}
                    disabled={disabled}
                    title="PRIMARY KEY"
                    onChange={(e) => onPatch(i, { primaryKey: e.target.checked })}
                  />
                </td>
                <td className="border-b border-slate-200/70 px-1.5 py-1 text-center align-middle">
                  <input
                    type="checkbox"
                    className="h-3.5 w-3.5 accent-accent"
                    checked={row.autoIncrement}
                    disabled={disabled}
                    title="AUTO_INCREMENT / IDENTITY"
                    onChange={(e) =>
                      onPatch(i, { autoIncrement: e.target.checked })
                    }
                  />
                </td>
                <td className="border-b border-slate-200/70 px-1.5 py-1 align-middle">
                  <input
                    className={CELL_INPUT}
                    value={row.defaultText}
                    placeholder="raw SQL, e.g. CURRENT_TIMESTAMP"
                    disabled={disabled}
                    onChange={(e) => onPatch(i, { defaultText: e.target.value })}
                  />
                </td>
                <td className="border-b border-slate-200/70 px-1.5 py-1 align-middle">
                  <div className="flex h-7 items-center gap-0.5">
                    <button
                      className={ICON_BTN}
                      onClick={() => onMove(i, -1)}
                      disabled={i === 0 || disabled}
                      title="Move up"
                    >
                      <ArrowUp size={12} strokeWidth={1.8} />
                    </button>
                    <button
                      className={ICON_BTN}
                      onClick={() => onMove(i, 1)}
                      disabled={i === rows.length - 1 || disabled}
                      title="Move down"
                    >
                      <ArrowDown size={12} strokeWidth={1.8} />
                    </button>
                    <button
                      className={`${ICON_BTN} hover:text-rose-600`}
                      onClick={() => onRemove(i)}
                      disabled={disabled}
                      title="Remove column"
                    >
                      <Trash2 size={12} strokeWidth={1.8} />
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="mt-2">
        <button
          className="flex h-7 items-center gap-1 rounded-md border border-slate-300 bg-white px-2 text-[12px] text-slate-700 hover:bg-slate-50 disabled:opacity-50"
          onClick={onAdd}
          disabled={disabled}
        >
          <Plus size={12} strokeWidth={1.8} />
          Add column
        </button>
      </div>
      <datalist id="table-designer-types">
        {types.map((t) => (
          <option key={t} value={t} />
        ))}
      </datalist>
    </div>
  );
}

export function TableDesignerDialog({
  mode,
  sessionId,
  scope,
  table,
  onClose,
}: {
  mode: "create" | "edit";
  sessionId: string | null;
  scope: Scope;
  table?: string;
  onClose: () => void;
}) {
  const session = useStore(
    (s) => s.sessions.find((x) => x.id === sessionId) ?? null,
  );
  const toast = useStore((s) => s.toast);

  const [tableName, setTableName] = useState("");
  const [rows, setRows] = useState<ColumnRow[]>(() =>
    initialRows(session?.engine ?? null),
  );
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const engine = session?.engine ?? null;
  const supported = supportsCreateTable(engine);
  const types = commonTypes(engine);
  const previewEngine = engine ?? "postgres";
  const name = tableName.trim();

  // ------------------------------------------------------------- edit mode
  if (mode === "edit") {
    return (
      <EditTableDialog
        sessionId={sessionId}
        scope={scope}
        table={table}
        onClose={onClose}
      />
    );
  }

  // ------------------------------------------------------------ validation
  const problems: string[] = [];
  if (!isValidObjectName(name)) {
    problems.push(
      "Table name is required — letters, digits, “_” and “$” only.",
    );
  }
  problems.push(...validateRows(rows));

  const { sql, warnings } = buildCreateTable(
    previewEngine,
    name || "new_table",
    rows.map(toSpec),
    scope,
  );
  const canRun =
    supported && problems.length === 0 && !running && sessionId !== null;

  const patchRow = (index: number, patch: Partial<ColumnRow>) =>
    setRows((prev) =>
      prev.map((row, i) => (i === index ? { ...row, ...patch } : row)),
    );

  const move = (index: number, delta: number) =>
    setRows((prev) => {
      const j = index + delta;
      if (j < 0 || j >= prev.length) return prev;
      const next = prev.slice();
      const tmp = next[index]!;
      next[index] = next[j]!;
      next[j] = tmp;
      return next;
    });

  const remove = (index: number) =>
    setRows((prev) => prev.filter((_, i) => i !== index));

  const run = async () => {
    if (!sessionId || !supported || problems.length > 0) return;
    setRunning(true);
    setError(null);
    try {
      const res = await api.executeBatch(sessionId, [sql], false);
      if (res.failed > 0) {
        setError(res.errors.join("\n") || "The backend did not apply the statement.");
        return;
      }
      toast("success", `Table “${name}” created`);
      window.dispatchEvent(new CustomEvent("mieosql:refresh-tree"));
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setRunning(false);
    }
  };

  return (
    <ObjectDialogShell
      title="New Table"
      width={900}
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
              <Play size={13} strokeWidth={1.8} />
            )}
            Create Table
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        <div className="flex flex-wrap items-center gap-2 text-[12px] text-slate-600">
          {session ? (
            <>
              <span className="font-medium text-slate-800">{session.name}</span>
              <span>{ENGINE_META[session.engine].label}</span>
            </>
          ) : (
            <span>No active session.</span>
          )}
          {(scope.database || scope.schema) && (
            <span>
              · target {[scope.database, scope.schema].filter(Boolean).join(" / ")}
            </span>
          )}
        </div>

        {!session && (
          <DialogError text="Open a connection before creating a table." />
        )}

        {session && !supported && (
          <DialogNotice>
            CREATE TABLE is not supported for {ENGINE_META[session.engine].label}{" "}
            connections.
          </DialogNotice>
        )}

        {supported && (
          <>
            <div>
              <label className={DLG_LABEL}>Table name *</label>
              <input
                className={DLG_INPUT}
                value={tableName}
                placeholder="customers"
                autoFocus
                disabled={running}
                onChange={(e) => setTableName(e.target.value)}
              />
            </div>

            <ColumnGrid
              rows={rows}
              disabled={running}
              types={types}
              onPatch={patchRow}
              onMove={move}
              onRemove={remove}
              onAdd={() => setRows((prev) => [...prev, makeRow()])}
            />

            <div>
              <label className={DLG_LABEL}>Preview</label>
              <pre className={DLG_PRE}>{sql};</pre>
            </div>

            {warnings.length > 0 && (
              <DialogNotice>
                <ul className="list-inside list-disc space-y-0.5">
                  {warnings.map((w) => (
                    <li key={w}>{w}</li>
                  ))}
                </ul>
              </DialogNotice>
            )}

            {problems.length > 0 && (
              <div className="rounded-md border border-rose-200 bg-rose-50 p-2.5 text-[12.5px] text-rose-600">
                <ul className="list-inside list-disc space-y-0.5">
                  {problems.slice(0, 4).map((p) => (
                    <li key={p}>{p}</li>
                  ))}
                </ul>
              </div>
            )}
          </>
        )}

        {error && <DialogError text={error} />}
      </div>
    </ObjectDialogShell>
  );
}

// ------------------------------------------------------------------ edit --

type LoadState = "loading" | "ready" | "error";

/**
 * Edit mode: loads the live columns of an existing table and diffs them
 * against the grid. Renames are detected per row via `originalName` and
 * emitted with {@link buildRenameColumn}; everything else goes through
 * {@link buildAlterTable} (matched by exact name, primary keys untouched).
 * RENAME statements run last, so the diff sees pre-rename (old) names.
 */
function EditTableDialog({
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
  const session = useStore(
    (s) => s.sessions.find((x) => x.id === sessionId) ?? null,
  );
  const toast = useStore((s) => s.toast);

  const [rows, setRows] = useState<ColumnRow[]>([]);
  const [baseline, setBaseline] = useState<ColumnSpec[]>([]);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const engine = session?.engine ?? null;
  const supported = supportsCreateTable(engine);
  const types = commonTypes(engine);
  const scopeKey = JSON.stringify(scope);
  const tableName = table ?? "";

  useEffect(() => {
    let cancelled = false;
    if (!sessionId) {
      setLoadState("error");
      setLoadError("No active session — open a connection before editing a table.");
      return;
    }
    if (!table) {
      setLoadState("error");
      setLoadError("No table selected.");
      return;
    }
    setLoadState("loading");
    setLoadError(null);
    api
      .tableColumns(sessionId, scope, table)
      .then((cols) => {
        if (cancelled) return;
        const next = [...cols]
          .sort((a, b) => a.ordinal - b.ordinal)
          .map(fromColumnDef);
        setRows(next);
        setBaseline(next.map(toSpec));
        setLoadState("ready");
      })
      .catch((err) => {
        if (cancelled) return;
        setLoadError(errorText(err));
        setLoadState("error");
      });
    return () => {
      cancelled = true;
    };
    // `scope` is compared via its serialized form; the closure value is fresh
    // on every re-run triggered by `scopeKey`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, table, scopeKey, nonce]);

  const patchRow = (index: number, patch: Partial<ColumnRow>) =>
    setRows((prev) =>
      prev.map((row, i) => (i === index ? { ...row, ...patch } : row)),
    );

  const move = (index: number, delta: number) =>
    setRows((prev) => {
      const j = index + delta;
      if (j < 0 || j >= prev.length) return prev;
      const next = prev.slice();
      const tmp = next[index]!;
      next[index] = next[j]!;
      next[j] = tmp;
      return next;
    });

  const remove = (index: number) =>
    setRows((prev) => prev.filter((_, i) => i !== index));

  const problems = validateRows(rows);

  // Explicit renames: rows whose name was edited after loading.
  const renames = rows.filter(
    (row) =>
      row.originalName !== null &&
      row.name.trim() !== "" &&
      row.originalName !== row.name.trim(),
  );
  // The name-based diff sees pre-rename names for renamed rows, so attribute
  // changes on them still diff against the right column.
  const effectiveNew: ColumnSpec[] = rows.map((row) =>
    row.originalName !== null && row.originalName !== row.name.trim()
      ? { ...toSpec(row), name: row.originalName }
      : toSpec(row),
  );

  const alter =
    engine !== null && loadState === "ready"
      ? buildAlterTable(engine, tableName, scope, baseline, effectiveNew)
      : { sql: [] as string[], warnings: [] as string[], destructive: [] as string[] };

  const renameSql: string[] = [];
  const renameWarnings: string[] = [];
  if (engine !== null && loadState === "ready") {
    for (const row of renames) {
      if (!isValidObjectName(row.name.trim())) continue;
      const res = buildRenameColumn(engine, tableName, scope, row.originalName!, toSpec(row));
      if (res.sql) renameSql.push(res.sql);
      if (res.warning) renameWarnings.push(res.warning);
    }
  }

  const statements = [...alter.sql, ...renameSql];
  const notices = [...alter.warnings, ...renameWarnings];
  const destructive = alter.destructive;
  const isMysql = engine === "mysql" || engine === "mariadb";

  const canRun =
    supported &&
    problems.length === 0 &&
    statements.length > 0 &&
    !running &&
    sessionId !== null &&
    loadState === "ready";

  const apply = async () => {
    if (!sessionId || !canRun) return;
    if (destructive.length > 0) {
      const ok = await confirmDialog(
        `This will permanently delete data:\n${destructive.map((d) => `• ${d}`).join("\n")}\n\nApply ${statements.length} change(s) to “${tableName}” anyway?`,
        { title: "Destructive changes", kind: "warning" },
      );
      if (!ok) return;
    }
    setRunning(true);
    setError(null);
    try {
      const res = await api.executeBatch(sessionId, statements, false);
      if (res.failed > 0) {
        setError(res.errors.join("\n") || "The backend did not apply the statements.");
        return;
      }
      toast(
        "success",
        `Table “${tableName}” updated (${statements.length} change${statements.length === 1 ? "" : "s"})`,
      );
      window.dispatchEvent(new CustomEvent("mieosql:refresh-tree"));
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setRunning(false);
    }
  };

  return (
    <ObjectDialogShell
      title={tableName ? `Edit Table — ${tableName}` : "Edit Table"}
      width={900}
      busy={running}
      onClose={onClose}
      footer={
        <>
          <button className={DLG_BTN} onClick={onClose} disabled={running}>
            Cancel
          </button>
          <button
            className={DLG_BTN_PRIMARY}
            onClick={() => void apply()}
            disabled={!canRun}
          >
            {running ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <Check size={13} strokeWidth={1.8} />
            )}
            Apply Changes
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        <div className="flex flex-wrap items-center gap-2 text-[12px] text-slate-600">
          {session ? (
            <>
              <span className="font-medium text-slate-800">{session.name}</span>
              <span>{ENGINE_META[session.engine].label}</span>
            </>
          ) : (
            <span>No active session.</span>
          )}
          {(scope.database || scope.schema) && (
            <span>
              · target {[scope.database, scope.schema].filter(Boolean).join(" / ")}
            </span>
          )}
        </div>

        {!session && (
          <DialogError text="Open a connection before editing a table." />
        )}

        {session && !supported && (
          <DialogNotice>
            ALTER TABLE is not supported for {ENGINE_META[session.engine].label}{" "}
            connections.
          </DialogNotice>
        )}

        {loadState === "loading" && (
          <div className="flex items-center gap-2 py-6 text-[12.5px] text-slate-500">
            <Loader2 size={14} className="animate-spin" />
            Loading columns{tableName ? ` for “${tableName}”` : ""}…
          </div>
        )}

        {loadState === "error" && (
          <>
            <DialogError text={loadError ?? "Could not load the table columns."} />
            <button
              className={DLG_BTN}
              onClick={() => setNonce((n) => n + 1)}
            >
              <RotateCcw size={13} strokeWidth={1.8} />
              Retry
            </button>
          </>
        )}

        {loadState === "ready" && supported && (
          <>
            <ColumnGrid
              rows={rows}
              disabled={running}
              types={types}
              onPatch={patchRow}
              onMove={move}
              onRemove={remove}
              onAdd={() => setRows((prev) => [...prev, makeRow()])}
            />

            {rows.length === 0 && (
              <DialogNotice>
                This table has no columns — add one above to generate an ADD
                COLUMN statement.
              </DialogNotice>
            )}

            {isMysql && statements.length > 0 && (
              <DialogNotice>
                MySQL applies DDL immediately (no rollback) — each statement
                takes effect as soon as it runs.
              </DialogNotice>
            )}

            <div>
              <label className={DLG_LABEL}>
                Preview
                {statements.length > 0 &&
                  ` (${statements.length} statement${statements.length === 1 ? "" : "s"})`}
              </label>
              {statements.length > 0 ? (
                <pre className={DLG_PRE}>
                  {statements.map((s) => `${s};`).join("\n")}
                </pre>
              ) : (
                <DialogNotice>
                  No changes — edit the columns above to generate ALTER
                  statements.
                </DialogNotice>
              )}
            </div>

            {destructive.length > 0 && (
              <DialogError text={destructive.join("\n")} />
            )}

            {notices.filter((w) => !destructive.includes(w)).length > 0 && (
              <DialogNotice>
                <ul className="list-inside list-disc space-y-0.5">
                  {notices
                    .filter((w) => !destructive.includes(w))
                    .map((w) => (
                      <li key={w}>{w}</li>
                    ))}
                </ul>
              </DialogNotice>
            )}

            {problems.length > 0 && (
              <div className="rounded-md border border-rose-200 bg-rose-50 p-2.5 text-[12.5px] text-rose-600">
                <ul className="list-inside list-disc space-y-0.5">
                  {problems.slice(0, 4).map((p) => (
                    <li key={p}>{p}</li>
                  ))}
                </ul>
              </div>
            )}
          </>
        )}

        {error && <DialogError text={error} />}
      </div>
    </ObjectDialogShell>
  );
}
