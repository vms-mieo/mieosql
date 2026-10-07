import { useEffect, useState } from "react";
import {
  ChevronRight,
  Copy,
  FileCode2,
  KeyRound,
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
    <aside className="flex w-[272px] shrink-0 flex-col border-l border-[#cbd5e1] bg-[#f8fafc]">
      <div className="flex h-[30px] shrink-0 items-center border-b border-slate-200 bg-[#eef2f7] px-1 text-[11px] select-none">
        {(
          [
            ["object", "Object"],
            ["history", "History"],
            ["snippets", "Snippets"],
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            className={`flex-1 rounded-t py-1 text-center ${
              tab === key
                ? "bg-white font-medium text-sky-700 shadow-sm"
                : "text-slate-500 hover:text-slate-700"
            }`}
            onClick={() => store.setRightTab(key)}
          >
            {label}
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
      <div className="flex flex-1 items-center justify-center p-4 text-center text-[11px] text-slate-400">
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
      <div className="flex items-start gap-2">
        <div className="grid h-9 w-9 shrink-0 place-items-center rounded-lg bg-sky-500 text-white shadow-sm">
          <Table2 size={17} />
        </div>
        <div className="min-w-0">
          <div className="truncate text-[12px] font-bold text-slate-800">
            {selected.table}
          </div>
          <div className="text-[10px] text-slate-400">
            {selected.kind}
            {selected.scope.schema ? ` · ${selected.scope.schema}` : ""}
            {selected.scope.database ? ` · ${selected.scope.database}` : ""}
          </div>
        </div>
      </div>

      <div className="mt-3 flex gap-1.5">
        <button
          className="flex items-center gap-1 rounded bg-sky-600 px-2 py-1 text-[10px] font-medium text-white hover:bg-sky-700"
          onClick={() =>
            store.openTable(selected.sessionId, selected.scope, selected.table)
          }
        >
          Open Data
        </button>
        <button
          className="flex items-center gap-1 rounded border border-slate-300 bg-white px-2 py-1 text-[10px] text-slate-600 hover:bg-slate-100"
          onClick={() => void openDdl()}
        >
          <FileCode2 size={11} /> DDL
        </button>
        <button
          className="flex items-center gap-1 rounded border border-slate-300 bg-white px-2 py-1 text-[10px] text-slate-600 hover:bg-slate-100"
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
        <div className="mt-3 space-y-1 border-t border-slate-200 pt-2 text-[10px] text-slate-500">
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

      <div className="mt-3 border-t border-slate-200 pt-2">
        <div className="mb-1 text-[10px] font-semibold tracking-wider text-slate-400 uppercase">
          Columns
        </div>
        {error && <div className="text-[10px] text-rose-600">{error}</div>}
        {!columns && !error && (
          <div className="text-[10px] text-slate-400">Loading…</div>
        )}
        {columns && (
          <div className="space-y-0.5">
            {columns.map((c) => (
              <div
                key={c.name}
                className="flex items-center gap-1.5 rounded px-1 py-0.5 hover:bg-slate-100"
              >
                {c.is_primary_key ? (
                  <KeyRound size={10} className="shrink-0 text-amber-500" />
                ) : (
                  <span className="w-[10px] shrink-0" />
                )}
                <span className="min-w-0 flex-1 truncate font-code text-[10px] text-slate-700">
                  {c.name}
                </span>
                <span className="shrink-0 text-[9px] text-slate-400">
                  {kindBadge(c.data_type)}
                </span>
                {!c.nullable && (
                  <span className="shrink-0 rounded bg-slate-200 px-1 text-[8px] text-slate-500">
                    NN
                  </span>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      {indexes.length > 0 && (
        <div className="mt-3 border-t border-slate-200 pt-2">
          <div className="mb-1 text-[10px] font-semibold tracking-wider text-slate-400 uppercase">
            Indexes
          </div>
          {indexes.map((idx) => (
            <div key={idx.name} className="px-1 py-0.5">
              <div className="flex items-center gap-1">
                <span className="truncate font-code text-[10px] text-slate-700">
                  {idx.name}
                </span>
                {idx.is_primary && (
                  <span className="rounded bg-amber-100 px-1 text-[8px] text-amber-700">
                    PK
                  </span>
                )}
                {!idx.is_primary && idx.is_unique && (
                  <span className="rounded bg-sky-100 px-1 text-[8px] text-sky-700">
                    UNIQUE
                  </span>
                )}
              </div>
              <div className="truncate text-[9px] text-slate-400">
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
    <div className="custom-scrollbar flex-1 overflow-y-auto p-2 text-[11px]">
      <div className="mb-1 flex items-center justify-between px-1">
        <span className="text-[10px] font-semibold tracking-wider text-slate-400 uppercase">
          Recent executions
        </span>
        <div className="flex items-center gap-1">
          <button
            className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-600"
            title="Refresh"
            onClick={() => void store.refreshHistory()}
          >
            ↻
          </button>
          <button
            className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-rose-600"
            title="Clear history"
            onClick={() => void store.clearHistory()}
          >
            <Trash2 size={11} />
          </button>
        </div>
      </div>
      {store.history.length === 0 && (
        <div className="p-2 text-[10px] text-slate-400">
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
      className="group cursor-pointer space-y-0.5 rounded p-1.5 hover:bg-slate-100"
      onClick={open}
      title="Open in a new query tab"
    >
      <div className="flex items-center justify-between">
        <span className={`flex items-center gap-1 text-[10px] font-semibold ${statusColor}`}>
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
        <span className="text-[9px] text-slate-400">{ago}</span>
      </div>
      <div className="truncate font-code text-[10px] text-slate-600">
        {entry.sql.replace(/\s+/g, " ").slice(0, 90)}
      </div>
      <div className="flex items-center justify-between text-[9px] text-slate-400">
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
        <div className="truncate text-[9px] text-rose-500">
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
    <div className="custom-scrollbar flex-1 overflow-y-auto p-2 text-[11px]">
      <div className="mb-1 px-1 text-[10px] font-semibold tracking-wider text-slate-400 uppercase">
        Quick SQL snippets
      </div>
      {SNIPPETS.map((s) => (
        <div
          key={s.label}
          className="group flex cursor-pointer items-center justify-between gap-2 rounded p-1.5 hover:bg-slate-100"
          onClick={() => store.newQueryTab(null, s.sql)}
          title="Open in a new query tab"
        >
          <span className="truncate text-slate-700">{s.label}</span>
          <Copy
            size={11}
            className="shrink-0 text-slate-400 opacity-0 group-hover:opacity-100"
          />
        </div>
      ))}
    </div>
  );
}
