// Global application state (zustand): profiles, sessions, tabs/results,
// history, toasts and the small bits of UI state shared across panels.

import { create } from "zustand";
import { save } from "@tauri-apps/plugin-dialog";

import type { ApiError } from "@bindings/ApiError";
import type { ColumnMeta } from "@bindings/ColumnMeta";
import type { ConnectionProfile } from "@bindings/ConnectionProfile";
import type { ExportFormat } from "@bindings/ExportFormat";
import type { HistoryEntry } from "@bindings/HistoryEntry";
import type { ObjectKind } from "@bindings/ObjectKind";
import type { ProfileInput } from "@bindings/ProfileInput";
import type { QueryEvent, QueryStats, Scope, ServerInfo, SessionInfo, Value } from "./bindings";
import { api, errorText } from "./api";
import { formatBytes } from "./format";
import { bindPlaceholders, findPlaceholders } from "./sql-params";
import { findUnsafeWrites } from "./sql-suggest";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type ResultSet = {
  queryId: string;
  /** The exact SQL that produced this result (re-run verbatim for exports). */
  sql: string;
  columns: ColumnMeta[];
  rows: Value[][];
  status: "running" | "done" | "error";
  error: ApiError | null;
  stats: QueryStats | null;
  startedAt: number;
  elapsedMs: number | null;
  truncated: boolean;
  /** true when the cap on buffered rows cut the result data (all rows still counted). */
  rowsDropped: boolean;
  /** Pinned results survive the per-tab result cap and are kept around. */
  pinned?: boolean;
};

export type LogMessage = {
  id: number;
  level: "info" | "error" | "warn";
  text: string;
  at: number;
};

export type Tab = {
  id: string;
  kind: "query" | "table" | "objects" | "feature";
  title: string;
  sessionId: string | null;
  /** query tabs: last known editor content */
  sql: string;
  /** table/objects tabs */
  scope?: Scope;
  table?: string;
  objectKind?: ObjectKind;
  /** feature tabs: id in the feature registry + opening props */
  featureId?: string;
  featureProps?: Record<string, unknown>;
  results: ResultSet[];
  activeResult: number;
  messages: LogMessage[];
  running: boolean;
  runStartedAt: number | null;
};

export type Toast = {
  id: number;
  level: "success" | "error" | "info";
  text: string;
};

export type SelectedObject = {
  sessionId: string;
  scope: Scope;
  table: string;
  kind: ObjectKind;
};

export type RightTab = "object" | "history" | "snippets";

export type ThemeSetting = "system" | "light" | "dark";
export type FontSizeSetting = "small" | "medium" | "large";
export type DensitySetting = "comfortable" | "compact";

export type AppSettings = {
  theme: ThemeSetting;
  fontSize: FontSizeSetting;
  density: DensitySetting;
  /** Editor tab width in spaces (stored; enforcement is TODO). */
  tabSize: number;
  /** Format SQL on save (stored; enforcement is TODO). */
  formatOnSave: boolean;
  /** Show the unsafe-write (UPDATE/DELETE without WHERE) warning. */
  confirmUnsafe: boolean;
};

/**
 * Database/table management dialogs hosted by `<ObjectDialogs />`.
 * `props` carries the opening context (session, tree scope, table name).
 */
export type ObjectDialogKind =
  | "new-database"
  | "new-table"
  | "edit-table"
  | "import"
  | "backup"
  | "restore";

export type ObjectDialogProps = {
  sessionId?: string | null;
  scope?: Scope;
  table?: string;
  objectKind?: ObjectKind;
};

const MAX_BUFFERED_ROWS = 200_000;
const MAX_RESULTS_PER_TAB = 8;
const LIMIT_KEY = "mieosql.limit";
const SETTINGS_KEY = "mieosql.settings";
const LIMIT_OPTIONS = [100, 500, 1000, 5000, 10000, 0] as const;

const DEFAULT_SETTINGS: AppSettings = {
  theme: "system",
  fontSize: "medium",
  density: "comfortable",
  tabSize: 2,
  formatOnSave: false,
  confirmUnsafe: true,
};

/** Export formats with their file extension and dialog label. */
const EXPORT_META: Record<ExportFormat, { ext: string; label: string }> = {
  csv: { ext: "csv", label: "CSV" },
  tsv: { ext: "tsv", label: "Text (TSV)" },
  json: { ext: "json", label: "JSON" },
  json_lines: { ext: "jsonl", label: "JSON Lines" },
  sql: { ext: "sql", label: "SQL INSERTs" },
  xlsx: { ext: "xlsx", label: "Excel (XLSX)" },
};

