// Shared endpoint (session + scope) picker for the sync / transfer dialogs.
// Owned by the F4 (Sync & Transfer) feature agent.

import type { Engine, Scope, SessionInfo, SyncEndpoint } from "../lib/bindings";

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

  const changeSession = (sessionId: string) => {
    const next = sessions.find((s) => s.id === sessionId);
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
        onChange={(e) => changeSession(e.target.value)}
      >
        <option value="">— select session —</option>
        {sessions.map((s) => (
          <option key={s.id} value={s.id}>
            {s.name}
          </option>
        ))}
      </select>
      {allowScopeInput && (
        <div className="mt-1.5 grid grid-cols-2 gap-1.5">
          <input
            className={INPUT_CLS}
            placeholder="database (optional)"
            value={value?.scope.database ?? ""}
            disabled={!session}
            onChange={(e) => patchScope({ database: e.target.value || undefined })}
          />
          <input
            className={INPUT_CLS}
            placeholder="schema (optional)"
            value={value?.scope.schema ?? ""}
            disabled={!session}
            onChange={(e) => patchScope({ schema: e.target.value || undefined })}
          />
        </div>
      )}
      {session && (
        <div className="mt-1 truncate text-[11px] text-slate-400">
          {session.database
            ? `connected database: ${session.database}`
            : "empty scope = the session default database"}
        </div>
      )}
      {sessions.length === 0 && (
        <div className="mt-1 text-[11px] text-amber-600">
          No active connections — open one first.
        </div>
      )}
    </div>
  );
}
