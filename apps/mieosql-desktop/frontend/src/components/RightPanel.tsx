import { useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronRight,
  Code2,
  Copy,
  FileCode2,
  History,
  KeyRound,
  PanelRight,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Star,
  Table2,
  Trash2,
  Zap,
} from "lucide-react";

import { api, errorText } from "../lib/api";
import type { ColumnDef, IndexDef, ObjectRef } from "../lib/bindings";
import { confirmDialog } from "../lib/dialog";
import { copyText } from "../lib/format";
import {
  buildDropIndex,
  supportsIndexEditor,
} from "../lib/index-ddl";
import {
  buildDropTrigger,
  supportsTriggers,
  triggerTable,
  triggerTemplate,
} from "../lib/trigger-ddl";
import type { HistoryEntry } from "../lib/bindings";
import { useStore } from "../lib/store";
import { IndexDialog } from "./dialogs/IndexDialog";

export function RightPanel() {
  const store = useStore();
  const tab = store.rightTab;

  return (
    <aside className="flex w-[280px] shrink-0 flex-col border-l border-hairline bg-inspector">
      <div className="flex h-9 shrink-0 items-end border-b border-slate-200/80 px-2 select-none">
        {(
          [
            ["object", "Object", PanelRight],
            ["history", "History", History],
            ["snippets", "Snippets", Code2],
          ] as const
        ).map(([key, label, Icon]) => (
          <button
            key={key}
            className={`relative flex flex-1 items-center justify-center gap-1.5 pb-1.5 text-[13px] transition-colors ${
              tab === key
                ? "font-medium text-slate-900"
                : "text-slate-500 hover:text-slate-700"
            }`}
            onClick={() => store.setRightTab(key)}
          >
            <Icon
              size={12}
              strokeWidth={1.6}
              className={tab === key ? "" : "text-slate-400"}
            />
            {label}
            {tab === key && (
              <span className="absolute inset-x-2 bottom-0 h-[2px] rounded-full bg-accent" />
            )}
          </button>
        ))}
      </div>

      {tab === "object" && <ObjectInspector />}
      {tab === "history" && <HistoryList />}
      {tab === "snippets" && <Snippets />}
    </aside>
  );
}

// ---------------------------------------------------------------------------
// Object inspector (columns / indexes / DDL of the selected object)
// ---------------------------------------------------------------------------