export const EXPORT_FORMATS: ExportFormat[] = [
  "csv",
  "tsv",
  "json",
  "json_lines",
  "sql",
  "xlsx",
];

export function exportFormatLabel(format: ExportFormat): string {
  return EXPORT_META[format].label;
}

let toastSeq = 1;
let messageSeq = 1;
let historyRefreshTimer: number | null = null;

/**
 * Query events can arrive *before* the `invoke()` that started the query has
 * resolved (the backend streams immediately). Events for unknown query ids are
 * buffered here and replayed once the result set is attached.
 */
const earlyEvents = new Map<string, QueryEvent[]>();

/** Profiles with an in-flight `openSession` (dedupes fast double-clicks). */
const connectingProfiles = new Set<string>();

function uid(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `t_${Date.now().toString(36)}_${Math.random().toString(36).slice(2)}`;
}

function loadLimit(): number {
  const raw = localStorage.getItem(LIMIT_KEY);
  const n = raw ? Number(raw) : NaN;
  return Number.isFinite(n) ? n : 1000;
}

function loadSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (!raw) return DEFAULT_SETTINGS;
    const parsed = JSON.parse(raw) as Partial<AppSettings>;
    return {
      theme:
        parsed.theme === "light" ||
        parsed.theme === "dark" ||
        parsed.theme === "system"
          ? parsed.theme
          : DEFAULT_SETTINGS.theme,
      fontSize:
        parsed.fontSize === "small" ||
        parsed.fontSize === "large" ||
        parsed.fontSize === "medium"
          ? parsed.fontSize
          : DEFAULT_SETTINGS.fontSize,
      density:
        parsed.density === "compact" || parsed.density === "comfortable"
          ? parsed.density
          : DEFAULT_SETTINGS.density,
      tabSize:
        typeof parsed.tabSize === "number" &&
        Number.isFinite(parsed.tabSize) &&
        parsed.tabSize >= 1 &&
        parsed.tabSize <= 8
          ? Math.floor(parsed.tabSize)
          : DEFAULT_SETTINGS.tabSize,
      formatOnSave:
        typeof parsed.formatOnSave === "boolean"
          ? parsed.formatOnSave
          : DEFAULT_SETTINGS.formatOnSave,
      confirmUnsafe:
        typeof parsed.confirmUnsafe === "boolean"
          ? parsed.confirmUnsafe
          : DEFAULT_SETTINGS.confirmUnsafe,
    };
  } catch {
    return DEFAULT_SETTINGS;
  }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

