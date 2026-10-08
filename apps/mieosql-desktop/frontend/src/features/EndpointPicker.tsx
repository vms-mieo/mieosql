// Shared endpoint (session + scope) picker for the sync / transfer dialogs.
// Owned by the F4 (Sync & Transfer) feature agent.
//
// Picking a connection immediately loads its catalog: databases come from
// `list_databases`, schemas from `list_schemas` (engines with a schema level).
// Both fall back to free-text inputs when the catalog cannot be read.

import { useEffect, useState } from "react";

import { api, errorText } from "../lib/api";
import { useStore } from "../lib/store";
import type {
  DatabaseRef,
  Engine,
  SchemaRef,
  Scope,
  SessionInfo,
  SyncEndpoint,
} from "../lib/bindings";

/** Build an endpoint from a session, defaulting the scope to the session DB. */
export function endpointFromSession(
  session: SessionInfo,
  scope?: Scope,
): SyncEndpoint {
  return {
    session_id: session.id,
    scope: scope ?? (session.database ? { database: session.database } : {}),
  };
}

/** Preferred source endpoint: the current object selection, else the active session. */
export function pickSourceEndpoint(
  sessions: SessionInfo[],
  selected: { sessionId: string; scope: Scope } | null,
  activeSessionId: string | null,
): SyncEndpoint | null {
  if (selected) {
    const session = sessions.find((s) => s.id === selected.sessionId);
    if (session) return { session_id: session.id, scope: selected.scope };
  }
  const session =
    sessions.find((s) => s.id === activeSessionId) ?? sessions[0] ?? null;
  return session ? endpointFromSession(session) : null;
}

/**
 * Preferred target endpoint: a session other than the source when one exists,
 * otherwise the active session (a same-session transfer across databases is
 * still valid).
 */
export function pickTargetEndpoint(
  sessions: SessionInfo[],
  source: SyncEndpoint | null,
  activeSessionId: string | null,
): SyncEndpoint | null {
  const session =
    sessions.find((s) => s.id !== source?.session_id) ??
    sessions.find((s) => s.id === activeSessionId) ??
    sessions[0] ??
    null;
  return session ? endpointFromSession(session) : null;
}

export function engineLabel(engine: Engine): string {
  switch (engine) {
    case "mysql":
      return "MySQL";
    case "mariadb":
      return "MariaDB";
    case "postgres":
      return "PostgreSQL";
    case "sqlite":
      return "SQLite";
    default:
      return engine;
  }
}

function engineBadgeClass(engine: Engine): string {
  switch (engine) {
    case "mysql":
    case "mariadb":
      return "border-slate-200 bg-slate-50 text-slate-600";
    case "postgres":
      return "border-slate-200 bg-slate-50 text-slate-600";
    case "sqlite":
      return "border-slate-200 bg-slate-50 text-slate-600";
    default:
      return "border-slate-200 bg-slate-50 text-slate-600";
  }
}

const INPUT_CLS =
  "h-7 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-800 focus:ring-2 focus:ring-accent/40 focus:outline-none disabled:bg-slate-50 disabled:text-slate-400";
const LABEL_CLS =
  "text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase";

