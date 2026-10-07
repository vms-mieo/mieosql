// Light visual query builder: pick a session/table, tick columns, add filters
// and sorting, watch the SQL preview and insert the query into a new tab.

import { useEffect, useMemo, useRef, useState } from "react";
import { Plus, RefreshCw, Trash2 } from "lucide-react";

import { api, errorText } from "../lib/api";
import type { Engine, Scope, TableSnapshot } from "../lib/bindings";
import { useStore } from "../lib/store";
import { DialogShell } from "./DialogShell";

type FilterRow = { id: number; column: string; op: string; value: string };
type OrderRow = { id: number; column: string; dir: "ASC" | "DESC" };

const OPERATORS = [
  "=",
  "<>",
  ">",
  ">=",
  "<",
  "<=",
  "LIKE",
  "NOT LIKE",
  "IN",
  "IS NULL",
  "IS NOT NULL",
] as const;

function isNullOp(op: string): boolean {
  return op === "IS NULL" || op === "IS NOT NULL";
}

function quoteIdent(engine: Engine | null, name: string): string {
  if (engine === "mysql" || engine === "mariadb") {
    return `\`${name.replace(/`/g, "``")}\``;
  }
  return `"${name.replace(/"/g, '""')}"`;
}

function literal(raw: string): string {
  const text = raw.trim();
  if (/^-?\d+$/.test(text)) return text;
  if (/^-?\d+\.\d+$/.test(text)) return text;
  if (/^null$/i.test(text)) return "NULL";
  if (/^(true|false)$/i.test(text)) return text.toUpperCase();
  return `'${text.replace(/'/g, "''")}'`;
}

function filterSql(engine: Engine | null, row: FilterRow): string | null {
  if (!row.column) return null;
  const column = quoteIdent(engine, row.column);
  if (isNullOp(row.op)) return `${column} ${row.op}`;
  const raw = row.value.trim();
  if (raw === "") return null;
  if (row.op === "IN") {
    const items = raw
      .split(",")
      .map((v) => v.trim())
      .filter((v) => v.length > 0);
    if (items.length === 0) return null;
    return `${column} IN (${items.map(literal).join(", ")})`;
  }
  return `${column} ${row.op} ${literal(raw)}`;
}