type State = {
  ready: boolean;
  apiVersion: string;
  profiles: ConnectionProfile[];
  sessions: SessionInfo[];
  serverInfos: Record<string, ServerInfo>;
  activeSessionId: string | null;
  tabs: Tab[];
  activeTabId: string | null;
  toasts: Toast[];
  history: HistoryEntry[];
  selected: SelectedObject | null;
  rightTab: RightTab;
  connectionModal: { open: boolean; profileId: string | null };
  objectDialog: { kind: ObjectDialogKind; props: ObjectDialogProps } | null;
  limit: number;
  limitOptions: readonly number[];
  cursor: { line: number; col: number };
  settings: AppSettings;
  settingsOpen: boolean;

  // lifecycle
  init: () => Promise<void>;
  toast: (level: Toast["level"], text: string) => void;
  dismissToast: (id: number) => void;

  // profiles / sessions
  refreshProfiles: () => Promise<void>;
  saveProfile: (input: ProfileInput) => Promise<ConnectionProfile>;
  deleteProfile: (id: string) => Promise<void>;
  openSession: (profileId: string) => Promise<void>;
  closeSession: (sessionId: string) => Promise<void>;
  refreshSessionInfo: (sessionId: string) => Promise<void>;
  setConnectionModal: (open: boolean, profileId?: string | null) => void;

  // object dialogs (database/table management, import, backup/restore)
  openObjectDialog: (kind: ObjectDialogKind, props?: ObjectDialogProps) => void;
  closeObjectDialog: () => void;

  // tabs
  newQueryTab: (sessionId?: string | null, sql?: string) => string;
  openTable: (sessionId: string, scope: Scope, table: string) => string;
  openObjects: (
    sessionId: string,
    scope: Scope,
    objectKind: ObjectKind,
    label: string,
  ) => string;
  closeTab: (tabId: string) => void;
  setActiveTab: (tabId: string) => void;
  setSql: (tabId: string, sql: string) => void;
  setActiveSession: (sessionId: string) => void;
  /** Replace a tab's scope (database/schema) — used by the toolbar pickers. */
  setTabScope: (tabId: string, scope: Scope) => void;

  // query execution
  runQuery: (tabId: string, sqlOverride?: string, params?: Value[]) => Promise<void>;
  runPreview: (tabId: string) => Promise<void>;
  cancelQuery: (queryId: string) => Promise<void>;
  stopActive: () => Promise<void>;
  setActiveResult: (tabId: string, index: number) => void;
  handleQueryEvent: (queryId: string, event: QueryEvent) => void;
  exportResult: (
    tabId: string,
    resultIndex: number,
    format: ExportFormat,
  ) => Promise<void>;
  togglePin: (tabId: string, index: number) => void;

  // feature pack v1
  featureDialog: string | null;
  openFeatureDialog: (id: string) => void;
  closeFeatureDialog: () => void;
  openFeatureTab: (
    featureId: string,
    title: string,
    props?: Record<string, unknown>,
  ) => string;
  explainActiveQuery: (sessionId: string, sql: string) => void;
  /** Run `sql`, asking for parameter values first when placeholders exist. */
  requestRun: (tabId: string, sqlOverride?: string) => Promise<void>;
  runWithParams: (
    tabId: string,
    sql: string,
    values: Array<string | null>,
  ) => Promise<void>;
  paramDialog: { tabId: string; sql: string } | null;
  closeParamDialog: () => void;

  // transactions
  beginTransaction: (sessionId: string) => Promise<void>;
  commitTransaction: (sessionId: string) => Promise<void>;
  rollbackTransaction: (sessionId: string) => Promise<void>;

  // misc UI
  refreshHistory: () => Promise<void>;
  clearHistory: () => Promise<void>;
  selectObject: (selected: SelectedObject | null) => void;
  setRightTab: (tab: RightTab) => void;
  setLimit: (limit: number) => void;
  setCursor: (line: number, col: number) => void;
  updateSettings: (patch: Partial<AppSettings>) => void;
  setSettingsOpen: (open: boolean) => void;
};

function scheduleHistoryRefresh(get: () => State) {
  if (historyRefreshTimer !== null) return;
  historyRefreshTimer = window.setTimeout(() => {
    historyRefreshTimer = null;
    void get().refreshHistory();
  }, 200);
}

