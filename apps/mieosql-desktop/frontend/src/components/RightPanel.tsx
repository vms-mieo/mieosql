import { useEffect, useState } from "react";
import {
  ChevronRight,
  Copy,
  FileCode2,
  KeyRound,
  RefreshCw,
  Table2,
  Trash2,
} from "lucide-react";

import { api, errorText } from "../lib/api";
import type { ColumnDef, IndexDef } from "../lib/bindings";
import { copyText, kindBadge } from "../lib/format";
import type { HistoryEntry } from "../lib/bindings";
import { useStore } from "../lib/store";

export function RightPanel() {
  const store = useStore();
  const tab = store.rightTab;

  return (
    <aside className="flex w-[280px] shrink-0 flex-col bg-sidebar">
      <div className="flex h-9 shrink-0 items-end border-b border-slate-200/80 px-2 select-none">
        {(
          [
            ["object", "Object"],
            ["history", "History"],
            ["snippets", "Snippets"],
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            className={`relative flex-1 pb-1.5 text-center text-[12.5px] transition-colors ${
              tab === key
                ? "font-medium text-slate-900"
                : "text-slate-500 hover:text-slate-700"
            }`}
            onClick={() => store.setRightTab(key)}
          >
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
  const [columns, setColumns] = useState<ColumnDef[] | null>(null);
  const [indexes, setIndexes] = useState<IndexDef[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!selected) return;
    setColumns(null);
    setIndexes([]);
    setError(null);
    let cancelled = false;
    void (async () => {
      try {
        const cols = await api.tableColumns(
          selected.sessionId,
          selected.scope,
          selected.table,
        );
        const idx = await api
          .tableIndexes(selected.sessionId, selected.scope, selected.table)
          .catch(() => []);
        if (!cancelled) {
          setColumns(cols);
          setIndexes(idx);
        }
      } catch (err) {
        if (!cancelled) setError(errorText(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selected]);

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
                <span className="shrink-0 text-[11px] text-slate-400">
                  {kindBadge(c.data_type)}
                </span>
                {!c.nullable && (
                  <span className="shrink-0 rounded bg-slate-900/[0.06] px-1 text-[10.5px] text-slate-600">
                    NN
                  </span>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      {indexes.length > 0 && (
        <div className="mt-3 border-t border-slate-200/80 pt-2">
          <div className="mb-1.5 text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
            Indexes
          </div>
          {indexes.map((idx) => (
            <div key={idx.name} className="px-1 py-0.5">
              <div className="flex items-center gap-1">
                <span className="truncate font-code text-[12px] text-slate-700">
                  {idx.name}
                </span>
                {idx.is_primary && (
                  <span className="rounded bg-amber-100 px-1 text-[10.5px] font-medium text-amber-700">
                    PK
                  </span>
                )}
                {!idx.is_primary && idx.is_unique && (
                  <span className="rounded bg-accent/10 px-1 text-[10.5px] font-medium text-accent">
                    UNIQUE
                  </span>
                )}
              </div>
              <div className="truncate text-[11px] text-slate-500">
                ({idx.columns.join(", ")})
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

function HistoryList() {
  const store = useStore();
  return (
    <div className="custom-scrollbar flex-1 overflow-y-auto px-2 pb-2 text-[11px]">
      <div className="flex items-center justify-between pt-2 pb-1 pl-1">
        <span className="text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
          Recent
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
      {store.history.length === 0 && (
        <div className="p-2 text-[12px] leading-relaxed text-slate-500">
          Executed queries will show up here.
        </div>
      )}
      {store.history.map((h) => (
        <HistoryItem key={h.id} entry={h} />
      ))}
    </div>
  );
}

function HistoryItem({ entry }: { entry: HistoryEntry }) {
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
      <div className="truncate font-code text-[12px] text-slate-700">
        {entry.sql.replace(/\s+/g, " ").slice(0, 90)}
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
