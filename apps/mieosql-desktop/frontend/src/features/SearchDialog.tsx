// F2 — Database-wide Search dialog ("Find in Database").
//
// The form maps 1:1 onto `SearchRequest`; results are grouped by
// database/schema. Clicking a hit opens the object in a table tab and
// closes the dialog.

import { useMemo, useState } from "react";
import {
  Database,
  Eye,
  FileText,
  Loader2,
  Search,
  Table2,
  TriangleAlert,
} from "lucide-react";

import type {
  ObjectKind,
  SearchHit,
  SearchMatchMode,
  SearchRequest,
  SearchResponse,
} from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { useStore } from "../lib/store";
import { DialogShell } from "./DialogShell";

const LIMITS = [100, 500, 2000] as const;

const MODES: Array<{ value: SearchMatchMode; label: string }> = [
  { value: "contains", label: "Contains" },
  { value: "prefix", label: "Prefix" },
  { value: "whole_word", label: "Whole word" },
  { value: "regex", label: "Regex" },
];

const KIND_STYLE: Record<ObjectKind, string> = {
  table: "bg-sky-100 text-sky-700",
  view: "bg-violet-100 text-violet-700",
  materialized_view: "bg-violet-100 text-violet-700",
  function: "bg-amber-100 text-amber-700",
  procedure: "bg-amber-100 text-amber-700",
  trigger: "bg-rose-100 text-rose-700",
  sequence: "bg-emerald-100 text-emerald-700",
};

const KIND_LABEL: Record<ObjectKind, string> = {
  table: "TABLE",
  view: "VIEW",
  materialized_view: "MAT VIEW",
  function: "FUNCTION",
  procedure: "PROC",
  trigger: "TRIGGER",
  sequence: "SEQUENCE",
};

const FIELD_LABEL: Record<SearchHit["matched_in"], string> = {
  name: "Name",
  column: "Column",
  definition: "Definition",
};

const FIELD_STYLE: Record<SearchHit["matched_in"], string> = {
  name: "bg-slate-100 text-slate-600",
  column: "bg-emerald-100 text-emerald-700",
  definition: "bg-amber-100 text-amber-700",
};

type HitGroup = { key: string; label: string; hits: SearchHit[] };

