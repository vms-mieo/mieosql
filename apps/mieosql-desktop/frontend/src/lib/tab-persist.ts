// Tab autosave + restore (QE-01: never lose a script).
//
// Persists open tabs (never results/rows) to localStorage, debounced, and
// re-opens them on startup. Sessions do not survive a restart, so a saved
// sessionId is remapped when possible (same live session id, else a session
// for the same profile) and falls back to null/active session otherwise.
//
// This module self-initializes on import; `main.tsx` imports it for its
// side effect. Importing it twice (HMR) is safe.

import type { ObjectKind, Scope } from "./bindings";
import { useStore } from "./store";

export const TAB_PERSIST_KEY = "mieosql.tabs.v1";
const MAX_TABS = 30;
const DEBOUNCE_MS = 1000;

/** Minimal tab snapshot — no results, rows, messages or stats. */
export type PersistedTab = {
  id: string;
  kind: "query" | "table" | "objects" | "feature";
  title: string;
  sessionId: string | null;
  /** Lets restore re-attach a tab when the old session id is gone. */
  profileId: string | null;
  sql: string;
  scope?: Scope;
  table?: string;
  objectKind?: ObjectKind;
  featureId?: string;
  featureProps?: Record<string, unknown>;
};

type PersistedState = {
  tabs: PersistedTab[];
  activeTabId: string | null;
};

let timer: ReturnType<typeof setTimeout> | null = null;
let restored = false;

function toPersisted(
  t: ReturnType<typeof useStore.getState>["tabs"][number],
  profileId: string | null,
): PersistedTab {
  const base = {
    id: t.id,
    kind: t.kind,
    title: t.title,
    sessionId: t.sessionId,
    profileId,
    sql: t.kind === "query" ? t.sql : "",
  };
  if (t.kind === "table") return { ...base, scope: t.scope, table: t.table };
  if (t.kind === "objects") {
    return { ...base, scope: t.scope, objectKind: t.objectKind };
  }
  if (t.kind === "feature") {
    return { ...base, featureId: t.featureId, featureProps: t.featureProps };
  }
  return base;
}

function persistNow(): void {
  try {
    const st = useStore.getState();
    const tabs = st.tabs.slice(-MAX_TABS).map((t) => {
      const session = st.sessions.find((s) => s.id === t.sessionId) ?? null;
      return toPersisted(t, session?.profile_id ?? null);
    });
    const ids = new Set(tabs.map((t) => t.id));
    const activeTabId =
      st.activeTabId !== null && ids.has(st.activeTabId)
        ? st.activeTabId
        : null;
    const payload: PersistedState = { tabs, activeTabId };
    localStorage.setItem(TAB_PERSIST_KEY, JSON.stringify(payload));
  } catch {
    // Storage full / private mode: autosave is best-effort.
  }
}

function schedulePersist(): void {
  if (timer !== null) clearTimeout(timer);
  timer = setTimeout(persistNow, DEBOUNCE_MS);
}

/** Map a saved session reference onto a live session (or null). */
function resolveSession(p: PersistedTab): string | null {
  const st = useStore.getState();
  if (p.sessionId && st.sessions.some((s) => s.id === p.sessionId)) {
    return p.sessionId;
  }
  if (p.profileId) {
    const byProfile = st.sessions.find((s) => s.profile_id === p.profileId);
    if (byProfile) return byProfile.id;
  }
  // Dead session: fall back to the active session (newQueryTab does this
  // itself when given null).
  return st.activeSessionId ?? st.sessions[0]?.id ?? null;
}

/** Drop stale session pointers from feature props so tabs fall back cleanly. */
function remapFeatureProps(
  props: Record<string, unknown> | undefined,
): Record<string, unknown> | undefined {
  if (!props) return undefined;
  const next = { ...props };
  if (typeof next.sessionId === "string") {
    const st = useStore.getState();
    const ok =
      st.sessions.some((s) => s.id === next.sessionId) ||
      (typeof next.sessionId === "string" &&
        st.sessions.some((s) => s.profile_id === next.sessionId));
    if (!ok) delete next.sessionId;
  }
  return next;
}

function restore(): void {
  if (restored) return;
  restored = true;
  let raw: string | null = null;
  try {
    raw = localStorage.getItem(TAB_PERSIST_KEY);
  } catch {
    return;
  }
  if (!raw) return;
  let parsed: PersistedState;
  try {
    parsed = JSON.parse(raw) as PersistedState;
  } catch {
    return;
  }
  if (!Array.isArray(parsed?.tabs) || parsed.tabs.length === 0) return;

  const st = useStore.getState();
  // Another copy already opened tabs (e.g. HMR re-import): don't duplicate.
  if (st.tabs.length > 0) return;

  const idMap = new Map<string, string>();
  for (const p of parsed.tabs.slice(-MAX_TABS)) {
    try {
      if (!p || typeof p !== "object") continue;
      if (p.kind === "query") {
        const id = st.newQueryTab(resolveSession(p), p.sql ?? "");
        idMap.set(p.id, id);
      } else if (p.kind === "table" && p.table && p.scope) {
        const sid = resolveSession(p);
        if (sid) idMap.set(p.id, st.openTable(sid, p.scope, p.table));
      } else if (p.kind === "objects" && p.scope && p.objectKind) {
        const sid = resolveSession(p);
        if (sid) {
          idMap.set(p.id, st.openObjects(sid, p.scope, p.objectKind, p.title));
        }
      } else if (p.kind === "feature" && p.featureId) {
        const id = st.openFeatureTab(
          p.featureId,
          p.title || p.featureId,
          remapFeatureProps(p.featureProps),
        );
        idMap.set(p.id, id);
      }
    } catch {
      // Skip corrupt/unknown entries, keep restoring the rest.
    }
  }

  const activeId =
    parsed.activeTabId != null ? idMap.get(parsed.activeTabId) : undefined;
  if (activeId) st.setActiveTab(activeId);
}

/** Subscribe for autosave and restore saved tabs. Idempotent. */
export function initTabPersist(): void {
  restore();
  // Full-state subscription filtered by debounce: cheap (one JSON write per
  // quiet second) and catches sql edits, tab open/close and reorder alike.
  useStore.subscribe(schedulePersist);
}

// Self-run on import (guarded for SSR/tests and HMR double evaluation).
declare global {
  interface Window {
    __mieosqlTabPersist?: boolean;
  }
}

if (typeof window !== "undefined" && !window.__mieosqlTabPersist) {
  window.__mieosqlTabPersist = true;
  initTabPersist();
}