function ObjectInspector() {
  const store = useStore();
  const selected = store.selected;
  const session = store.sessions.find((s) => s.id === selected?.sessionId);
  const engine = session?.engine ?? null;
  const [columns, setColumns] = useState<ColumnDef[] | null>(null);
  const [indexes, setIndexes] = useState<IndexDef[]>([]);
  const [triggers, setTriggers] = useState<ObjectRef[] | null>(null);
  const [indexDialog, setIndexDialog] = useState<{ editing: IndexDef | null } | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);
  const reload = () => setReloadToken((t) => t + 1);

  const selectedKey = selected
    ? `${selected.sessionId}|${selected.scope.database ?? ""}|${selected.scope.schema ?? ""}|${selected.table}`
    : "";
  const prevKey = useRef("");

  useEffect(() => {
    if (!selected) return;
    setError(null);
    if (prevKey.current !== selectedKey) {
      // Different object: clear stale rows instead of flashing them.
      prevKey.current = selectedKey;
      setColumns(null);
      setIndexes([]);
      setTriggers(null);
    }
    let cancelled = false;
    const wantsTriggers =
      selected.kind === "table" && supportsTriggers(session?.engine);
    void (async () => {
      try {
        const [cols, idx, allTriggers] = await Promise.all([
          api.tableColumns(selected.sessionId, selected.scope, selected.table),
          api
            .tableIndexes(selected.sessionId, selected.scope, selected.table)
            .catch(() => [] as IndexDef[]),
          wantsTriggers
            ? api
                .listObjects(
                  selected.sessionId,
                  selected.scope,
                  "trigger",
                  null,
                )
                .catch(() => [] as ObjectRef[])
            : Promise.resolve([] as ObjectRef[]),
        ]);
        if (cancelled) return;
        setColumns(cols);
        setIndexes(idx);
        if (wantsTriggers) {
          const tableLc = selected.table.toLowerCase();
          setTriggers(
            allTriggers.filter(
              (t) => triggerTable(t.comment)?.toLowerCase() === tableLc,
            ),
          );
        } else {
          setTriggers([]);
        }
      } catch (err) {
        if (!cancelled) setError(errorText(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selectedKey, reloadToken, session?.engine, selected]);

  if (!selected) {
    return (
      <div className="flex flex-1 items-center justify-center p-6 text-center text-[12px] leading-relaxed text-slate-500">
        Select a table in the connection tree to inspect its columns and indexes.
      </div>
    );
  }

  const openDdl = async () => {
    if (!selected) return;
    try {
      const ddl = await api.objectDdl(
        selected.sessionId,
        selected.scope,
        selected.kind,
        selected.table,
      );
      if (ddl) {
        store.newQueryTab(selected.sessionId, ddl);
      } else {
        store.toast("info", "DDL is not available for this object yet");
      }
    } catch (err) {
      store.toast("error", errorText(err));
    }
  };

  const dropIndex = async (idx: IndexDef) => {
    if (!selected || !engine) return;
    const sql = buildDropIndex(engine, selected.scope, selected.table, idx.name);
    if (!sql) return;
    const ok = await confirmDialog(
      `Drop index “${idx.name}”? This cannot be undone.`,
      { title: "Drop index", kind: "warning" },
    );
    if (!ok) return;
    try {
      const result = await api.executeBatch(selected.sessionId, [sql], false);
      if (result.failed > 0) {
        store.toast("error", result.errors[0] ?? "Drop index failed");
        return;
      }
      store.toast("success", `Index “${idx.name}” dropped`);
      reload();
    } catch (err) {
      store.toast("error", errorText(err));
    }
  };

  const viewTriggerDdl = async (trigger: ObjectRef) => {
    if (!selected) return;
    try {
      const ddl = await api.objectDdl(
        selected.sessionId,
        selected.scope,
        "trigger",
        trigger.name,
      );
      if (ddl) store.newQueryTab(selected.sessionId, ddl);
      else store.toast("info", "Trigger DDL is not available on this engine");
    } catch (err) {
      store.toast("error", errorText(err));
    }
  };

  const dropTrigger = async (trigger: ObjectRef) => {
    if (!selected || !engine) return;
    const sql = buildDropTrigger(
      engine,
      selected.scope,
      selected.table,
      trigger.name,
    );
    if (!sql) return;
    const ok = await confirmDialog(
      `Drop trigger “${trigger.name}”? This cannot be undone.`,
      { title: "Drop trigger", kind: "warning" },
    );
    if (!ok) return;
    try {
      const result = await api.executeBatch(selected.sessionId, [sql], false);
      if (result.failed > 0) {
        store.toast("error", result.errors[0] ?? "Drop trigger failed");
        return;
      }
      store.toast("success", `Trigger “${trigger.name}” dropped`);
      reload();
    } catch (err) {
      store.toast("error", errorText(err));
    }
  };

  const newTrigger = () => {
    if (!selected || !engine) return;
    const template = triggerTemplate(engine, selected.scope, selected.table);
    if (!template) {
      store.toast("info", "Triggers are not supported for this engine");
      return;
    }
    store.newQueryTab(selected.sessionId, template);
  };

  return (
    <div className="custom-scrollbar flex-1 overflow-y-auto p-3 text-[11px]">
      <div className="flex items-start gap-2.5">
        <div className="grid h-9 w-9 shrink-0 place-items-center rounded-lg bg-accent text-white">
          <Table2 size={17} />
        </div>
        <div className="min-w-0 pt-0.5">
          <div className="truncate text-[15px] font-semibold text-slate-900">
            {selected.table}
          </div>
          <div className="text-[11px] text-slate-500">
            {selected.kind}
            {selected.scope.schema ? ` · ${selected.scope.schema}` : ""}
            {selected.scope.database ? ` · ${selected.scope.database}` : ""}
          </div>
        </div>
      </div>

      <div className="mt-3 flex gap-1.5">
        {selected.kind === "function" ||
        selected.kind === "procedure" ||
        selected.kind === "trigger" ||
        selected.kind === "sequence" ? (
          <button
            className="flex h-7 items-center gap-1 rounded-md bg-accent px-2.5 text-[12.5px] font-medium text-white hover:bg-accent-hover"
            onClick={() => void openDdl()}
          >
            <FileCode2 size={11} /> View Definition
          </button>
        ) : (
          <>
            <button
              className="flex h-7 items-center gap-1 rounded-md bg-accent px-2.5 text-[12.5px] font-medium text-white hover:bg-accent-hover"
              onClick={() =>
                store.openTable(selected.sessionId, selected.scope, selected.table)
              }
            >
              Open Data
            </button>
            <button
              className="flex h-7 items-center gap-1 rounded-md border border-slate-300 bg-white px-2.5 text-[12.5px] text-slate-700 hover:bg-slate-50"
              onClick={() => void openDdl()}
            >
              <FileCode2 size={11} /> DDL
            </button>
          </>
        )}
        <button
          className="grid h-7 w-7 place-items-center rounded-md border border-slate-300 bg-white text-slate-500 hover:bg-slate-50 hover:text-slate-700"
          onClick={() => {
            void copyText(selected.table).then((ok) =>
              store.toast(ok ? "success" : "error", ok ? "Copied" : "Copy failed"),
            );
          }}
        >
          <Copy size={11} />
        </button>
      </div>

      {session && (
        <div className="mt-3 space-y-1 border-t border-slate-200/80 pt-2 text-[12px] text-slate-500">
          <div>
            Engine: <span className="text-slate-700">{session.engine}</span>
          </div>
          <div>
            Session: <span className="text-slate-700">{session.name}</span>
          </div>
          {session.database && (
            <div>
              Database: <span className="text-slate-700">{session.database}</span>
            </div>
          )}
        </div>
      )}

      <div className="mt-3 border-t border-slate-200/80 pt-2">
        <div className="mb-1.5 text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
          Columns
        </div>
        {error && <div className="text-[11px] text-rose-600">{error}</div>}
        {!columns && !error && (
          <div className="text-[11px] text-slate-500">Loading…</div>
        )}
        {columns && (
          <div className="space-y-0.5">
            {columns.map((c) => (
              <div
                key={c.name}
                className="flex items-center gap-1.5 rounded-md px-1 py-0.5 hover:bg-slate-900/[0.04]"
              >
                {c.is_primary_key ? (
                  <KeyRound size={10} className="shrink-0 text-amber-500" />
                ) : (
                  <span className="w-[10px] shrink-0" />
                )}
                <span className="min-w-0 flex-1 truncate font-code text-[12px] text-slate-700">
                  {c.name}
                </span>
                {!c.nullable && (
                  <span className="shrink-0 text-[10.5px] text-slate-400" title="NOT NULL">
                    NN
                  </span>
                )}
                <span
                  className="w-[86px] shrink-0 truncate text-right font-code text-[10.5px] text-slate-400"
                  title={c.native_type}
                >
                  {c.native_type}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>

      {(indexes.length > 0 || supportsIndexEditor(engine)) && (
        <div className="mt-3 border-t border-slate-200/80 pt-2">
          <div className="mb-1.5 flex items-center justify-between">
            <span className="text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
              Indexes
            </span>
            {supportsIndexEditor(engine) && (
              <button
                className="grid h-5 w-5 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-700"
                title="Add index"
                onClick={() => setIndexDialog({ editing: null })}
              >
                <Plus size={12} strokeWidth={1.8} />
              </button>
            )}
          </div>
          {indexes.length === 0 && (
            <div className="px-1 text-[12px] text-slate-500">No indexes.</div>
          )}
          {indexes.map((idx) => (
            <div
              key={idx.name}
              className="group flex items-start gap-1 rounded-md px-1 py-0.5 hover:bg-slate-900/[0.04]"
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1">
                  <span className="truncate font-code text-[12px] text-slate-700">
                    {idx.name}
                  </span>
                  {idx.is_primary && (
                    <span className="shrink-0 rounded bg-amber-100 px-1 text-[10.5px] font-medium text-amber-700">
                      PK
                    </span>
                  )}
                  {!idx.is_primary && idx.is_unique && (
                    <span className="shrink-0 rounded bg-accent/10 px-1 text-[10.5px] font-medium text-accent">
                      UNIQUE
                    </span>
                  )}
                </div>
                <div className="truncate text-[11px] text-slate-500">
                  ({idx.columns.join(", ")})
                </div>
              </div>
              {!idx.is_primary && supportsIndexEditor(engine) && (
                <span className="hidden shrink-0 items-center gap-0.5 pt-0.5 group-hover:flex">
                  <button
                    className="grid h-5 w-5 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
                    title="Edit index (drop + recreate)"
                    onClick={() => setIndexDialog({ editing: idx })}
                  >
                    <Pencil size={11} />
                  </button>
                  <button
                    className="grid h-5 w-5 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-rose-600"
                    title="Drop index"
                    onClick={() => void dropIndex(idx)}
                  >
                    <Trash2 size={11} />
                  </button>
                </span>
              )}
            </div>
          ))}
        </div>
      )}

      {selected.kind === "table" && supportsTriggers(engine) && (
        <div className="mt-3 border-t border-slate-200/80 pt-2">
          <div className="mb-1.5 flex items-center justify-between">
            <span className="text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
              Triggers
            </span>
            <button
              className="grid h-5 w-5 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-700"
              title="New trigger (opens a query tab with a template)"
              onClick={newTrigger}
            >
              <Plus size={12} strokeWidth={1.8} />
            </button>
          </div>
          {triggers === null && (
            <div className="px-1 text-[12px] text-slate-500">Loading…</div>
          )}
          {triggers?.length === 0 && (
            <div className="px-1 text-[12px] text-slate-500">
              No triggers on this table.
            </div>
          )}
          {triggers?.map((t) => (
            <div
              key={t.name}
              className="group flex items-start gap-1 rounded-md px-1 py-0.5 hover:bg-slate-900/[0.04]"
            >
              <Zap size={11} className="mt-0.5 shrink-0 text-slate-400" />
              <div className="min-w-0 flex-1">
                <div className="truncate font-code text-[12px] text-slate-700">
                  {t.name}
                </div>
                {t.comment && (
                  <div className="truncate text-[11px] text-slate-500">
                    {t.comment}
                  </div>
                )}
              </div>
              <span className="hidden shrink-0 items-center gap-0.5 pt-0.5 group-hover:flex">
                <button
                  className="grid h-5 w-5 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
                  title="View trigger DDL"
                  onClick={() => void viewTriggerDdl(t)}
                >
                  <FileCode2 size={11} />
                </button>
                <button
                  className="grid h-5 w-5 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-rose-600"
                  title="Drop trigger"
                  onClick={() => void dropTrigger(t)}
                >
                  <Trash2 size={11} />
                </button>
              </span>
            </div>
          ))}
        </div>
      )}

      {indexDialog && session && (
        <IndexDialog
          sessionId={selected.sessionId}
          engine={session.engine}
          scope={selected.scope}
          table={selected.table}
          columns={columns ?? []}
          editing={indexDialog.editing}
          onClose={() => setIndexDialog(null)}
          onApplied={reload}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

function HistoryList() {
  const store = useStore();
  const [query, setQuery] = useState("");
  const [starred, setStarred] = useState<Set<string>>(loadStarredHistory);

  const toggleStar = (id: string) => {
    setStarred((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      try {
        localStorage.setItem(STARRED_HISTORY_KEY, JSON.stringify([...next]));
      } catch {
        // Best-effort only.
      }
      return next;
    });
  };

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? store.history.filter((h) =>
          `${h.sql}\n${h.database ?? ""}\n${h.engine}`
            .toLowerCase()
            .includes(q),
        )
      : store.history.slice();
    // Starred entries sort first, keeping backend order otherwise.
    filtered.sort((a, b) => {
      const sa = starred.has(a.id) ? 0 : 1;
      const sb = starred.has(b.id) ? 0 : 1;
      return sa - sb;
    });
    return filtered;
  }, [store.history, query, starred]);

  return (
    <div className="custom-scrollbar flex-1 overflow-y-auto px-2 pb-2 text-[11px]">
      <div className="flex items-center justify-between pt-2 pb-1 pl-1">
        <span className="text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
          Recent{" "}
          <span
            className="font-normal normal-case"
            title="Auto-delete history older than N days (Settings → Data)"
          >
            · keep {store.settings.historyRetentionDays === 0 ? "∞" : `${store.settings.historyRetentionDays}d`}
          </span>
        </span>
        <div className="flex items-center gap-0.5">
          <button
            className="grid h-5 w-5 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
            title="Refresh"
            onClick={() => void store.refreshHistory()}
          >
            <RefreshCw size={11} />
          </button>
          <button
            className="grid h-5 w-5 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-rose-600"
            title="Clear history"
            onClick={() => void store.clearHistory()}
          >
            <Trash2 size={11} />
          </button>
        </div>
      </div>
      <div className="relative mb-1">
        <Search
          size={11}
          className="pointer-events-none absolute top-1/2 left-2 -translate-y-1/2 text-slate-400"
        />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search sql, database, engine…"
          className="h-6 w-full rounded-md border border-slate-200 bg-white pr-2 pl-7 text-[12px] text-slate-700 placeholder:text-slate-400 focus:ring-2 focus:ring-accent/40 focus:outline-none"
        />
      </div>
      {visible.length === 0 && store.history.length > 0 && (
        <div className="p-2 text-[12px] leading-relaxed text-slate-500">
          No history matches “{query.trim()}”.
        </div>
      )}
      {store.history.length === 0 && (
        <div className="p-2 text-[12px] leading-relaxed text-slate-500">
          Executed queries will show up here.
        </div>
      )}
      {visible.map((h) => (
        <HistoryItem
          key={h.id}
          entry={h}
          starred={starred.has(h.id)}
          onToggleStar={() => toggleStar(h.id)}
        />
      ))}
    </div>
  );
}

/** History ids the user starred, persisted across restarts. */
const STARRED_HISTORY_KEY = "mieosql.history.starred.v1";

function loadStarredHistory(): Set<string> {
  try {
    const raw = localStorage.getItem(STARRED_HISTORY_KEY);
    if (!raw) return new Set();
    const parsed: unknown = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return new Set(parsed.filter((x): x is string => typeof x === "string"));
    }
  } catch {
    // Corrupt value: start fresh.
  }
  return new Set();
}

function HistoryItem({
  entry,
  starred,
  onToggleStar,
}: {
  entry: HistoryEntry;
  starred: boolean;
  onToggleStar: () => void;
}) {
  const store = useStore();
  const open = () => {
    const live = store.sessions.find((s) => s.id === entry.session_id);
    store.newQueryTab(live?.id ?? null, entry.sql);
  };
  const statusColor =
    entry.status === "ok"
      ? "text-emerald-600"
      : entry.status === "cancelled"
        ? "text-amber-600"
        : "text-rose-600";
  const ago = formatAgo(new Date(entry.started_at).getTime());
  return (
    <div
      className="group cursor-default space-y-0.5 rounded-md p-2 hover:bg-slate-900/[0.04]"
      onClick={open}
      title="Open in a new query tab"
    >
      <div className="flex items-center justify-between">
        <span className={`flex items-center gap-1 text-[11px] font-semibold ${statusColor}`}>
          <span className="inline-block h-1.5 w-1.5 rounded-full bg-current" />
          {entry.status === "ok"
            ? "OK"
            : entry.status === "cancelled"
              ? "CANCELLED"
              : "ERROR"}
          <span className="font-normal text-slate-400">
            {entry.elapsed_ms}ms
          </span>
        </span>
        <span className="text-[11px] text-slate-400">{ago}</span>
      </div>
      <div className="flex min-w-0 items-center gap-1">
        <button
          className={`grid h-5 w-5 shrink-0 place-items-center rounded-md hover:bg-slate-900/[0.06] ${
            starred ? "text-amber-500" : "text-slate-300 hover:text-slate-500"
          }`}
          title={starred ? "Unstar" : "Star (pin to top)"}
          onClick={(e) => {
            e.stopPropagation();
            onToggleStar();
          }}
        >
          <Star size={11} fill={starred ? "currentColor" : "none"} />
        </button>
        <FileCode2 size={11} className="shrink-0 text-slate-400" />
        <span className="truncate font-code text-[12px] text-slate-700">
          {entry.sql.replace(/\s+/g, " ").slice(0, 90)}
        </span>
      </div>
      <div className="flex items-center justify-between text-[11px] text-slate-400">
        <span>
          {entry.rows.toLocaleString()} rows
          {entry.rows_affected > 0
            ? ` · ${entry.rows_affected} affected`
            : ""}
        </span>
        <span className="flex items-center gap-1">
          {entry.database ?? ""}
          <ChevronRight size={9} className="opacity-0 group-hover:opacity-100" />
        </span>
      </div>
      {entry.error && (
        <div className="truncate text-[11px] text-rose-500">
          {entry.error.message}
        </div>
      )}
    </div>
  );
}

function formatAgo(ts: number): string {
  const diff = Date.now() - ts;
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  return `${Math.floor(diff / 86_400_000)}d ago`;
}

// ---------------------------------------------------------------------------
// Snippets
// ---------------------------------------------------------------------------

const SNIPPETS: { label: string; sql: string }[] = [
  {
    label: "Count rows by group",
    sql: "-- Count rows by group\nSELECT group_column, COUNT(*) AS total\nFROM table_name\nGROUP BY group_column\nORDER BY total DESC;",
  },
  {
    label: "Show table columns",
    sql: "-- Introspect a table\nSELECT column_name, data_type, is_nullable\nFROM information_schema.columns\nWHERE table_name = 'table_name'\nORDER BY ordinal_position;",
  },
  {
    label: "Recent rows",
    sql: "SELECT *\nFROM table_name\nORDER BY id DESC\nLIMIT 100;",
  },
  {
    label: "Postgres: activity",
    sql: "SELECT pid, state, query, now() - query_start AS duration\nFROM pg_stat_activity\nWHERE state <> 'idle'\nORDER BY duration DESC;",
  },
  {
    label: "MySQL: process list",
    sql: "SHOW FULL PROCESSLIST;",
  },
  {
    label: "SQLite: schema",
    sql: "SELECT type, name, sql\nFROM sqlite_master\nWHERE type IN ('table','index')\nORDER BY name;",
  },
];

function Snippets() {
  const store = useStore();
  return (
    <div className="custom-scrollbar flex-1 overflow-y-auto px-2 pb-2 text-[11px]">
      <div className="px-1 pt-2 pb-1 text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
        Quick SQL snippets
      </div>
      {SNIPPETS.map((s) => (
        <div
          key={s.label}
          className="group flex cursor-default items-center justify-between gap-2 rounded-md p-2 hover:bg-slate-900/[0.04]"
          onClick={() => store.newQueryTab(null, s.sql)}
          title="Open in a new query tab"
        >
          <span className="truncate text-[12.5px] text-slate-700">{s.label}</span>
          <Copy
            size={11}
            className="shrink-0 text-slate-400 opacity-0 group-hover:opacity-100"
          />
        </div>
      ))}
    </div>
  );
}