export function EndpointPicker({
  label,
  value,
  onChange,
  sessions,
  allowScopeInput = true,
}: {
  label: string;
  value: SyncEndpoint | null;
  onChange: (value: SyncEndpoint | null) => void;
  sessions: SessionInfo[];
  allowScopeInput?: boolean;
}) {
  const session = value
    ? (sessions.find((s) => s.id === value.session_id) ?? null)
    : null;
  // Saved connections that are not open yet — picking one connects it first,
  // so Source/Target work without pre-opening a session in the sidebar.
  const profiles = useStore((s) => s.profiles);
  const offline = profiles.filter(
    (p) => !sessions.some((s) => s.profile_id === p.id),
  );
  const [connecting, setConnecting] = useState(false);

  const [dbs, setDbs] = useState<DatabaseRef[] | null>(null);
  const [dbFailed, setDbFailed] = useState(false);
  const [schemas, setSchemas] = useState<SchemaRef[] | null>(null);
  const [schemaFailed, setSchemaFailed] = useState(false);

  // Selecting a connection loads its database list right away.
  useEffect(() => {
    if (!session || !allowScopeInput) {
      setDbs(null);
      setDbFailed(false);
      return;
    }
    let cancelled = false;
    setDbs(null);
    setDbFailed(false);
    void api
      .listDatabases(session.id)
      .then((list) => {
        if (!cancelled) setDbs(list);
      })
      .catch(() => {
        if (!cancelled) {
          setDbs([]);
          setDbFailed(true);
        }
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session?.id, allowScopeInput]);

  // Schemas follow the picked database (engines with a schema level only).
  const schemaCapable = session?.capabilities.list_schemas ?? false;
  const scopeDatabase = value?.scope.database ?? null;
  useEffect(() => {
    if (!session || !allowScopeInput || !schemaCapable) {
      setSchemas(null);
      setSchemaFailed(false);
      return;
    }
    let cancelled = false;
    setSchemas(null);
    setSchemaFailed(false);
    void api
      .listSchemas(session.id, scopeDatabase)
      .then((list) => {
        if (!cancelled) setSchemas(list);
      })
      .catch(() => {
        if (!cancelled) {
          setSchemas([]);
          setSchemaFailed(true);
        }
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session?.id, schemaCapable, allowScopeInput, scopeDatabase]);

  const changeSession = async (id: string) => {
    // "p:<id>" = saved connection, connect on demand before selecting it.
    if (id.startsWith("p:")) {
      const profileId = id.slice(2);
      setConnecting(true);
      try {
        await useStore.getState().openSession(profileId);
        const next = useStore
          .getState()
          .sessions.find((s) => s.profile_id === profileId);
        if (next) onChange(endpointFromSession(next));
      } catch (err) {
        useStore.getState().toast("error", errorText(err));
      } finally {
        setConnecting(false);
      }
      return;
    }
    const next = sessions.find((s) => s.id === id);
    if (!next) {
      onChange(null);
      return;
    }
    onChange(endpointFromSession(next));
  };

  const patchScope = (patch: Partial<Scope>) => {
    if (!value) return;
    onChange({ ...value, scope: { ...value.scope, ...patch } });
  };

  const dbOptions = dbs ?? [];
  const schemaOptions = schemas ?? [];
  const currentDb = value?.scope.database;
  const currentSchema = value?.scope.schema;

  return (
    <div className="rounded-md border border-slate-200 bg-slate-50/70 p-2">
      <div className="mb-1 flex items-center justify-between gap-2">
        <span className={LABEL_CLS}>{label}</span>
        {session && (
          <span
            className={`rounded border px-1.5 py-px text-[10.5px] font-medium ${engineBadgeClass(
              session.engine,
            )}`}
          >
            {engineLabel(session.engine)}
          </span>
        )}
      </div>
      <select
        className={INPUT_CLS}
        value={value?.session_id ?? ""}
        disabled={connecting}
        onChange={(e) => void changeSession(e.target.value)}
      >
        <option value="">
          {connecting ? "connecting…" : "— select session —"}
        </option>
        {sessions.length > 0 && (
          <optgroup label="Connected">
            {sessions.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name}
              </option>
            ))}
          </optgroup>
        )}
        {offline.length > 0 && (
          <optgroup label="Saved connections">
            {offline.map((p) => (
              <option key={`p:${p.id}`} value={`p:${p.id}`}>
                {p.name}
              </option>
            ))}
          </optgroup>
        )}
      </select>
      {allowScopeInput && (
        <div className="mt-1.5 grid grid-cols-2 gap-1.5">
          {!session ? (
            <input className={INPUT_CLS} placeholder="database" disabled />
          ) : dbFailed ? (
            <input
              className={INPUT_CLS}
              placeholder="database (optional)"
              value={currentDb ?? ""}
              onChange={(e) =>
                patchScope({
                  database: e.target.value || undefined,
                  schema: undefined,
                })
              }
            />
          ) : (
            <select
              className={INPUT_CLS}
              value={currentDb ?? ""}
              disabled={dbs === null}
              title="Database on the selected connection"
              onChange={(e) =>
                patchScope({
                  database: e.target.value || undefined,
                  schema: undefined,
                })
              }
            >
              <option value="">
                {dbs === null ? "loading databases…" : "— default database —"}
              </option>
              {dbOptions.map((d) => (
                <option key={d.name} value={d.name}>
                  {d.name}
                  {d.is_system ? " (system)" : ""}
                </option>
              ))}
              {currentDb && !dbOptions.some((d) => d.name === currentDb) && (
                <option value={currentDb}>{currentDb}</option>
              )}
            </select>
          )}

          {!session ? (
            <input className={INPUT_CLS} placeholder="schema" disabled />
          ) : !schemaCapable ? (
            <input
              className={INPUT_CLS}
              value=""
              placeholder="schema (n/a)"
              disabled
              title="This engine has no schema level"
            />
          ) : schemaFailed ? (
            <input
              className={INPUT_CLS}
              placeholder="schema (optional)"
              value={currentSchema ?? ""}
              onChange={(e) =>
                patchScope({ schema: e.target.value || undefined })
              }
            />
          ) : (
            <select
              className={INPUT_CLS}
              value={currentSchema ?? ""}
              disabled={schemas === null}
              title="Schema inside the selected database"
              onChange={(e) =>
                patchScope({ schema: e.target.value || undefined })
              }
            >
              <option value="">
                {schemas === null ? "loading schemas…" : "— default schema —"}
              </option>
              {schemaOptions.map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name}
                  {s.is_system ? " (system)" : ""}
                </option>
              ))}
              {currentSchema &&
                !schemaOptions.some((s) => s.name === currentSchema) && (
                  <option value={currentSchema}>{currentSchema}</option>
                )}
            </select>
          )}
        </div>
      )}
      {session && (
        <div className="mt-1 truncate text-[11px] text-slate-400">
          {session.database
            ? `connected database: ${session.database}`
            : "empty scope = the session default database"}
        </div>
      )}
      {sessions.length === 0 && offline.length === 0 && (
        <div className="mt-1 text-[11px] text-amber-600">
          No saved connections — create one first.
        </div>
      )}
    </div>
  );
}