export function SearchDialog({ onClose }: { onClose: () => void }) {
  const sessions = useStore((s) => s.sessions);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const activeTab = useStore(
    (s) => s.tabs.find((t) => t.id === s.activeTabId) ?? null,
  );
  const openTable = useStore((s) => s.openTable);

  // Session: active tab first, then the global active session, then any.
  const preferredId = activeTab?.sessionId ?? activeSessionId;
  const session =
    sessions.find((s) => s.id === preferredId) ?? sessions[0] ?? null;

  const [pattern, setPattern] = useState("");
  const [lastPattern, setLastPattern] = useState("");
  const [mode, setMode] = useState<SearchMatchMode>("contains");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [includeColumns, setIncludeColumns] = useState(false);
  const [includeDefinitions, setIncludeDefinitions] = useState(false);
  const [limit, setLimit] = useState<number>(500);
  const [running, setRunning] = useState(false);
  const [response, setResponse] = useState<SearchResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const canSearch = !!session && pattern.trim().length > 0 && !running;

  const run = async () => {
    if (!session || !pattern.trim() || running) return;
    const request: SearchRequest = {
      pattern: pattern.trim(),
      case_sensitive: caseSensitive,
      mode,
      kinds: [],
      databases: [],
      schemas: [],
      include_columns: includeColumns,
      include_definitions: includeDefinitions,
      limit,
    };
    setRunning(true);
    setError(null);
    setResponse(null);
    setLastPattern(request.pattern);
    try {
      setResponse(await api.searchDatabase(session.id, request));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setRunning(false);
    }
  };

  const openHit = (hit: SearchHit) => {
    if (!session) return;
    openTable(
      session.id,
      { database: hit.database, schema: hit.schema },
      hit.object,
    );
    onClose();
  };

  // Keep hits grouped by database/schema in backend order.
  const groups = useMemo<HitGroup[]>(() => {
    if (!response) return [];
    const map = new Map<string, HitGroup>();
    for (const hit of response.hits) {
      const key = `${hit.database ?? ""}\u0000${hit.schema ?? ""}`;
      let group = map.get(key);
      if (!group) {
        const parts: string[] = [];
        if (hit.database) parts.push(hit.database);
        if (hit.schema) parts.push(hit.schema);
        group = {
          key,
          label: parts.length > 0 ? parts.join(" · ") : "Current database",
          hits: [],
        };
        map.set(key, group);
      }
      group.hits.push(hit);
    }
    return [...map.values()];
  }, [response]);

  return (
    <DialogShell
      title="Find in Database"
      width={880}
      onClose={onClose}
      footer={
        <>
          <button
            className="rounded border border-slate-300 bg-white px-3 py-1.5 text-[11px] text-slate-700 hover:bg-slate-100"
            onClick={onClose}
          >
            Close
          </button>
          <button
            type="submit"
            form="db-search-form"
            disabled={!canSearch}
            className="flex items-center gap-1.5 rounded bg-sky-600 px-3 py-1.5 text-[11px] font-medium text-white hover:bg-sky-700 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {running ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Search size={12} />
            )}
            {running ? "Searching…" : "Search"}
          </button>
        </>
      }
    >
      {!session ? (
        <div className="grid place-items-center px-8 py-14 text-center">
          <div>
            <Database size={20} className="mx-auto mb-2 text-slate-300" />
            <div className="text-[11px] font-medium text-slate-500">
              No active connection
            </div>
            <div className="mt-1 text-[10px] text-slate-400">
              Open a connection first — the search runs on the session’s
              databases.
            </div>
          </div>
        </div>
      ) : (
        <div className="flex flex-col">
          {/* Search form */}
          <form
            id="db-search-form"
            className="space-y-2 border-b border-slate-200 bg-[#f8fafc] px-3 py-2.5"
            onSubmit={(e) => {
              e.preventDefault();
              void run();
            }}
          >
            <div className="flex items-center gap-2">
              <div className="relative min-w-0 flex-1">
                <Search
                  size={12}
                  className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-slate-400"
                />
                <input
                  autoFocus
                  spellCheck={false}
                  value={pattern}
                  onChange={(e) => setPattern(e.target.value)}
                  placeholder="Search object names, columns and definitions…"
                  className="w-full rounded border border-slate-300 bg-white py-1.5 pl-7 pr-2 font-code text-[11px] text-slate-800 placeholder:text-slate-400 focus:border-sky-400 focus:outline-none"
                />
              </div>
              <select
                value={mode}
                onChange={(e) => setMode(e.target.value as SearchMatchMode)}
                className="rounded border border-slate-300 bg-white px-2 py-1.5 text-[11px] text-slate-700 focus:border-sky-400 focus:outline-none"
                title="Match mode"
              >
                {MODES.map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.label}
                  </option>
                ))}
              </select>
              <label className="flex items-center gap-1 text-[10px] text-slate-500">
                Limit
                <select
                  value={limit}
                  onChange={(e) => setLimit(Number(e.target.value))}
                  className="rounded border border-slate-300 bg-white px-2 py-1.5 text-[11px] text-slate-700 focus:border-sky-400 focus:outline-none"
                >
                  {LIMITS.map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <div className="flex items-center justify-between gap-3">
              <div className="flex flex-wrap items-center gap-4">
                <Check
                  label="Case sensitive"
                  checked={caseSensitive}
                  onChange={setCaseSensitive}
                />
                <Check
                  label="Include columns"
                  checked={includeColumns}
                  onChange={setIncludeColumns}
                />
                <Check
                  label="Include definitions"
                  checked={includeDefinitions}
                  onChange={setIncludeDefinitions}
                />
              </div>
              <span
                className="shrink-0 text-[10px] text-slate-400"
                title={`Session: ${session.name}`}
              >
                {session.name} · {session.engine}
              </span>
            </div>
          </form>

          {error && (
            <div className="m-3 flex items-start gap-2 rounded border border-rose-200 bg-rose-50 px-3 py-2 text-[11px] text-rose-700">
              <TriangleAlert size={12} className="mt-px shrink-0" />
              <div className="min-w-0 flex-1 whitespace-pre-wrap break-words">
                {error}
              </div>
            </div>
          )}

          {response && (
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-slate-200 bg-white px-3 py-1.5 text-[11px] text-slate-600">
              <span className="font-medium text-slate-700">
                {response.hits.length.toLocaleString()} hit(s)
              </span>
              <span>{response.scanned_objects.toLocaleString()} object(s) scanned</span>
              {response.truncated && (
                <span className="flex items-center gap-1 text-amber-600">
                  <TriangleAlert size={11} />
                  Result limit reached — refine the search
                </span>
              )}
            </div>
          )}

          {running && !response && (
            <div className="grid place-items-center px-6 py-12">
              <div className="flex items-center gap-2 text-[11px] text-slate-500">
                <Loader2 size={13} className="animate-spin text-sky-500" />
                Searching {session.name}…
              </div>
            </div>
          )}

          {!running && !response && !error && (
            <div className="grid place-items-center px-6 py-12 text-center">
              <div>
                <Search size={20} className="mx-auto mb-2 text-slate-300" />
                <div className="text-[11px] font-medium text-slate-500">
                  Search across the whole database
                </div>
                <div className="mt-1 text-[10px] text-slate-400">
                  Scans table and view names, columns and definitions. Press
                  Enter to run.
                </div>
              </div>
            </div>
          )}

          {response && response.hits.length === 0 && (
            <div className="grid place-items-center px-6 py-10 text-center text-[11px] text-slate-400">
              No matches for “{lastPattern}” — try another mode or enable more
              options.
            </div>
          )}

          {response && response.hits.length > 0 && (
            <table className="w-full border-collapse text-left text-[11px]">
              <thead className="sticky top-0 z-10 bg-[#f1f5f9] text-slate-600 shadow-sm">
                <tr>
                  <th className="border-b border-slate-200 px-3 py-1.5 font-medium">
                    Object
                  </th>
                  <th className="w-[82px] border-b border-slate-200 px-2 py-1.5 font-medium">
                    Kind
                  </th>
                  <th className="w-[86px] border-b border-slate-200 px-2 py-1.5 font-medium">
                    Matched in
                  </th>
                  <th className="w-[180px] border-b border-slate-200 px-2 py-1.5 font-medium">
                    Column / Type
                  </th>
                  <th className="border-b border-slate-200 px-2 py-1.5 font-medium">
                    Excerpt
                  </th>
                </tr>
              </thead>
              {groups.map((group) => (
                <tbody key={group.key}>
                  <tr className="bg-slate-50">
                    <td
                      colSpan={5}
                      className="border-y border-slate-200 px-3 py-1 text-[10px] font-semibold uppercase tracking-wide text-slate-500"
                    >
                      <span className="flex items-center gap-1.5">
                        <Database size={10} className="text-slate-400" />
                        {group.label}
                        <span className="font-normal normal-case text-slate-400">
                          · {group.hits.length} hit(s)
                        </span>
                      </span>
                    </td>
                  </tr>
                  {group.hits.map((hit, i) => (
                    <tr
                      key={`${hit.object}-${hit.matched_in}-${hit.column ?? ""}-${i}`}
                      onClick={() => openHit(hit)}
                      className="cursor-pointer border-b border-slate-100 hover:bg-sky-50"
                    >
                      <td className="px-3 py-[3px]">
                        <span className="flex items-center gap-1.5">
                          <HitIcon kind={hit.kind} />
                          <span className="font-medium text-slate-800">
                            {hit.object}
                          </span>
                        </span>
                      </td>
                      <td className="px-2 py-[3px]">
                        <span
                          className={`rounded px-1.5 py-px text-[10px] font-medium ${KIND_STYLE[hit.kind]}`}
                        >
                          {KIND_LABEL[hit.kind]}
                        </span>
                      </td>
                      <td className="px-2 py-[3px]">
                        <span
                          className={`rounded px-1.5 py-px text-[10px] font-medium ${FIELD_STYLE[hit.matched_in]}`}
                        >
                          {FIELD_LABEL[hit.matched_in]}
                        </span>
                      </td>
                      <td className="px-2 py-[3px]">
                        {hit.column ? (
                          <span
                            className="block max-w-[170px] truncate font-code text-[10px] text-slate-600"
                            title={`${hit.column}${hit.column_type ? ` · ${hit.column_type}` : ""}`}
                          >
                            {hit.column}
                            {hit.column_type && (
                              <span className="text-slate-400">
                                {" "}
                                · {hit.column_type}
                              </span>
                            )}
                          </span>
                        ) : (
                          <span className="text-slate-300">—</span>
                        )}
                      </td>
                      <td className="px-2 py-[3px]">
                        {hit.excerpt ? (
                          <span
                            className="block max-w-[420px] truncate font-code text-[10px] text-slate-500"
                            title={hit.excerpt}
                          >
                            {hit.excerpt}
                          </span>
                        ) : (
                          <span className="text-slate-300">—</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              ))}
            </table>
          )}
        </div>
      )}
    </DialogShell>
  );
}

function Check({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer items-center gap-1.5 text-[11px] text-slate-600">
      <input
        type="checkbox"
        className="accent-sky-600"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      {label}
    </label>
  );
}

function HitIcon({ kind }: { kind: ObjectKind }) {
  if (kind === "table") {
    return <Table2 size={11} className="shrink-0 text-sky-500" />;
  }
  if (kind === "view" || kind === "materialized_view") {
    return <Eye size={11} className="shrink-0 text-violet-500" />;
  }
  return <FileText size={11} className="shrink-0 text-slate-400" />;
}