export const useStore = create<State>((set, get) => ({
  ready: false,
  apiVersion: "",
  profiles: [],
  sessions: [],
  serverInfos: {},
  activeSessionId: null,
  tabs: [],
  activeTabId: null,
  toasts: [],
  history: [],
  selected: null,
  rightTab: "history",
  connectionModal: { open: false, profileId: null },
  objectDialog: null,
  featureDialog: null,
  paramDialog: null,
  limit: loadLimit(),
  limitOptions: LIMIT_OPTIONS,
  cursor: { line: 1, col: 1 },
  settings: loadSettings(),
  settingsOpen: false,

  init: async () => {
    try {
      const apiVersion = await api.apiVersion();
      const [profiles, history] = await Promise.all([
        api.listProfiles(),
        api.listHistory(100),
      ]);
      set({ ready: true, apiVersion, profiles, history });
    } catch (err) {
      set({ ready: true });
      get().toast("error", `Backend not reachable: ${errorText(err)}`);
    }
  },

  toast: (level, text) => {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts, { id, level, text }] }));
    window.setTimeout(() => get().dismissToast(id), level === "error" ? 9000 : 5000);
  },

  dismissToast: (id) =>
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),

  // ------------------------------------------------------------- profiles

  refreshProfiles: async () => {
    try {
      set({ profiles: await api.listProfiles() });
    } catch (err) {
      get().toast("error", errorText(err));
    }
  },

  saveProfile: async (input) => {
    const profile = await api.saveProfile(input);
    await get().refreshProfiles();
    return profile;
  },

  deleteProfile: async (id) => {
    await api.deleteProfile(id);
    await get().refreshProfiles();
    get().toast("info", "Connection deleted");
  },

  openSession: async (profileId) => {
    // Already connected: focus the existing session instead of opening a
    // second one. Also dedupe while a connect for this profile is in flight.
    const existing = get().sessions.find((s) => s.profile_id === profileId);
    if (existing) {
      set({ activeSessionId: existing.id });
      return;
    }
    if (connectingProfiles.has(profileId)) return;
    connectingProfiles.add(profileId);
    try {
      const info = await api.openSession(profileId);
      let serverInfo: ServerInfo | null = null;
      try {
        serverInfo = await api.serverInfo(info.id);
      } catch {
        /* server info is decorative */
      }
      set((s) => ({
        sessions: [...s.sessions.filter((x) => x.id !== info.id), info],
        serverInfos: serverInfo
          ? { ...s.serverInfos, [info.id]: serverInfo }
          : s.serverInfos,
        activeSessionId: info.id,
      }));
      get().toast("success", `Connected to ${info.name}`);
    } finally {
      connectingProfiles.delete(profileId);
    }
  },

  closeSession: async (sessionId) => {
    try {
      await api.closeSession(sessionId);
    } catch (err) {
      get().toast("error", errorText(err));
    }
    set((s) => {
      const sessions = s.sessions.filter((x) => x.id !== sessionId);
      const serverInfos = { ...s.serverInfos };
      delete serverInfos[sessionId];
      return {
        sessions,
        serverInfos,
        activeSessionId:
          s.activeSessionId === sessionId
            ? (sessions[0]?.id ?? null)
            : s.activeSessionId,
        tabs: s.tabs.map((t) =>
          t.sessionId === sessionId ? { ...t, running: false } : t,
        ),
      };
    });
  },

  refreshSessionInfo: async (sessionId) => {
    try {
      const info = await api.sessionInfo(sessionId);
      set((s) => ({
        sessions: s.sessions.map((x) => (x.id === sessionId ? info : x)),
      }));
    } catch {
      /* session may be gone */
    }
  },

  setConnectionModal: (open, profileId = null) =>
    set({ connectionModal: { open, profileId } }),

  // -------------------------------------------------------- object dialogs

  openObjectDialog: (kind, props) =>
    set({ objectDialog: { kind, props: props ?? {} } }),

  closeObjectDialog: () => set({ objectDialog: null }),

  // ----------------------------------------------------------------- tabs

  newQueryTab: (sessionId, sql) => {
    const state = get();
    const sid =
      sessionId ?? state.activeSessionId ?? state.sessions[0]?.id ?? null;
    const count = state.tabs.filter((t) => t.kind === "query").length + 1;
    const tab: Tab = {
      id: uid(),
      kind: "query",
      title: `Query ${count}`,
      sessionId: sid,
      sql: sql ?? "",
      results: [],
      activeResult: 0,
      messages: [],
      running: false,
      runStartedAt: null,
    };
    set({ tabs: [...state.tabs, tab], activeTabId: tab.id });
    return tab.id;
  },

  openTable: (sessionId, scope, table) => {
    const state = get();
    const existing = state.tabs.find(
      (t) =>
        t.kind === "table" &&
        t.sessionId === sessionId &&
        t.table === table &&
        t.scope?.database === scope.database &&
        t.scope?.schema === scope.schema,
    );
    if (existing) {
      set({ activeTabId: existing.id });
      return existing.id;
    }
    const tab: Tab = {
      id: uid(),
      kind: "table",
      title: table,
      sessionId,
      sql: "",
      scope,
      table,
      results: [],
      activeResult: 0,
      messages: [],
      running: false,
      runStartedAt: null,
    };
    set({ tabs: [...state.tabs, tab], activeTabId: tab.id });
    void get().runPreview(tab.id);
    return tab.id;
  },

  openObjects: (sessionId, scope, objectKind, label) => {
    const state = get();
    const tab: Tab = {
      id: uid(),
      kind: "objects",
      title: label,
      sessionId,
      sql: "",
      scope,
      objectKind,
      results: [],
      activeResult: 0,
      messages: [],
      running: false,
      runStartedAt: null,
    };
    set({ tabs: [...state.tabs, tab], activeTabId: tab.id });
    return tab.id;
  },

  closeTab: (tabId) =>
    set((s) => {
      const tabs = s.tabs.filter((t) => t.id !== tabId);
      const activeTabId =
        s.activeTabId === tabId ? (tabs[tabs.length - 1]?.id ?? null) : s.activeTabId;
      return { tabs, activeTabId };
    }),

  setActiveTab: (tabId) => set({ activeTabId: tabId }),

  setSql: (tabId, sql) =>
    set((s) => ({
      tabs: s.tabs.map((t) => (t.id === tabId ? { ...t, sql } : t)),
    })),

  setActiveSession: (sessionId) => {
    const state = get();
    set({
      activeSessionId: sessionId,
      tabs: state.tabs.map((t) =>
        t.id === state.activeTabId && t.kind === "query"
          ? { ...t, sessionId }
          : t,
      ),
    });
  },

  setTabScope: (tabId, scope) =>
    set((s) => ({
      tabs: s.tabs.map((t) => (t.id === tabId ? { ...t, scope } : t)),
    })),

  // -------------------------------------------------------------- queries

  runQuery: async (tabId, sqlOverride, params) => {
    const state = get();
    const tab = state.tabs.find((t) => t.id === tabId);
    if (!tab) return;
    const sessionId = tab.sessionId;
    if (!sessionId) {
      get().toast("error", "No active connection for this tab");
      return;
    }
    const sql = (sqlOverride ?? tab.sql).trim();
    if (!sql) return;

    if (!confirmUnsafeWrites(sql)) {
      get().toast("info", "Run cancelled");
      return;
    }

    const boundParams = params ?? [];
    // Navicat-style multi-statement run: split the editor text first. A lone
    // statement keeps the prepared streaming path (bound params + grids);
    // several statements — notably DELIMITER-wrapped routine definitions —
    // run sequentially on the simple execution path instead of being sent to
    // the server as one prepared statement (which MySQL rejects).
    if (boundParams.length === 0) {
      try {
        const parts = await api.splitSqlText(sql);
        for (const warning of parts.warnings) {
          pushMessage(set, tabId, "warn", warning);
        }
        if (parts.statements.length > 1) {
          await runStatementsSequentially(set, get().toast, tabId, sessionId, parts.statements);
          return;
        }
      } catch {
        // Splitting must never block running: fall through to the single
        // prepared statement path below.
      }
    }

    try {
      const limit = state.limit;
      const queryId = await api.startQuery(sessionId, sql, boundParams, {
        mode: "prepared",
        fetch_size: 512,
        max_rows: limit > 0 ? limit : null,
        timeout_ms: null,
      });
      attachResult(set, get, tabId, queryId, sql);
    } catch (err) {
      pushMessage(set, tabId, "error", errorText(err));
      get().toast("error", errorText(err));
    }
  },

  runPreview: async (tabId) => {
    const state = get();
    const tab = state.tabs.find((t) => t.id === tabId);
    if (!tab?.sessionId || !tab.table || !tab.scope) return;
    try {
      const limit = state.limit > 0 ? state.limit : 1000;
      const { query_id, sql } = await api.previewTable(
        tab.sessionId,
        tab.scope,
        tab.table,
        limit,
      );
      attachResult(set, get, tabId, query_id, sql);
    } catch (err) {
      pushMessage(set, tabId, "error", errorText(err));
      get().toast("error", errorText(err));
    }
  },

  /// Re-run the result's SQL in the backend and write it straight to a file.
  exportResult: async (tabId, resultIndex, format) => {
    const state = get();
    const tab = state.tabs.find((t) => t.id === tabId);
    const result = tab?.results[resultIndex];
    if (!tab?.sessionId || !result) return;
    if (!result.sql) {
      get().toast("error", "No SQL recorded for this result");
      return;
    }

    const meta = EXPORT_META[format];
    const base = (tab.kind === "table" ? (tab.table ?? "result") : tab.title)
      .replace(/[^\w.-]+/g, "_")
      .slice(0, 60);
    const stamp = new Date()
      .toISOString()
      .slice(0, 19)
      .replace(/[:T]/g, "-");

    let path: string | null = null;
    try {
      path = await save({
        title: `Export ${meta.label}`,
        defaultPath: `mieosql-${base}-${stamp}.${meta.ext}`,
        filters: [{ name: meta.label, extensions: [meta.ext] }],
      });
    } catch (err) {
      get().toast("error", errorText(err));
      return;
    }
    if (!path) return; // cancelled

    try {
      get().toast("info", `Exporting ${meta.label}…`);
      const stats = await api.exportResult(
        tab.sessionId,
        result.sql,
        format,
        path,
        tab.kind === "table" ? (tab.table ?? null) : null,
      );
      get().toast(
        "success",
        `Exported ${stats.rows.toLocaleString()} rows (${formatBytes(stats.bytes)}) in ${stats.elapsed_ms} ms${
          stats.truncated ? " — truncated at format limit" : ""
        }\n${stats.path}`,
      );
    } catch (err) {
      get().toast("error", errorText(err));
    }
  },

  cancelQuery: async (queryId) => {
    try {
      await api.cancelQuery(queryId);
    } catch (err) {
      get().toast("error", errorText(err));
    }
  },

  stopActive: async () => {
    const state = get();
    const tab = state.tabs.find((t) => t.id === state.activeTabId);
    const running = tab?.results.find((r) => r.status === "running");
    if (running) await get().cancelQuery(running.queryId);
  },

  setActiveResult: (tabId, index) =>
    set((s) => ({
      tabs: s.tabs.map((t) =>
        t.id === tabId ? { ...t, activeResult: index } : t,
      ),
    })),

  handleQueryEvent: (queryId, event) => {
    const known = get().tabs.some((t) =>
      t.results.some((r) => r.queryId === queryId),
    );
    if (!known) {
      const list = earlyEvents.get(queryId) ?? [];
      if (list.length < 1000) {
        list.push(event);
        earlyEvents.set(queryId, list);
      }
      while (earlyEvents.size > 64) {
        const first = earlyEvents.keys().next().value;
        if (first === undefined) break;
        earlyEvents.delete(first);
      }
      return;
    }
    let touchedSession: string | null = null;
    set((s) => {
      let messages: LogMessage[] | null = null;
      const tabs = s.tabs.map((tab) => {
        const idx = tab.results.findIndex((r) => r.queryId === queryId);
        if (idx < 0) return tab;
        const results = tab.results.slice();
        const prev = results[idx]!;
        const rs: ResultSet = { ...prev };

        switch (event.type) {
          case "started":
            break;
          case "columns":
            rs.columns = event.columns;
            break;
          case "batch": {
            if (rs.rows.length < MAX_BUFFERED_ROWS) {
              const room = MAX_BUFFERED_ROWS - rs.rows.length;
              if (event.rows.length > room) {
                rs.rows = rs.rows.concat(event.rows.slice(0, room));
                rs.rowsDropped = true;
              } else {
                rs.rows = rs.rows.concat(event.rows);
              }
            } else {
              rs.rowsDropped = true;
            }
            break;
          }
          case "affected": {
            const base: QueryStats = rs.stats ?? {
              rows: 0,
              rows_affected: 0,
              elapsed_ms: 0,
              truncated: false,
            };
            rs.stats = {
              ...base,
              rows_affected: event.rows_affected,
              last_insert_id: event.last_insert_id ?? undefined,
            };
            break;
          }
          case "message": {
            messages = [
              ...(messages ?? tab.messages),
              {
                id: messageSeq++,
                level: event.level === "warning" ? "warn" : "info",
                text: event.text,
                at: Date.now(),
              },
            ];
            break;
          }
          case "error": {
            rs.status = "error";
            rs.error = event.error;
            rs.elapsedMs = Date.now() - rs.startedAt;
            messages = [
              ...(messages ?? tab.messages),
              {
                id: messageSeq++,
                level: "error",
                text: `${event.error.code}: ${event.error.message}`,
                at: Date.now(),
              },
            ];
            break;
          }
          case "finished": {
            rs.status = "done";
            rs.stats = event.stats;
            rs.elapsedMs = event.stats.elapsed_ms;
            rs.truncated = event.stats.truncated;
            break;
          }
        }

        results[idx] = rs;
        const done = rs.status !== "running";
        if (done) touchedSession = tab.sessionId;
        return {
          ...tab,
          results,
          running: done ? false : tab.running,
          messages: messages ?? tab.messages,
        };
      });
      return { tabs };
    });

    if (touchedSession) {
      scheduleHistoryRefresh(get);
      void get().refreshSessionInfo(touchedSession);
    }
  },

  // ----------------------------------------------------------- transactions

  beginTransaction: async (sessionId) => {
    try {
      await api.beginTransaction(sessionId);
      await get().refreshSessionInfo(sessionId);
      get().toast("info", "Transaction started");
    } catch (err) {
      get().toast("error", errorText(err));
    }
  },

  commitTransaction: async (sessionId) => {
    try {
      await api.commitTransaction(sessionId);
      await get().refreshSessionInfo(sessionId);
      get().toast("success", "Transaction committed");
    } catch (err) {
      get().toast("error", errorText(err));
    }
  },

  rollbackTransaction: async (sessionId) => {
    try {
      await api.rollbackTransaction(sessionId);
      await get().refreshSessionInfo(sessionId);
      get().toast("info", "Transaction rolled back");
    } catch (err) {
      get().toast("error", errorText(err));
    }
  },

  // --------------------------------------------------------- feature pack v1

  openFeatureDialog: (id) => set({ featureDialog: id }),

  closeFeatureDialog: () => set({ featureDialog: null }),

  openFeatureTab: (featureId, title, props) => {
    const state = get();
    const active = state.tabs.find((t) => t.id === state.activeTabId);
    const tab: Tab = {
      id: uid(),
      kind: "feature",
      title,
      sessionId: active?.sessionId ?? state.activeSessionId,
      sql: "",
      results: [],
      activeResult: 0,
      messages: [],
      running: false,
      runStartedAt: null,
      featureId,
      featureProps: props,
    };
    set({ tabs: [...state.tabs, tab], activeTabId: tab.id });
    return tab.id;
  },

  explainActiveQuery: (sessionId, sql) => {
    const state = get();
    const engine = state.sessions.find((s) => s.id === sessionId)?.engine;
    state.openFeatureTab("explain", "Explain", {
      sessionId,
      sql,
      analyze: engine === "postgres",
    });
  },

  requestRun: async (tabId, sqlOverride) => {
    const state = get();
    const tab = state.tabs.find((t) => t.id === tabId);
    if (!tab) return;
    const sql = (sqlOverride ?? tab.sql).trim();
    if (!sql) return;
    const engine =
      state.sessions.find((s) => s.id === tab.sessionId)?.engine ?? null;
    if (findPlaceholders(engine, sql).length > 0) {
      set({ paramDialog: { tabId, sql } });
      return;
    }
    await state.runQuery(tabId, sql);
  },

  runWithParams: async (tabId, sql, values) => {
    const state = get();
    const tab = state.tabs.find((t) => t.id === tabId);
    if (!tab?.sessionId) {
      state.toast("error", "No active connection for this tab");
      return;
    }
    const engine =
      state.sessions.find((s) => s.id === tab.sessionId)?.engine ?? null;
    const bound = bindPlaceholders(engine, sql, values);
    set({ paramDialog: null });
    try {
      const limit = state.limit;
      const queryId = await api.startQuery(
        tab.sessionId,
        bound.sql,
        bound.params,
        {
          mode: "prepared",
          fetch_size: 512,
          max_rows: limit > 0 ? limit : null,
          timeout_ms: null,
        },
      );
      attachResult(set, get, tabId, queryId, bound.sql);
    } catch (err) {
      pushMessage(set, tabId, "error", errorText(err));
      state.toast("error", errorText(err));
    }
  },

  closeParamDialog: () =>
    set((s) => ({
      paramDialog: null,
      featureDialog: s.featureDialog === "params" ? null : s.featureDialog,
    })),

  togglePin: (tabId, index) =>
    set((s) => ({
      tabs: s.tabs.map((t) =>
        t.id === tabId
          ? {
              ...t,
              results: t.results.map((r, i) =>
                i === index ? { ...r, pinned: !r.pinned } : r,
              ),
            }
          : t,
      ),
    })),

  // ------------------------------------------------------------------ misc

  refreshHistory: async () => {
    try {
      set({ history: await api.listHistory(100) });
    } catch {
      /* non critical */
    }
  },

  clearHistory: async () => {
    try {
      await api.clearHistory();
      set({ history: [] });
    } catch (err) {
      get().toast("error", errorText(err));
    }
  },

  selectObject: (selected) => set({ selected, rightTab: "object" }),

  setRightTab: (tab) => set({ rightTab: tab }),

  setLimit: (limit) => {
    localStorage.setItem(LIMIT_KEY, String(limit));
    set({ limit });
  },

  setCursor: (line, col) => set({ cursor: { line, col } }),

  updateSettings: (patch) => {
    const settings = { ...get().settings, ...patch };
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
    } catch {
      /* storage may be unavailable; keep the in-memory value */
    }
    set({ settings });
  },

  setSettingsOpen: (open) => set({ settingsOpen: open }),
}));

