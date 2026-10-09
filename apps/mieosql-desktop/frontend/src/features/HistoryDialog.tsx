// Query history panel: searchable list of past executions (Beekeeper/DbGate/
// Sequel Ace all autosave every run). Clicking an entry opens its SQL in a
// new query tab bound to the same live session when still open.

import { useEffect, useMemo, useState } from "react";
import { History, Loader2, Play, Search, TriangleAlert } from "lucide-react";

import type { HistoryEntry } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { useStore } from "../lib/store";
import { DialogShell } from "./DialogShell";

export function HistoryDialog({ onClose }: { onClose: () => void }) {
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");

  useEffect(() => {
    let cancelled = false;
    void api
      .listHistory(200)
      .then((list) => {
        if (!cancelled) setEntries(list);
      })
      .catch((err) => {
        if (!cancelled) setError(errorText(err));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const filtered = useMemo(() => {
    if (!entries) return [];
    const q = query.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter(
      (e) =>
        e.sql.toLowerCase().includes(q) ||
        (e.database ?? "").toLowerCase().includes(q),
    );
  }, [entries, query]);

  const reopen = (entry: HistoryEntry) => {
    const st = useStore.getState();
    const live = st.sessions.some((s) => s.id === entry.session_id);
    const tabId = st.newQueryTab(live ? entry.session_id : undefined, entry.sql);
    if (entry.database) st.setTabScope(tabId, { database: entry.database });
    onClose();
  };

  return (
    <DialogShell title="Query history" width={720} onClose={onClose}>
      <div className="flex flex-col gap-2 p-4">
        <div className="flex h-8 shrink-0 items-center gap-2 rounded-md bg-slate-900/[0.04] px-2.5">
          <Search size={13} className="shrink-0 text-slate-400" />
          <input
            autoFocus
            className="w-full border-none bg-transparent p-0 text-[13px] placeholder:text-slate-400 focus:outline-none"
            placeholder="Search SQL or database…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        {error && (
          <div className="flex items-center gap-2 rounded-md bg-rose-50 px-3 py-2 text-[12.5px] text-rose-700">
            <TriangleAlert size={13} className="shrink-0" />
            {error}
          </div>
        )}
        {entries === null && !error && (
          <div className="flex items-center gap-2 px-1 py-6 text-[13px] text-slate-400">
            <Loader2 size={14} className="animate-spin" /> Loading history…
          </div>
        )}
        {entries !== null && filtered.length === 0 && (
          <div className="flex flex-col items-center gap-1 px-1 py-8 text-center">
            <History size={20} className="text-slate-300" />
            <div className="text-[13px] text-slate-500">
              {entries.length === 0 ? "No queries run yet." : "No matches."}
            </div>
          </div>
        )}
        {filtered.map((e) => (
          <button
            key={e.id}
            className="group flex min-w-0 items-start gap-2.5 rounded-md border border-transparent px-2.5 py-2 text-left hover:border-slate-200 hover:bg-slate-900/[0.03]"
            onClick={() => reopen(e)}
            title="Open in a new query tab"
          >
            <Play
              size={13}
              className="mt-1 shrink-0 text-slate-300 group-hover:text-accent"
            />
            <span className="min-w-0 flex-1">
              <span className="font-code block truncate text-[12.5px] text-slate-800">
                {e.sql.split("\n").find((l) => l.trim()) ?? e.sql}
              </span>
              <span className="mt-0.5 block text-[11.5px] text-slate-400">
                {e.database ?? "no db"} · {e.engine} ·{" "}
                {new Date(e.started_at).toLocaleString()} · {e.elapsed_ms} ms ·{" "}
                {e.rows.toLocaleString()} rows
                {e.error ? " · failed" : ""}
              </span>
            </span>
            {e.error && (
              <TriangleAlert size={13} className="mt-1 shrink-0 text-rose-400" />
            )}
          </button>
        ))}
      </div>
    </DialogShell>
  );
}