export function QueryBuilderDialog({ onClose }: { onClose: () => void }) {
  const sessions = useStore((s) => s.sessions);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const selected = useStore((s) => s.selected);
  const activeTab = useStore((s) => s.tabs.find((t) => t.id === s.activeTabId));
  const newQueryTab = useStore((s) => s.newQueryTab);
  const toast = useStore((s) => s.toast);

  const initialSessionId =
    activeTab?.sessionId ?? activeSessionId ?? sessions[0]?.id ?? null;
  const initialSession = sessions.find((s) => s.id === initialSessionId) ?? null;
  const initialScope =
    selected && selected.sessionId === initialSessionId ? selected.scope : null;

  const [sessionId, setSessionId] = useState<string | null>(initialSessionId);
  const [database, setDatabase] = useState(
    initialScope?.database ?? initialSession?.database ?? "",
  );
  const [schema, setSchema] = useState(initialScope?.schema ?? "");
  const [tables, setTables] = useState<TableSnapshot[] | null>(null);
  const [tableName, setTableName] = useState("");
  const [cols, setCols] = useState<Record<string, boolean>>({});
  const [filters, setFilters] = useState<FilterRow[]>([]);
  const [orders, setOrders] = useState<OrderRow[]>([]);
  const [limit, setLimit] = useState("100");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);

  const session = sessions.find((s) => s.id === sessionId) ?? null;
  const engine = session?.engine ?? null;

  const scope = useMemo<Scope>(() => {
    const out: Scope = {};
    if (database.trim()) out.database = database.trim();
    if (schema.trim()) out.schema = schema.trim();
    return out;
  }, [database, schema]);

  const loadTables = async () => {
    if (!sessionId) return;
    setLoading(true);
    setError(null);
    try {
      const snapshot = await api.schemaSnapshot(sessionId, scope, false, 200);
      const list = snapshot.tables.filter(
        (t) => t.kind === "table" || t.kind === "view",
      );
      setTables(list);
      setTableName((current) =>
        list.some((t) => t.name === current) ? current : (list[0]?.name ?? ""),
      );
    } catch (err) {
      setTables(null);
      setError(errorText(err));
    } finally {
      setLoading(false);
    }
  };

  // Load tables whenever the connection changes.
  useEffect(() => {
    void loadTables();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  const table = useMemo(
    () => tables?.find((t) => t.name === tableName) ?? null,
    [tables, tableName],
  );

  // Reset column selection / filters when the table changes.
  useEffect(() => {
    if (!table) return;
    const next: Record<string, boolean> = {};
    for (const col of table.columns) next[col.name] = true;
    setCols(next);
    setFilters([]);
    setOrders([]);
  }, [table]);

  const addFilter = () => {
    if (!table) return;
    setFilters((rows) => [
      ...rows,
      { id: ++seq.current, column: table.columns[0]?.name ?? "", op: "=", value: "" },
    ]);
  };

  const addOrder = () => {
    if (!table) return;
    setOrders((rows) => [
      ...rows,
      { id: ++seq.current, column: table.columns[0]?.name ?? "", dir: "ASC" },
    ]);
  };

  const sql = useMemo(() => {
    if (!table) return "";
    const quote = (name: string) => quoteIdent(engine, name);
    const parts: string[] = [];
    if (engine === "postgres") {
      if (schema.trim()) parts.push(schema.trim());
      else if (database.trim()) parts.push(database.trim());
    } else {
      if (database.trim()) parts.push(database.trim());
      else if (schema.trim()) parts.push(schema.trim());
    }
    parts.push(table.name);
    const from = parts.map(quote).join(".");

    const picked = table.columns
      .filter((c) => cols[c.name] ?? true)
      .map((c) => quote(c.name));
    const select = picked.length > 0 ? picked.join(", ") : "*";

    const lines = [`SELECT ${select}`, `FROM ${from}`];
    const where = filters
      .filter((f) => f.column)
      .map((f) => filterSql(engine, f))
      .filter((x): x is string => x !== null);
    if (where.length > 0) lines.push(`WHERE ${where.join("\n  AND ")}`);
    const order = orders
      .filter((o) => o.column)
      .map((o) => `${quote(o.column)} ${o.dir}`);
    if (order.length > 0) lines.push(`ORDER BY ${order.join(", ")}`);
    const lim = Number.parseInt(limit, 10);
    if (Number.isFinite(lim) && lim > 0) lines.push(`LIMIT ${lim}`);
    return `${lines.join("\n")};`;
  }, [table, engine, database, schema, cols, filters, orders, limit]);

  const insert = () => {
    if (!sessionId || !table || !sql) return;
    newQueryTab(sessionId, sql);
    toast("success", `Query inserted for ${table.name}`);
    onClose();
  };

  const field =
    "h-7 rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-800 focus:ring-2 focus:ring-accent/40 focus:outline-none";
  const sectionTitle = "text-[13px] font-semibold text-slate-900";
  const smallButton =
    "h-7 rounded-md border border-slate-300 bg-white px-2 text-[12.5px] text-slate-700 hover:bg-slate-50 disabled:opacity-40";

  return (
    <DialogShell
      title="Query Builder"
      width={780}
      onClose={onClose}
      footer={
        <>
          <button
            className="h-7 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50"
            onClick={onClose}
          >
            Cancel
          </button>
          <button
            className="h-7 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover disabled:opacity-50"
            disabled={!sessionId || !table}
            onClick={insert}
          >
            Insert into editor
          </button>
        </>
      }
    >
      {sessions.length === 0 ? (
        <div className="p-6 text-[13px] text-slate-500">
          Connect to a database first, then reopen the query builder.
        </div>
      ) : (
        <div className="space-y-4 p-3">
          {/* Connection */}
          <section className="space-y-1.5">
            <h3 className={sectionTitle}>1. Connection &amp; scope</h3>
            <div className="flex items-center gap-2">
              <label className="w-20 shrink-0 text-[12px] text-slate-500">Session</label>
              <select
                className={`${field} min-w-0 flex-1`}
                value={sessionId ?? ""}
                onChange={(e) => {
                  setSessionId(e.target.value || null);
                  setTables(null);
                  setTableName("");
                }}
              >
                {sessions.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name} · {s.engine}
                  </option>
                ))}
              </select>
            </div>
            <div className="flex items-center gap-2">
              <label className="w-20 shrink-0 text-[12px] text-slate-500">Database</label>
              <input
                className={`${field} min-w-0 flex-1`}
                placeholder="database (optional)"
                value={database}
                onChange={(e) => setDatabase(e.target.value)}
              />
              <label className="shrink-0 text-[12px] text-slate-500">Schema</label>
              <input
                className={`${field} min-w-0 flex-1`}
                placeholder="schema (optional)"
                value={schema}
                onChange={(e) => setSchema(e.target.value)}
              />
              <button
                className={`${smallButton} flex items-center gap-1`}
                onClick={() => void loadTables()}
                disabled={loading}
                title="Reload tables for this scope"
              >
                <RefreshCw size={12} />
                Load
              </button>
            </div>
          </section>

          {/* Table */}
          <section className="space-y-1.5">
            <h3 className={sectionTitle}>2. Table &amp; columns</h3>
            {loading && <div className="text-[11px] text-slate-400">Loading tables…</div>}
            {error && <div className="text-[11px] text-rose-600">{error}</div>}
            {!loading && !error && tables && tables.length === 0 && (
              <div className="text-[11px] text-slate-400">
                No tables found in this scope.
              </div>
            )}
            {tables && tables.length > 0 && (
              <div className="flex items-center gap-2">
                <label className="w-20 shrink-0 text-[12px] text-slate-500">Table</label>
                <select
                  className={`${field} min-w-0 flex-1`}
                  value={tableName}
                  onChange={(e) => setTableName(e.target.value)}
                >
                  {tables.map((t) => (
                    <option key={t.name} value={t.name}>
                      {t.name}
                      {t.kind === "view" ? " (view)" : ""}
                    </option>
                  ))}
                </select>
              </div>
            )}
            {table && (
              <>
                <div className="flex items-center justify-between">
                  <span className="text-[11px] text-slate-500">
                    {table.columns.length} column(s) — unchecked columns are omitted
                    (all unchecked = <span className="font-code">*</span>)
                  </span>
                  <div className="flex gap-1">
                    <button
                      className={smallButton}
                      onClick={() =>
                        setCols(
                          Object.fromEntries(table.columns.map((c) => [c.name, true])),
                        )
                      }
                    >
                      All
                    </button>
                    <button
                      className={smallButton}
                      onClick={() =>
                        setCols(
                          Object.fromEntries(table.columns.map((c) => [c.name, false])),
                        )
                      }
                    >
                      None
                    </button>
                  </div>
                </div>
                <div className="custom-scrollbar grid max-h-36 grid-cols-2 gap-x-4 gap-y-0.5 overflow-auto rounded-md border border-slate-200 p-2">
                  {table.columns.map((col) => (
                    <label
                      key={col.name}
                      className="flex items-center gap-1.5 text-[12.5px] text-slate-700"
                    >
                      <input
                        type="checkbox"
                        className="accent-sky-600"
                        checked={cols[col.name] ?? true}
                        onChange={(e) =>
                          setCols((m) => ({ ...m, [col.name]: e.target.checked }))
                        }
                      />
                      <span className="truncate font-code">{col.name}</span>
                      <span className="truncate text-[11px] text-slate-400">
                        {col.native_type}
                      </span>
                    </label>
                  ))}
                </div>
              </>
            )}
          </section>

          {/* Filters */}
          {table && (
            <section className="space-y-1.5">
              <div className="flex items-center justify-between">
                <h3 className={sectionTitle}>3. Filters</h3>
                <button className={`${smallButton} flex items-center gap-1`} onClick={addFilter}>
                  <Plus size={12} />
                  Add filter
                </button>
              </div>
              {filters.length === 0 && (
                <div className="text-[11px] text-slate-400">No filters.</div>
              )}
              {filters.map((row, i) => (
                <div key={row.id} className="flex items-center gap-1.5">
                  <select
                    className={`${field} min-w-0 flex-1`}
                    value={row.column}
                    onChange={(e) =>
                      setFilters((rows) =>
                        rows.map((r) => (r.id === row.id ? { ...r, column: e.target.value } : r)),
                      )
                    }
                  >
                    {table.columns.map((c) => (
                      <option key={c.name} value={c.name}>
                        {c.name}
                      </option>
                    ))}
                  </select>
                  <select
                    className={`${field} w-[92px] shrink-0`}
                    value={row.op}
                    onChange={(e) =>
                      setFilters((rows) =>
                        rows.map((r) => (r.id === row.id ? { ...r, op: e.target.value } : r)),
                      )
                    }
                  >
                    {OPERATORS.map((op) => (
                      <option key={op} value={op}>
                        {op}
                      </option>
                    ))}
                  </select>
                  <input
                    className={`${field} min-w-0 flex-1 font-code disabled:bg-slate-50 disabled:text-slate-400`}
                    placeholder={row.op === "IN" ? "a, b, c" : "value"}
                    disabled={isNullOp(row.op)}
                    value={isNullOp(row.op) ? "" : row.value}
                    onChange={(e) =>
                      setFilters((rows) =>
                        rows.map((r) => (r.id === row.id ? { ...r, value: e.target.value } : r)),
                      )
                    }
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && i === filters.length - 1) addFilter();
                    }}
                  />
                  <button
                    className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.05] hover:text-rose-600"
                    title="Remove filter"
                    onClick={() => setFilters((rows) => rows.filter((r) => r.id !== row.id))}
                  >
                    <Trash2 size={12} />
                  </button>
                </div>
              ))}
            </section>
          )}

          {/* Sorting & limit */}
          {table && (
            <section className="space-y-1.5">
              <div className="flex items-center justify-between">
                <h3 className={sectionTitle}>4. Sorting &amp; limit</h3>
                <button className={`${smallButton} flex items-center gap-1`} onClick={addOrder}>
                  <Plus size={12} />
                  Add sort
                </button>
              </div>
              {orders.map((row) => (
                <div key={row.id} className="flex items-center gap-1.5">
                  <select
                    className={`${field} min-w-0 flex-1`}
                    value={row.column}
                    onChange={(e) =>
                      setOrders((rows) =>
                        rows.map((r) => (r.id === row.id ? { ...r, column: e.target.value } : r)),
                      )
                    }
                  >
                    {table.columns.map((c) => (
                      <option key={c.name} value={c.name}>
                        {c.name}
                      </option>
                    ))}
                  </select>
                  <select
                    className={`${field} w-[84px] shrink-0`}
                    value={row.dir}
                    onChange={(e) =>
                      setOrders((rows) =>
                        rows.map((r) =>
                          r.id === row.id ? { ...r, dir: e.target.value as "ASC" | "DESC" } : r,
                        ),
                      )
                    }
                  >
                    <option value="ASC">ASC</option>
                    <option value="DESC">DESC</option>
                  </select>
                  <button
                    className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.05] hover:text-rose-600"
                    title="Remove sort"
                    onClick={() => setOrders((rows) => rows.filter((r) => r.id !== row.id))}
                  >
                    <Trash2 size={12} />
                  </button>
                </div>
              ))}
              <div className="flex items-center gap-2">
                <label className="w-20 shrink-0 text-[12px] text-slate-500">Limit</label>
                <input
                  className={`${field} w-28 font-code`}
                  inputMode="numeric"
                  placeholder="no limit"
                  value={limit}
                  onChange={(e) => setLimit(e.target.value.replace(/[^\d]/g, ""))}
                />
                <span className="text-[11px] text-slate-400">empty = no LIMIT clause</span>
              </div>
            </section>
          )}

          {/* Preview */}
          <section className="space-y-1.5">
            <h3 className={sectionTitle}>Preview</h3>
            <pre className="custom-scrollbar max-h-44 min-h-[60px] overflow-auto rounded-md border border-slate-200 bg-sidebar p-2 font-code text-[12.5px] leading-snug whitespace-pre-wrap text-slate-700">
              {table ? sql : "-- select a table to build a query"}
            </pre>
          </section>
        </div>
      )}
    </DialogShell>
  );
}