/**
 * Warn + block UPDATE/DELETE statements without WHERE (DBeaver/Navicat-style
 * safety net). Returns true when the run may proceed.
 */
export function confirmUnsafeWrites(sql: string): boolean {
  try {
    if (useStore.getState().settings.confirmUnsafe === false) return true;
  } catch {
    /* store unavailable (tests): fall through to the dialog */
  }
  const unsafe = findUnsafeWrites(sql);
  if (unsafe.length === 0) return true;
  const lines = unsafe
    .slice(0, 5)
    .map((w) => `- ${w.kind} ${w.preview}`)
    .join("\n");
  const more =
    unsafe.length > 5 ? `\n… and ${unsafe.length - 5} more` : "";
  return window.confirm(
    `Warning: ${unsafe.length} statement(s) without WHERE will hit every row:\n${lines}${more}\n\nRun anyway?`,
  );
}

// ---------------------------------------------------------------------------
// Helpers (operate on the store via set/get)
// ---------------------------------------------------------------------------

type SetFn = (partial: Partial<State> | ((s: State) => Partial<State>)) => void;

function attachResult(
  set: SetFn,
  get: () => State,
  tabId: string,
  queryId: string,
  sql: string,
) {
  set((s) => ({
    tabs: s.tabs.map((tab) => {
      if (tab.id !== tabId) return tab;
      const rs: ResultSet = {
        queryId,
        sql,
        columns: [],
        rows: [],
        status: "running",
        error: null,
        stats: null,
        startedAt: Date.now(),
        elapsedMs: null,
        truncated: false,
        rowsDropped: false,
      };
      const appended = [...tab.results, rs];
      const keep = new Set<ResultSet>();
      for (const r of appended) if (r.pinned) keep.add(r);
      for (const r of appended.slice(-MAX_RESULTS_PER_TAB)) keep.add(r);
      const results = appended.filter((r) => keep.has(r));
      return {
        ...tab,
        results,
        activeResult: results.indexOf(rs),
        running: true,
        runStartedAt: Date.now(),
        messages: [
          ...tab.messages,
          { id: messageSeq++, level: "info" as const, text: `> ${firstLine(sql)}`, at: Date.now() },
        ],
      };
    }),
  }));
  void get;

  // Replay any events that arrived before the result set existed.
  const buffered = earlyEvents.get(queryId);
  if (buffered) {
    earlyEvents.delete(queryId);
    for (const evt of buffered) {
      get().handleQueryEvent(queryId, evt);
    }
  }
}

function firstLine(sql: string): string {
  const line = sql.split("\n").find((l) => l.trim().length > 0) ?? sql;
  return line.length > 160 ? `${line.slice(0, 160)}…` : line;
}

/**
 * Run several statements one by one on the simple execution path (the same
 * path the .sql script runner uses), stopping at the first error. Used when
 * the editor holds multi-statement input such as DELIMITER-wrapped
 * CREATE PROCEDURE scripts. Plain SELECT grids are only produced by the
 * single-statement prepared path — here each statement is reported in the
 * Messages log, Navicat-style.
 */
async function runStatementsSequentially(
  set: SetFn,
  toast: (level: "success" | "error" | "info", text: string) => void,
  tabId: string,
  sessionId: string,
  statements: string[],
): Promise<void> {
  const startedAt = Date.now();
  let applied = 0;
  for (let idx = 0; idx < statements.length; idx++) {
    const stmt = statements[idx]!;
    pushMessage(set, tabId, "info", `> ${firstLine(stmt)}`);
    try {
      const result = await api.executeBatch(sessionId, [stmt], false);
      applied += result.applied;
      if (result.failed > 0) {
        const msg = `Statement ${idx + 1}/${statements.length} failed: ${result.errors[0] ?? "unknown error"}`;
        pushMessage(set, tabId, "error", msg);
        toast("error", msg);
        return;
      }
      pushMessage(
        set,
        tabId,
        "info",
        `Statement ${idx + 1}/${statements.length} ok (${result.elapsed_ms} ms)`,
      );
    } catch (err) {
      const msg = `Statement ${idx + 1}/${statements.length} failed: ${errorText(err)}`;
      pushMessage(set, tabId, "error", msg);
      toast("error", msg);
      return;
    }
  }
  const elapsedMs = Date.now() - startedAt;
  toast(
    "success",
    `Ran ${statements.length} statements (${applied} applied) in ${elapsedMs} ms`,
  );
}

function pushMessage(
  set: SetFn,
  tabId: string,
  level: LogMessage["level"],
  text: string,
) {
  set((s) => ({
    tabs: s.tabs.map((tab) =>
      tab.id === tabId
        ? {
            ...tab,
            messages: [
              ...tab.messages,
              { id: messageSeq++, level, text, at: Date.now() },
            ],
          }
        : tab,
    ),
  }));
}

// ---------------------------------------------------------------------------
// Debug handle: lets developers (and support) inspect and drive the store from
// the webview devtools, e.g. `__mieosql.getState().tabs`.
// ---------------------------------------------------------------------------

declare global {
  interface Window {
    __mieosql?: typeof useStore;
  }
}

if (typeof window !== "undefined") {
  window.__mieosql = useStore;
}
