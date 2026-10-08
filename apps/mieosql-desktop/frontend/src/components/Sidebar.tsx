import { useCallback, useEffect, useState } from "react";
import {
  Archive,
  Braces,
  CalendarClock,
  ChevronDown,
  ChevronRight,
  Cog,
  Database,
  Eye,
  FileCode2,
  Folder,
  Inbox,
  Layers,
  ListOrdered,
  Loader2,
  MessageSquare,
  Plug,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Star,
  Table2,
  X,
  Zap,
} from "lucide-react";

import { ContextMenu, type ContextMenuItem } from "./ContextMenu";
import { EngineGlyph } from "./EngineGlyph";
import { api, errorText } from "../lib/api";
import { confirmDialog, promptDialog } from "../lib/dialog";
import { openMessages } from "../lib/kafka";
import type {
  ConnectionProfile,
  DatabaseRef,
  Engine,
  ObjectKind,
  ObjectRef,
  SchemaRef,
  Scope,
  SessionInfo,
} from "../lib/bindings";
import { copyText, plainText } from "../lib/format";
import {
  canRenameTable,
  countSql,
  dropDatabaseSql,
  dropRoutineSql,
  dropSchemaSql,
  dropTableSql,
  dropViewSql,
  duplicateTableSql,
  isSqlEngine,
  qualified,
  quoteIdent,
  renameTableSql,
  selectStarSql,
  truncateTableSql,
} from "../lib/quoting";
import { exportTableData, TABLE_EXPORT_MENU } from "../lib/table-export";
import { useStore } from "../lib/store";

type Loadable<T> = T | "loading";

type Cache = {
  databases: Record<string, Loadable<DatabaseRef[]>>;
  schemas: Record<string, Loadable<SchemaRef[]>>;
  objects: Record<string, Loadable<ObjectRef[]>>;
};

const EMPTY_CACHE: Cache = { databases: {}, schemas: {}, objects: {} };

const FAVORITES_KEY = "mieosql.favorites.v1";

function loadFavorites(): Set<string> {
  try {
    const raw = localStorage.getItem(FAVORITES_KEY);
    const parsed = raw ? (JSON.parse(raw) as unknown) : [];
    return new Set(Array.isArray(parsed) ? (parsed as string[]) : []);
  } catch {
    return new Set();
  }
}

function dbKey(sid: string, db: string) {
  return `${sid}|${db}`;
}
function schemaKey(sid: string, db: string, schema: string) {
  return `${sid}|${db}|${schema}`;
}
function folderKey(sid: string, db: string, schema: string, kind: string) {
  return `${sid}|${db}|${schema}|${kind}`;
}

/** Message brokers get their own "Messaging" section in the tree. */
function isBroker(engine: Engine): boolean {
  return engine === "kafka";
}

function objectIcon(kind: ObjectKind, size = 11, selected = false): React.ReactNode {
  const tone = (normal: string) => (selected ? "text-white" : normal);
  switch (kind) {
    case "view":
      return <Eye size={size} className={tone("text-emerald-600")} />;
    case "materialized_view":
      return <Layers size={size} className={tone("text-amber-600")} />;
    case "sequence":
      return <ListOrdered size={size} className={tone("text-slate-400")} />;
    case "function":
      return <Braces size={size} className={tone("text-violet-600")} />;
    case "procedure":
      return <Cog size={size} className={tone("text-violet-600")} />;
    case "trigger":
      return <Zap size={size} className={tone("text-amber-600")} />;
    default:
      return <Table2 size={size} className={tone("text-sky-600")} />;
  }
}

type VirtualFolderId = "events" | "queries" | "backups";

function virtualFolderIcon(id: VirtualFolderId, size = 12): React.ReactNode {
  switch (id) {
    case "events":
      return <CalendarClock size={size} className="text-amber-600" />;
    case "queries":
      return <FileCode2 size={size} className="text-slate-500" />;
    case "backups":
      return <Archive size={size} className="text-slate-500" />;
  }
}

function TreeRow({
  depth,
  caret,
  icon,
  label,
  subtitle,
  trailing,
  expanded,
  selected,
  strong,
  loading,
  onClick,
  onToggle,
  onDoubleClick,
  onContextMenu,
}: {
  depth: number;
  caret?: boolean;
  icon: React.ReactNode;
  label: React.ReactNode;
  subtitle?: React.ReactNode;
  trailing?: React.ReactNode;
  expanded?: boolean;
  selected?: boolean;
  /** Solid accent highlight (Navicat-style) instead of the subtle tint. */
  strong?: boolean;
  loading?: boolean;
  onClick?: (e: React.MouseEvent) => void;
  onToggle?: () => void;
  onDoubleClick?: () => void;
  onContextMenu?: (e: React.MouseEvent) => void;
}) {
  return (
    <div
      className={`group flex min-h-7 cursor-default items-center gap-1 rounded-md pr-1 ${
        subtitle ? "py-[3px]" : "py-px"
      } ${
        selected
          ? strong
            ? "bg-accent text-white shadow-[inset_0_1px_0_rgba(255,255,255,0.08)]"
            : "bg-accent/[0.12] text-accent"
          : "text-slate-700 hover:bg-slate-900/[0.04]"
      }`}
      style={{ paddingLeft: 4 + depth * 13 }}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
    >
      <button
        className={`grid h-3.5 w-3.5 shrink-0 place-items-center rounded ${
          caret
            ? "text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
            : "invisible"
        }`}
        onClick={(e) => {
          e.stopPropagation();
          // Most rows only pass `onClick` (single-click toggles, double-click
          // reloads); the caret falls back to it so `>` is never a dead button.
          if (onToggle) onToggle();
          else onClick?.(e);
        }}
        tabIndex={-1}
      >
        {loading ? (
          <Loader2 size={10} className="animate-spin" />
        ) : expanded ? (
          <ChevronDown size={11} />
        ) : (
          <ChevronRight size={11} />
        )}
      </button>
      <span
        className={`shrink-0 ${
          selected ? (strong ? "text-white" : "text-accent") : "text-slate-500"
        }`}
      >
        {icon}
      </span>
      <span className="flex min-w-0 flex-1 flex-col leading-tight">
        <span className="truncate text-[13px] font-medium">{label}</span>
        {subtitle && (
          <span className="truncate text-[11.5px] text-slate-600">{subtitle}</span>
        )}
      </span>
      {trailing}
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="px-2 pt-3 pb-1 text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
      {children}
    </div>
  );
}

export function Sidebar() {
  const store = useStore();
  const [cache, setCache] = useState<Cache>(EMPTY_CACHE);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const [favorites, setFavorites] = useState<Set<string>>(loadFavorites);

  const toggle = useCallback((key: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  const toggleFavorite = useCallback((profileId: string) => {
    setFavorites((prev) => {
      const next = new Set(prev);
      if (next.has(profileId)) next.delete(profileId);
      else next.add(profileId);
      try {
        localStorage.setItem(FAVORITES_KEY, JSON.stringify([...next]));
      } catch {
        /* storage unavailable */
      }
      return next;
    });
  }, []);

  // ------------------------------------------------------------ loaders

  const loadDatabases = useCallback(async (sid: string) => {
    setCache((c) => ({
      ...c,
      databases: { ...c.databases, [sid]: "loading" },
    }));
    try {
      const databases = await api.listDatabases(sid);
      setCache((c) => ({
        ...c,
        databases: { ...c.databases, [sid]: databases },
      }));
    } catch (err) {
      setCache((c) => ({ ...c, databases: { ...c.databases, [sid]: [] } }));
      useStore.getState().toast("error", errorText(err));
    }
  }, []);

  const loadSchemas = useCallback(
    async (sid: string, db: string, engine: Engine) => {
      const key = dbKey(sid, db);
      // Engines without a schema level (MySQL, SQLite, SQL Server boxes...)
      // render the object folders directly — nothing to load.
      const caps = useStore
        .getState()
        .sessions.find((s) => s.id === sid)?.capabilities;
      if (caps && !caps.list_schemas) {
        setCache((c) => ({ ...c, schemas: { ...c.schemas, [key]: [] } }));
        return;
      }
      setCache((c) => ({ ...c, schemas: { ...c.schemas, [key]: "loading" } }));
      try {
        if (engine === "postgres") {
          // A PostgreSQL connection is pinned to one database: switch it.
          await api.useDatabase(sid, db);
          await useStore.getState().refreshSessionInfo(sid);
          // Objects cached for other databases of this session are stale.
          setCache((c) => {
            const objects = Object.fromEntries(
              Object.entries(c.objects).filter(
                ([k]) => !k.startsWith(`${sid}|`) || k.startsWith(`${sid}|${db}|`),
              ),
            ) as Cache["objects"];
            return { ...c, objects };
          });
        }
        const schemas = await api.listSchemas(sid, db);
        setCache((c) => ({
          ...c,
          schemas: { ...c.schemas, [key]: schemas },
        }));
      } catch (err) {
        setCache((c) => ({ ...c, schemas: { ...c.schemas, [key]: [] } }));
        useStore.getState().toast("error", errorText(err));
      }
    },
    [],
  );

  const loadObjects = useCallback(
    async (sid: string, db: string | null, schema: string | null, kind: ObjectKind) => {
      const key = folderKey(sid, db ?? "", schema ?? "", kind);
      setCache((c) => ({ ...c, objects: { ...c.objects, [key]: "loading" } }));
      try {
        const scope: Scope = {
          database: db ?? undefined,
          schema: schema ?? undefined,
        };
        const objects = await api.listObjects(
          sid,
          scope,
          kind,
          query.trim() || null,
        );
        setCache((c) => ({ ...c, objects: { ...c.objects, [key]: objects } }));
      } catch (err) {
        setCache((c) => ({ ...c, objects: { ...c.objects, [key]: [] } }));
        useStore.getState().toast("error", errorText(err));
      }
    },
    [query],
  );

  // Declarative loading: whenever a node is expanded and missing from the
  // cache, load it (also covers "refresh tree"). Broker sessions (Kafka)
  // render topics instead of databases — their `s:` keys load nothing.
  useEffect(() => {
    for (const key of expanded) {
      if (key.startsWith("s:")) {
        const sid = key.slice(2);
        const session = store.sessions.find((s) => s.id === sid);
        if (session && isBroker(session.engine)) continue;
        if (!cache.databases[sid]) void loadDatabases(sid);
      } else if (key.startsWith("db:")) {
        const [sid, db] = key.slice(3).split("|");
        const session = store.sessions.find((s) => s.id === sid);
        if (!sid || !db || !session) continue;
        if (!cache.schemas[dbKey(sid, db)]) {
          void loadSchemas(sid, db, session.engine);
        }
      } else if (key.startsWith("f:")) {
        const [sid, db, schema, kind] = key.slice(2).split("|");
        if (!sid || !kind) continue;
        if (!cache.objects[folderKey(sid, db ?? "", schema ?? "", kind)]) {
          void loadObjects(sid, db || null, schema || null, kind as ObjectKind);
        }
      }
    }
  }, [expanded, cache, store.sessions, loadDatabases, loadSchemas, loadObjects]);

  // Refresh tree event (from the toolbar / File menu).
  useEffect(() => {
    const handler = () => setCache(EMPTY_CACHE);
    window.addEventListener("mieosql:refresh-tree", handler);
    return () => window.removeEventListener("mieosql:refresh-tree", handler);
  }, []);

  // ------------------------------------------------------- context menus

  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    items: ContextMenuItem[];
  } | null>(null);

  const closeMenu = useCallback(() => setMenu(null), []);

  const openMenu = (e: React.MouseEvent, items: ContextMenuItem[]) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, items });
  };

  const refreshTreeEvent = () =>
    window.dispatchEvent(new CustomEvent("mieosql:refresh-tree"));

  const copyName = (name: string) => {
    void copyText(name).then((ok) =>
      useStore
        .getState()
        .toast(ok ? "success" : "error", ok ? `Copied "${name}"` : "Copy failed"),
    );
  };

  // Drop cached entries for a key prefix so the declarative loader refetches
  // the expanded nodes on the next render.
  const clearObjectCaches = (prefix: string) => {
    setCache((c) => {
      const schemas = Object.fromEntries(
        Object.entries(c.schemas).filter(([k]) => !k.startsWith(prefix)),
      ) as Cache["schemas"];
      const objects = Object.fromEntries(
        Object.entries(c.objects).filter(([k]) => !k.startsWith(prefix)),
      ) as Cache["objects"];
      return { ...c, schemas, objects };
    });
  };

  const refreshSession = (session: SessionInfo) => {
    setCache((c) => {
      const databases = { ...c.databases };
      delete databases[session.id];
      const prefix = `${session.id}|`;
      const schemas = Object.fromEntries(
        Object.entries(c.schemas).filter(([k]) => !k.startsWith(prefix)),
      ) as Cache["schemas"];
      const objects = Object.fromEntries(
        Object.entries(c.objects).filter(([k]) => !k.startsWith(prefix)),
      ) as Cache["objects"];
      return { databases, schemas, objects };
    });
  };

  const refreshDatabase = (session: SessionInfo, db: string) => {
    setCache((c) => {
      const schemas = { ...c.schemas };
      delete schemas[dbKey(session.id, db)];
      return { ...c, schemas };
    });
    clearObjectCaches(`${session.id}|${db}|`);
  };

  const refreshSchema = (session: SessionInfo, db: string, schema: string) =>
    clearObjectCaches(`${session.id}|${db}|${schema}|`);

  /** Execute small maintenance statements, then toast + reload the tree. */
  const runStatements = async (
    sessionId: string,
    statements: string[],
    successText: string,
    atomic = false,
  ) => {
    try {
      const result = await api.executeBatch(sessionId, statements, atomic);
      if (result.failed > 0) {
        useStore
          .getState()
          .toast("error", result.errors[0] ?? "Statement failed");
        return;
      }
      useStore.getState().toast("success", successText);
      refreshTreeEvent();
    } catch (err) {
      useStore.getState().toast("error", errorText(err));
    }
  };

  const openDdlTab = async (session: SessionInfo, scope: Scope, obj: ObjectRef) => {
    try {
      const ddl = await api.objectDdl(session.id, scope, obj.kind, obj.name);
      if (ddl) useStore.getState().newQueryTab(session.id, ddl);
      else useStore.getState().toast("info", "DDL is not available for this object yet");
    } catch (err) {
      useStore.getState().toast("error", errorText(err));
    }
  };

  const copyDdl = async (session: SessionInfo, scope: Scope, obj: ObjectRef) => {
    try {
      const ddl = await api.objectDdl(session.id, scope, obj.kind, obj.name);
      if (!ddl) {
        useStore.getState().toast("info", "DDL is not available for this object yet");
        return;
      }
      const ok = await copyText(ddl);
      useStore
        .getState()
        .toast(ok ? "success" : "error", ok ? "Copied CREATE statement" : "Copy failed");
    } catch (err) {
      useStore.getState().toast("error", errorText(err));
    }
  };

  // ------------------------------------------------- connection groups
  // Profiles carry an optional `group` folder + `color` dot. The tree shows
  // the group in the subtitle; these helpers implement
  // Edit / Duplicate / Copy to / Move to for both connected sessions and
  // disconnected profiles.

  const promptForGroup = async (current?: string | null): Promise<string | null> => {
    const next = await promptDialog("Group (empty = Ungrouped):", current ?? "", {
      title: "Move to group",
    });
    if (next === null) return null; // cancelled
    return next.trim();
  };

  const updateProfileGroup = (profile: ConnectionProfile, group: string) => {
    void (async () => {
      try {
        const full = await api.getProfile(profile.id);
        await useStore.getState().saveProfile({
          id: full.id,
          name: full.name,
          engine: full.engine,
          host: full.host,
          port: full.port,
          username: full.username,
          database: full.database,
          file: full.file,
          params: full.params,
          ssl_mode: full.ssl_mode,
          ssh: full.ssh,
          color: full.color,
          group: group || undefined,
          variant: full.variant,
          password: undefined,
          ssh_password: undefined,
        } as unknown as Parameters<typeof api.saveProfile>[0]);
        await useStore.getState().refreshProfiles();
        useStore
          .getState()
          .toast(
            "success",
            group ? `Moved "${profile.name}" to "${group}"` : `Moved "${profile.name}" to Ungrouped`,
          );
      } catch (err) {
        useStore.getState().toast("error", errorText(err));
      }
    })();
  };

  const duplicateConnection = (profile: ConnectionProfile, targetGroup?: string) => {
    void (async () => {
      try {
        const full = await api.getProfile(profile.id);
        const group = targetGroup !== undefined ? targetGroup : (full.group ?? undefined);
        await useStore.getState().saveProfile({
          name: `${full.name} copy`,
          engine: full.engine,
          host: full.host,
          port: full.port,
          username: full.username,
          database: full.database,
          file: full.file,
          params: full.params,
          ssl_mode: full.ssl_mode,
          ssh: full.ssh,
          color: full.color,
          group,
          variant: full.variant,
          // password intentionally not copied — user re-enters it.
          password: null,
        } as unknown as Parameters<typeof api.saveProfile>[0]);
        await useStore.getState().refreshProfiles();
        useStore.getState().toast("success", `Duplicated "${profile.name}"`);
      } catch (err) {
        useStore.getState().toast("error", errorText(err));
      }
    })();
  };

  const moveConnectionTo = (profile: ConnectionProfile) => {
    void (async () => {
      const next = await promptForGroup(profile.group);
      if (next === null) return;
      void updateProfileGroup(profile, next);
    })();
  };

  const copyConnectionTo = (profile: ConnectionProfile) => {
    void (async () => {
      const next = await promptForGroup(profile.group);
      if (next === null) return;
      duplicateConnection(profile, next);
    })();
  };

  const renameGroup = (group: string) => {
    void (async () => {
      const next = (await promptDialog(`Rename group "${group}" to:`, group, {
        title: "Rename group",
      }))?.trim();
      if (!next || next === group) return;
      try {
        const all = store.profiles.filter((p) => (p.group ?? "") === group);
        for (const p of all) {
          const full = await api.getProfile(p.id);
          await useStore.getState().saveProfile({
            id: full.id,
            name: full.name,
            engine: full.engine,
            host: full.host,
            port: full.port,
            username: full.username,
            database: full.database,
            file: full.file,
            params: full.params,
            ssl_mode: full.ssl_mode,
            ssh: full.ssh,
            color: full.color,
            group: next,
            variant: full.variant,
              password: undefined,
            ssh_password: undefined,
          } as unknown as Parameters<typeof api.saveProfile>[0]);
        }
        await useStore.getState().refreshProfiles();
        useStore.getState().toast("success", `Renamed group "${group}" to "${next}"`);
      } catch (err) {
        useStore.getState().toast("error", errorText(err));
      }
    })();
  };

  const groupHeaderMenu = (group: string): ContextMenuItem[] => [
    {
      kind: "item",
      label: "Rename Group…",
      onClick: () => renameGroup(group),
    },
    {
      kind: "item",
      label: "Ungroup All",
      onClick: () => {
        void (async () => {
          if (!(await confirmDialog(`Move all connections out of "${group}"?`, {
            title: "Ungroup all",
            kind: "warning",
          })))
            return;
          try {
            const all = useStore.getState().profiles.filter((p) => (p.group ?? "") === group);
            for (const p of all) {
              const full = await api.getProfile(p.id);
              await useStore.getState().saveProfile({
                id: full.id,
                name: full.name,
                engine: full.engine,
                host: full.host,
                port: full.port,
                username: full.username,
                database: full.database,
                file: full.file,
                params: full.params,
                ssl_mode: full.ssl_mode,
                ssh: full.ssh,
                color: full.color,
                group: undefined,
                variant: full.variant,
                      password: undefined,
                ssh_password: undefined,
              } as unknown as Parameters<typeof api.saveProfile>[0]);
            }
            await useStore.getState().refreshProfiles();
          } catch (err) {
            useStore.getState().toast("error", errorText(err));
          }
        })();
      },
    },
    { kind: "separator" },
    { kind: "item", label: "Copy Name", onClick: () => copyName(group) },
  ];

  const sessionMenu = (session: SessionInfo): ContextMenuItem[] => {
    if (isBroker(session.engine)) return brokerSessionMenu(session);
    const profile = store.profiles.find((p) => p.id === session.profile_id);
    const sqlEngine = isSqlEngine(session.engine);
    const items: ContextMenuItem[] = [
      { kind: "item", label: "New Query", onClick: () => store.newQueryTab(session.id) },
      {
        kind: "item",
        label: "New Table…",
        onClick: () =>
          useStore.getState().openObjectDialog("new-table", {
            sessionId: session.id,
            scope: session.database ? { database: session.database } : {},
          }),
      },
    ];
    if (sqlEngine && session.engine !== "sqlite") {
      items.push({
        kind: "item",
        label: "New Database…",
        onClick: () =>
          useStore.getState().openObjectDialog("new-database", { sessionId: session.id }),
      });
    }
    items.push(
      { kind: "item", label: "Refresh", onClick: () => refreshSession(session) },
      { kind: "separator" },
      {
        kind: "item",
        label: "Backup Database…",
        onClick: () =>
          useStore.getState().openObjectDialog("backup", {
            sessionId: session.id,
            scope: session.database ? { database: session.database } : {},
          }),
      },
      {
        kind: "item",
        label: "Restore SQL Script…",
        onClick: () =>
          useStore.getState().openObjectDialog("restore", { sessionId: session.id }),
      },
      {
        kind: "item",
        label: "Users & Roles…",
        onClick: () => {
          store.setActiveSession(session.id);
          store.openFeatureTab("users", "Users & Roles", { sessionId: session.id });
        },
      },
      { kind: "separator" },
      { kind: "item", label: "Copy Name", onClick: () => copyName(session.name) },
    );
    if (profile) {
      items.push(
        { kind: "separator" },
        { kind: "label", label: "Connection" },
        {
          kind: "item",
          label: "Edit Connection…",
          onClick: () => store.setConnectionModal(true, profile.id),
        },
        {
          kind: "item",
          label: "Duplicate Connection",
          onClick: () => duplicateConnection(profile),
        },
        {
          kind: "item",
          label: "Copy Connection To…",
          onClick: () => copyConnectionTo(profile),
        },
        {
          kind: "item",
          label: "Move Connection To…",
          onClick: () => moveConnectionTo(profile),
        },
      );
    }
    items.push(
      { kind: "separator" },
      {
        kind: "item",
        label: "Disconnect",
        danger: true,
        onClick: () => void store.closeSession(session.id),
      },
    );
    return items;
  };

  const databaseMenu = (
    session: SessionInfo,
    db: DatabaseRef,
  ): ContextMenuItem[] => {
    const scope: Scope = { database: db.name };
    const sqlEngine = isSqlEngine(session.engine);
    const items: ContextMenuItem[] = [
      { kind: "item", label: "New Query", onClick: () => store.newQueryTab(session.id) },
      {
        kind: "item",
        label: "Open Tables",
        onClick: () => store.openObjects(session.id, scope, "table", `${db.name} · Tables`),
      },
      { kind: "item", label: "Refresh", onClick: () => refreshDatabase(session, db.name) },
    ];
    if (sqlEngine) {
      items.push({ kind: "separator" });
      items.push({
        kind: "item",
        label: "New Table…",
        onClick: () =>
          useStore.getState().openObjectDialog("new-table", {
            sessionId: session.id,
            scope,
          }),
      });
      if (session.engine !== "sqlite") {
        items.push({
          kind: "item",
          label: "New Database…",
          onClick: () =>
            useStore
              .getState()
              .openObjectDialog("new-database", { sessionId: session.id }),
        });
      }
      items.push({
        kind: "item",
        label: "Import Data…",
        onClick: () =>
          useStore.getState().openObjectDialog("import", {
            sessionId: session.id,
            scope,
          }),
      });
      items.push({
        kind: "item",
        label: "Backup Database…",
        onClick: () =>
          useStore
            .getState()
            .openObjectDialog("backup", {
              sessionId: session.id,
              scope: { database: db.name },
            }),
      });
      items.push({
        kind: "item",
        label: "Restore SQL Script…",
        onClick: () =>
          useStore.getState().openObjectDialog("restore", { sessionId: session.id }),
      });
      items.push({
        kind: "item",
        label: "Data Dictionary…",
        onClick: () => {
          store.setActiveSession(session.id);
          store.openFeatureDialog("docs");
        },
      });
    }
    items.push({ kind: "separator" });
    items.push({ kind: "item", label: "Copy Name", onClick: () => copyName(db.name) });
    items.push({
      kind: "item",
      label: "Copy SELECT *",
      onClick: () => {
        if (!sqlEngine) return;
        const sql = `SELECT * FROM ${qualified(session.engine, scope, "<table>")} LIMIT 100;`;
        void copyText(sql).then((ok) =>
          useStore.getState().toast(ok ? "success" : "error", ok ? "Copied SELECT template" : "Copy failed"),
        );
      },
    });
    if (sqlEngine) {
      items.push({ kind: "separator" });
      items.push({
        kind: "item",
        label: "Empty Database… (truncate all tables)",
        danger: true,
        onClick: () => void emptyScopeTables(session, scope, `database "${db.name}"`),
      });
    }
    const dropSql = dropDatabaseSql(session.engine, db.name);
    if (dropSql) {
      items.push({ kind: "separator" });
      items.push({
        kind: "item",
        label: "Drop Database…",
        danger: true,
        onClick: () => {
          void (async () => {
            if (
              !(await confirmDialog(
                `Drop database "${db.name}"? This cannot be undone.`,
                { title: "Drop database", kind: "warning" },
              ))
            ) {
              return;
            }
            void runStatements(session.id, [dropSql], `Dropped database "${db.name}"`);
          })();
        },
      });
    }
    return items;
  };

  const emptyScopeTables = async (session: SessionInfo, scope: Scope, label: string) => {
    try {
      const tables = await api.listObjects(session.id, scope, "table", null);
      if (tables.length === 0) {
        useStore.getState().toast("info", `No tables in ${label}`);
        return;
      }
      if (
        !(await confirmDialog(
          `Truncate ${tables.length} table(s) in ${label}? All rows will be deleted. This cannot be undone.`,
          { title: "Truncate tables", kind: "warning" },
        ))
      ) {
        return;
      }
      const statements: string[] = [];
      for (const t of tables) {
        const tableScope: Scope = {
          database: scope.database ?? t.database ?? undefined,
          schema: t.schema ?? scope.schema,
        };
        const sql = truncateTableSql(session.engine, tableScope, t.name);
        if (sql) statements.push(sql);
      }
      if (statements.length === 0) {
        useStore.getState().toast("info", "Truncate is not supported for this engine");
        return;
      }
      await runStatements(session.id, statements, `Emptied ${tables.length} table(s) in ${label}`);
    } catch (err) {
      useStore.getState().toast("error", errorText(err));
    }
  };

  const openSelectQuery = (session: SessionInfo, scope: Scope, table: string) => {
    const sql = selectStarSql(session.engine, scope, table, 100);
    if (!sql) {
      useStore.getState().toast("info", "Generate SQL is only available for SQL engines");
      return;
    }
    useStore.getState().newQueryTab(session.id, sql);
  };

  const openCountQuery = (session: SessionInfo, scope: Scope, table: string) => {
    const sql = countSql(session.engine, scope, table);
    if (!sql) {
      useStore.getState().toast("info", "Row count is only available for SQL engines");
      return;
    }
    useStore.getState().newQueryTab(session.id, sql);
  };

  const showRowCount = async (session: SessionInfo, scope: Scope, table: string) => {
    const sql = countSql(session.engine, scope, table);
    if (!sql) {
      useStore.getState().toast("info", "Row count is only available for SQL engines");
      return;
    }
    try {
      const out = await api.fetchRows(session.id, sql, 1);
      const first = out.rows[0]?.[0];
      const text = first ? plainText(first) : "0";
      useStore.getState().toast("success", `"${table}" has ${Number(text).toLocaleString()} row(s)`);
    } catch (err) {
      useStore.getState().toast("error", errorText(err));
    }
  };

  const openInsertTemplate = async (session: SessionInfo, scope: Scope, table: string) => {
    try {
      const cols = await api.tableColumns(session.id, scope, table);
      const names = cols.map((c) => c.name);
      const q = qualified(session.engine, scope, table);
      if (names.length === 0) {
        useStore.getState().newQueryTab(session.id, `INSERT INTO ${q} DEFAULT VALUES;`);
        return;
      }
      const colList = names.map((n) => quoteIdent(session.engine, n)).join(", ");
      const placeholders = names.map(() => "NULL").join(", ");
      useStore.getState().newQueryTab(session.id, `INSERT INTO ${q} (${colList})\nVALUES (${placeholders});`);
    } catch (err) {
      useStore.getState().toast("error", errorText(err));
    }
  };

  const openUpdateTemplate = async (session: SessionInfo, scope: Scope, table: string) => {
    try {
      const cols = await api.tableColumns(session.id, scope, table);
      const names = cols.map((c) => c.name);
      const q = qualified(session.engine, scope, table);
      if (names.length === 0) {
        useStore.getState().newQueryTab(session.id, `SELECT * FROM ${q} LIMIT 100;`);
        return;
      }
      const first = quoteIdent(session.engine, names[0]!);
      const setList = names.map((n) => `${quoteIdent(session.engine, n)} = NULL`).join(",\n  ");
      useStore.getState().newQueryTab(session.id, `UPDATE ${q}\nSET ${setList}\nWHERE ${first} = NULL; -- adjust condition`);
    } catch (err) {
      useStore.getState().toast("error", errorText(err));
    }
  };

  const openDeleteTemplate = (session: SessionInfo, scope: Scope, table: string) => {
    const q = qualified(session.engine, scope, table);
    useStore.getState().newQueryTab(session.id, `DELETE FROM ${q}\nWHERE 1 = 0; -- adjust condition`);
  };

  const duplicateTable = (session: SessionInfo, scope: Scope, table: string) => {
    void (async () => {
      const next = (
        await promptDialog(`Duplicate table "${table}" as:`, `${table}_copy`, {
          title: "Duplicate table",
        })
      )?.trim();
      if (!next) return;
      const sql = duplicateTableSql(session.engine, scope, table, next);
      if (!sql) {
        useStore.getState().toast("info", "Duplicate is only available for SQL engines");
        return;
      }
      void runStatements(session.id, [sql], `Duplicated "${table}" as "${next}"`);
    })();
  };

  const schemaMenu = (
    session: SessionInfo,
    db: string,
    schema: string,
  ): ContextMenuItem[] => {
    const scope: Scope = { database: db, schema };
    const sqlEngine = isSqlEngine(session.engine);
    const items: ContextMenuItem[] = [
      { kind: "item", label: "New Query", onClick: () => store.newQueryTab(session.id) },
      {
        kind: "item",
        label: "Open Tables",
        onClick: () => store.openObjects(session.id, scope, "table", `${schema} · Tables`),
      },
      {
        kind: "item",
        label: "Refresh",
        onClick: () => refreshSchema(session, db, schema),
      },
    ];
    if (sqlEngine) {
      items.push({ kind: "separator" });
      items.push({
        kind: "item",
        label: "New Table…",
        onClick: () =>
          useStore.getState().openObjectDialog("new-table", {
            sessionId: session.id,
            scope,
          }),
      });
      items.push({
        kind: "item",
        label: "Import Data…",
        onClick: () =>
          useStore.getState().openObjectDialog("import", {
            sessionId: session.id,
            scope,
          }),
      });
      items.push({
        kind: "item",
        label: "Backup Schema…",
        onClick: () =>
          useStore.getState().openObjectDialog("backup", {
            sessionId: session.id,
            scope,
          }),
      });
      items.push({
        kind: "item",
        label: "Data Dictionary…",
        onClick: () => {
          store.setActiveSession(session.id);
          store.openFeatureDialog("docs");
        },
      });
    }
    items.push({ kind: "separator" });
    items.push({ kind: "item", label: "Copy Name", onClick: () => copyName(schema) });
    if (sqlEngine) {
      items.push({ kind: "separator" });
      items.push({
        kind: "item",
        label: "Empty Schema… (truncate all tables)",
        danger: true,
        onClick: () => void emptyScopeTables(session, scope, `schema "${schema}"`),
      });
      const dropSql = dropSchemaSql(session.engine, schema);
      if (dropSql) {
        items.push({
          kind: "item",
          label: "Drop Schema…",
          danger: true,
          onClick: () => {
            void (async () => {
              if (
                !(await confirmDialog(
                  `Drop schema "${schema}"? This cannot be undone.`,
                  { title: "Drop schema", kind: "warning" },
                ))
              )
                return;
              void runStatements(session.id, [dropSql], `Dropped schema "${schema}"`);
            })();
          },
        });
      }
    }
    return items;
  };

  const folderMenu = (
    session: SessionInfo,
    db: string | null,
    schema: string | null,
    kind: ObjectKind,
    label: string,
  ): ContextMenuItem[] => {
    const scope: Scope = {
      database: db ?? undefined,
      schema: schema ?? undefined,
    };
    const sqlEngine = isSqlEngine(session.engine);
    const items: ContextMenuItem[] = [];
    items.push({
      kind: "item",
      label: `Open ${label}`,
      onClick: () =>
        store.openObjects(session.id, scope, kind, `${db ?? schema ?? session.name} · ${label}`),
    });
    if (kind === "table" && sqlEngine) {
      items.push({
        kind: "item",
        label: "New Table…",
        onClick: () =>
          useStore.getState().openObjectDialog("new-table", {
            sessionId: session.id,
            scope: {
              database: db ?? undefined,
              schema: schema ?? undefined,
            },
          }),
      });
      items.push({ kind: "separator" });
      items.push({
        kind: "item",
        label: "Import Data…",
        onClick: () =>
          useStore.getState().openObjectDialog("import", {
            sessionId: session.id,
            scope,
          }),
      });
      items.push({
        kind: "item",
        label: "Backup…",
        onClick: () =>
          useStore.getState().openObjectDialog("backup", {
            sessionId: session.id,
            scope,
          }),
      });
      items.push({
        kind: "item",
        label: "Empty All Tables…",
        danger: true,
        onClick: () => void emptyScopeTables(session, scope, label.toLowerCase()),
      });
    }
    items.push({
      kind: "item",
      label: "Refresh",
      onClick: () => void loadObjects(session.id, db, schema, kind),
    });
    items.push({ kind: "separator" });
    items.push({ kind: "item", label: "Copy Name", onClick: () => copyName(label) });
    return items;
  };

  const exportItems = (
    session: SessionInfo,
    scope: Scope,
    table: string,
  ): ContextMenuItem[] =>
    TABLE_EXPORT_MENU.map(({ format, label }) => ({
      kind: "item" as const,
      label,
      onClick: () =>
        void exportTableData(session.id, session.engine, scope, table, format),
    }));

  const objectMenu = (
    session: SessionInfo,
    db: string | null,
    schema: string | null,
    obj: ObjectRef,
  ): ContextMenuItem[] => {
    const scope: Scope = {
      database: db ?? undefined,
      schema: schema ?? undefined,
    };
    const sqlEngine = isSqlEngine(session.engine);
    const items: ContextMenuItem[] = [];

    if (obj.kind === "table" && sqlEngine) {
      items.push(
        {
          kind: "item",
          label: "Open Data",
          onClick: () => store.openTable(session.id, scope, obj.name),
        },
        {
          kind: "item",
          label: "Row Count",
          onClick: () => void showRowCount(session, scope, obj.name),
        },
        { kind: "separator" },
        { kind: "label", label: "Query" },
        { kind: "item", label: "New Query", onClick: () => store.newQueryTab(session.id) },
        {
          kind: "item",
          label: "Generate SELECT *",
          onClick: () => openSelectQuery(session, scope, obj.name),
        },
        {
          kind: "item",
          label: "Generate SELECT COUNT(*)",
          onClick: () => openCountQuery(session, scope, obj.name),
        },
        {
          kind: "item",
          label: "Generate INSERT",
          onClick: () => void openInsertTemplate(session, scope, obj.name),
        },
        {
          kind: "item",
          label: "Generate UPDATE",
          onClick: () => void openUpdateTemplate(session, scope, obj.name),
        },
        {
          kind: "item",
          label: "Generate DELETE",
          onClick: () => openDeleteTemplate(session, scope, obj.name),
        },
        {
          kind: "item",
          label: "Generate DDL",
          onClick: () => void openDdlTab(session, scope, obj),
        },
        { kind: "separator" },
        { kind: "label", label: "Data" },
        {
          kind: "item",
          label: "Import Data…",
          onClick: () =>
            useStore.getState().openObjectDialog("import", {
              sessionId: session.id,
              scope,
              table: obj.name,
            }),
        },
        ...exportItems(session, scope, obj.name),
        {
          kind: "item",
          label: "Backup Table…",
          onClick: () =>
            useStore
              .getState()
              .openObjectDialog("backup", { sessionId: session.id, scope }),
        },
        {
          kind: "item",
          label: "Generate Test Data…",
          onClick: () => {
            store.setActiveSession(session.id);
            store.openFeatureDialog("datagen");
          },
        },
        { kind: "separator" },
        { kind: "label", label: "Copy" },
        { kind: "item", label: "Copy Name", onClick: () => copyName(obj.name) },
        {
          kind: "item",
          label: "Copy SELECT *",
          onClick: () => {
            const sql = selectStarSql(session.engine, scope, obj.name, 100) ?? "";
            void copyText(sql).then((ok) =>
              useStore.getState().toast(ok ? "success" : "error", ok ? "Copied SELECT" : "Copy failed"),
            );
          },
        },
        {
          kind: "item",
          label: "Copy CREATE Statement",
          onClick: () => void copyDdl(session, scope, obj),
        },
        { kind: "separator" },
        { kind: "label", label: "Manage" },
        {
          kind: "item",
          label: "Edit Table…",
          onClick: () =>
            useStore.getState().openObjectDialog("edit-table", {
              sessionId: session.id,
              scope,
              table: obj.name,
            }),
        },
        {
          kind: "item",
          label: "Duplicate Table…",
          onClick: () => duplicateTable(session, scope, obj.name),
        },
      );
      if (canRenameTable(session.engine)) {
        items.push({
          kind: "item",
          label: "Rename Table…",
          onClick: () => {
            void (async () => {
              const next = (
                await promptDialog(`Rename table "${obj.name}" to:`, obj.name, {
                  title: "Rename table",
                })
              )?.trim();
              if (!next || next === obj.name) return;
              const renameSql = renameTableSql(session.engine, scope, obj.name, next);
              if (!renameSql) return;
              void runStatements(
                session.id,
                [renameSql],
                `Renamed "${obj.name}" to "${next}"`,
              );
            })();
          },
        });
      }
      items.push(
        {
          kind: "item",
          label: "Truncate Table…",
          danger: true,
          onClick: () => {
            void (async () => {
              if (
                !(await confirmDialog(
                  `Truncate table "${obj.name}"? All rows will be deleted.`,
                  { title: "Truncate table", kind: "warning" },
                ))
              ) {
                return;
              }
              const truncateSql = truncateTableSql(session.engine, scope, obj.name);
              if (truncateSql) {
                void runStatements(
                  session.id,
                  [truncateSql],
                  `Truncated "${obj.name}"`,
                  true,
                );
              }
            })();
          },
        },
        {
          kind: "item",
          label: "Drop Table…",
          danger: true,
          onClick: () => {
            void (async () => {
              if (
                !(await confirmDialog(
                  `Drop table "${obj.name}"? This cannot be undone.`,
                  { title: "Drop table", kind: "warning" },
                ))
              ) {
                return;
              }
              const dropSql = dropTableSql(session.engine, scope, obj.name);
              if (dropSql) {
                void runStatements(session.id, [dropSql], `Dropped "${obj.name}"`);
              }
            })();
          },
        },
      );
      return items;
    }

    if ((obj.kind === "view" || obj.kind === "materialized_view") && sqlEngine) {
      items.push(
        {
          kind: "item",
          label: "Open Data",
          onClick: () => store.openTable(session.id, scope, obj.name),
        },
        {
          kind: "item",
          label: "Row Count",
          onClick: () => void showRowCount(session, scope, obj.name),
        },
        { kind: "separator" },
        {
          kind: "item",
          label: "Generate SELECT *",
          onClick: () => openSelectQuery(session, scope, obj.name),
        },
        {
          kind: "item",
          label: "Generate DDL",
          onClick: () => void openDdlTab(session, scope, obj),
        },
        { kind: "separator" },
        ...exportItems(session, scope, obj.name),
        { kind: "separator" },
        { kind: "item", label: "Copy Name", onClick: () => copyName(obj.name) },
        {
          kind: "item",
          label: "Copy CREATE Statement",
          onClick: () => void copyDdl(session, scope, obj),
        },
        { kind: "separator" },
        {
          kind: "item",
          label: `Drop ${obj.kind === "view" ? "View" : "Materialized View"}…`,
          danger: true,
          onClick: () => {
            void (async () => {
              if (
                !(await confirmDialog(`Drop "${obj.name}"? This cannot be undone.`, {
                  title: "Drop view",
                  kind: "warning",
                }))
              ) {
                return;
              }
              const dropSql = dropViewSql(session.engine, scope, obj.name, obj.kind === "materialized_view");
              if (dropSql) {
                void runStatements(session.id, [dropSql], `Dropped "${obj.name}"`);
              }
            })();
          },
        },
      );
      return items;
    }

    if (obj.kind === "sequence" && sqlEngine) {
      items.push(
        {
          kind: "item",
          label: "Generate DDL",
          onClick: () => void openDdlTab(session, scope, obj),
        },
        { kind: "separator" },
        { kind: "item", label: "Copy Name", onClick: () => copyName(obj.name) },
        {
          kind: "item",
          label: "Copy CREATE Statement",
          onClick: () => void copyDdl(session, scope, obj),
        },
      );
      return items;
    }

    if (
      (obj.kind === "function" || obj.kind === "procedure" || obj.kind === "trigger") &&
      sqlEngine
    ) {
      const routineLabel =
        obj.kind === "function" ? "Function" : obj.kind === "procedure" ? "Procedure" : "Trigger";
      items.push(
        {
          kind: "item",
          label: "Generate DDL",
          onClick: () => void openDdlTab(session, scope, obj),
        },
        { kind: "separator" },
        { kind: "item", label: "Copy Name", onClick: () => copyName(obj.name) },
        {
          kind: "item",
          label: "Copy CREATE Statement",
          onClick: () => void copyDdl(session, scope, obj),
        },
      );
      const dropSql = dropRoutineSql(session.engine, scope, obj.name, obj.kind);
      if (dropSql) {
        items.push(
          { kind: "separator" },
          {
            kind: "item",
            label: `Drop ${routineLabel}…`,
            danger: true,
            onClick: () => {
              void (async () => {
                if (
                  !(await confirmDialog(
                    `Drop ${routineLabel.toLowerCase()} "${obj.name}"? This cannot be undone.`,
                    { title: `Drop ${routineLabel.toLowerCase()}`, kind: "warning" },
                  ))
                ) {
                  return;
                }
                void runStatements(session.id, [dropSql], `Dropped "${obj.name}"`);
              })();
            },
          },
        );
      }
      return items;
    }

    // Views, materialized views, sequences (and tables on non-SQL engines).
    items.push(
      {
        kind: "item",
        label: "Open",
        onClick: () => store.openTable(session.id, scope, obj.name),
      },
      {
        kind: "item",
        label: "Generate DDL",
        onClick: () => void openDdlTab(session, scope, obj),
      },
    );
    if (sqlEngine && (obj.kind === "view" || obj.kind === "materialized_view")) {
      items.push({ kind: "separator" }, ...exportItems(session, scope, obj.name));
    }
    items.push(
      { kind: "separator" },
      { kind: "item", label: "Copy Name", onClick: () => copyName(obj.name) },
    );
    return items;
  };

  const profileMenu = (profile: ConnectionProfile): ContextMenuItem[] => {
    const session = store.sessions.find((s) => s.profile_id === profile.id);
    if (session) {
      return [
        {
          kind: "item",
          label: "New Query",
          onClick: () => store.newQueryTab(session.id),
        },
        {
          kind: "item",
          label: "Refresh",
          onClick: () => refreshSession(session),
        },
        { kind: "item", label: "Copy Name", onClick: () => copyName(profile.name) },
        {
          kind: "item",
          label: "Edit Connection…",
          onClick: () => store.setConnectionModal(true, profile.id),
        },
        { kind: "separator" },
        {
          kind: "item",
          label: "Disconnect",
          danger: true,
          onClick: () => void store.closeSession(session.id),
        },
      ];
    }
    return [
      {
        kind: "item",
        label: "Connect",
        onClick: () =>
          void store.openSession(profile.id).catch((err) => {
            store.toast("error", errorText(err));
          }),
      },
      {
        kind: "item",
        label: "Test Connection",
        onClick: () => {
          void api
            .testProfile(profile.id)
            .then((info) =>
              useStore.getState().toast("success", `OK — ${info.version ?? "connected"}`),
            )
            .catch((err) => useStore.getState().toast("error", errorText(err)));
        },
      },
      {
        kind: "item",
        label: "Edit Connection…",
        onClick: () => store.setConnectionModal(true, profile.id),
      },
      {
        kind: "item",
        label: "Duplicate Connection",
        onClick: () => duplicateConnection(profile),
      },
      {
        kind: "item",
        label: "Copy Connection To…",
        onClick: () => copyConnectionTo(profile),
      },
      {
        kind: "item",
        label: "Move Connection To…",
        onClick: () => moveConnectionTo(profile),
      },
      { kind: "separator" },
      { kind: "item", label: "Copy Name", onClick: () => copyName(profile.name) },
      { kind: "separator" },
      {
        kind: "item",
        label: "Delete Connection",
        danger: true,
        onClick: () => {
          void (async () => {
            if (
              !(await confirmDialog(`Delete connection "${profile.name}"?`, {
                title: "Delete connection",
                kind: "warning",
              }))
            )
              return;
            void store.deleteProfile(profile.id).catch((err) => {
              store.toast("error", errorText(err));
            });
          })();
        },
      },
    ];
  };

  const selected = store.selected;
  const isSelected = (sid: string, db: string | null, schema: string | null, table: string) =>
    selected !== null &&
    selected.sessionId === sid &&
    selected.table === table &&
    (selected.scope.database ?? null) === db &&
    (selected.scope.schema ?? null) === schema;

  const filterMatch = (name: string) =>
    !query.trim() || name.toLowerCase().includes(query.trim().toLowerCase());

  // -------------------------------------------------------------- render

  const starButton = (profileId: string | null | undefined) => {
    if (!profileId) return undefined;
    const fav = favorites.has(profileId);
    return (
      <button
        className={`${fav ? "grid" : "hidden group-hover:grid"} h-5 w-5 shrink-0 place-items-center rounded-md hover:bg-slate-900/[0.06]`}
        title={fav ? "Remove from favorites" : "Add to favorites"}
        onClick={(e) => {
          e.stopPropagation();
          toggleFavorite(profileId);
        }}
      >
        <Star
          size={11}
          className={fav ? "fill-amber-400 text-amber-500" : "text-slate-400"}
        />
      </button>
    );
  };

  const virtualFolderMenu = (
    session: SessionInfo,
    db: string | null,
    schema: string | null,
    id: VirtualFolderId,
    label: string,
  ): ContextMenuItem[] => {
    const scope: Scope = {
      database: db ?? undefined,
      schema: schema ?? undefined,
    };
    const items: ContextMenuItem[] = [];
    if (id === "events") {
      items.push({
        kind: "item",
        label: "Show Events",
        onClick: () =>
          useStore.getState().newQueryTab(session.id, "SHOW EVENTS;"),
      });
      items.push({
        kind: "item",
        label: "Create Event…",
        onClick: () =>
          useStore.getState().newQueryTab(
            session.id,
            "CREATE EVENT `event_name`\nON SCHEDULE EVERY 1 DAY\nDO\n  SELECT 1;",
          ),
      });
    } else if (id === "queries") {
      items.push({
        kind: "item",
        label: "New Query",
        onClick: () => store.newQueryTab(session.id),
      });
      items.push({
        kind: "item",
        label: "Open Snippets",
        onClick: () => useStore.getState().setRightTab("snippets"),
      });
      items.push({
        kind: "item",
        label: "Open History",
        onClick: () => useStore.getState().setRightTab("history"),
      });
    } else {
      items.push({
        kind: "item",
        label: "Backup…",
        onClick: () =>
          useStore.getState().openObjectDialog("backup", {
            sessionId: session.id,
            scope,
          }),
      });
      items.push({
        kind: "item",
        label: "Restore SQL Script…",
        onClick: () =>
          useStore.getState().openObjectDialog("restore", { sessionId: session.id }),
      });
    }
    items.push({ kind: "separator" });
    items.push({ kind: "item", label: "Copy Name", onClick: () => copyName(label) });
    return items;
  };

  const renderVirtualFolder = (
    session: SessionInfo,
    db: string | null,
    schema: string | null,
    id: VirtualFolderId,
    label: string,
    depth: number,
  ) => {
    const vk = `v:${session.id}|${db ?? ""}|${schema ?? ""}|${id}`;
    const isOpen = expanded.has(vk);
    const scope: Scope = {
      database: db ?? undefined,
      schema: schema ?? undefined,
    };
    const rows: { label: string; action: () => void }[] =
      id === "events"
        ? [
            {
              label: "Show Events",
              action: () => useStore.getState().newQueryTab(session.id, "SHOW EVENTS;"),
            },
            {
              label: "Create Event…",
              action: () =>
                useStore.getState().newQueryTab(
                  session.id,
                  "CREATE EVENT `event_name`\nON SCHEDULE EVERY 1 DAY\nDO\n  SELECT 1;",
                ),
            },
          ]
        : id === "queries"
          ? [
              {
                label: "New Query",
                action: () => store.newQueryTab(session.id),
              },
              {
                label: "Snippets",
                action: () => useStore.getState().setRightTab("snippets"),
              },
              {
                label: "History",
                action: () => useStore.getState().setRightTab("history"),
              },
            ]
          : [
              {
                label: "Backup…",
                action: () =>
                  useStore.getState().openObjectDialog("backup", {
                    sessionId: session.id,
                    scope,
                  }),
              },
              {
                label: "Restore SQL Script…",
                action: () =>
                  useStore.getState().openObjectDialog("restore", { sessionId: session.id }),
              },
              {
                label: "Data Dictionary…",
                action: () => {
                  store.setActiveSession(session.id);
                  store.openFeatureDialog("docs");
                },
              },
            ];
    return (
      <div key={vk}>
        <TreeRow
          depth={depth}
          caret
          expanded={isOpen}
          icon={virtualFolderIcon(id, 12)}
          label={<span>{label}</span>}
          onClick={() => toggle(vk)}
          onContextMenu={(e) => openMenu(e, virtualFolderMenu(session, db, schema, id, label))}
        />
        {isOpen &&
          rows.map((r) => (
            <TreeRow
              key={`${vk}|${r.label}`}
              depth={depth + 1}
              icon={virtualFolderIcon(id, 11)}
              label={<span className="text-slate-600">{r.label}</span>}
              onClick={() => r.action()}
              onDoubleClick={() => r.action()}
              onContextMenu={(e) => openMenu(e, virtualFolderMenu(session, db, schema, id, label))}
            />
          ))}
      </div>
    );
  };

  const renderFolders = (
    session: SessionInfo,
    db: string | null,
    schema: string | null,
    depth: number,
  ) => {
    const sqlEngine = isSqlEngine(session.engine);
    const folders: { kind: ObjectKind; label: string }[] = [
      { kind: "table", label: "Tables" },
      { kind: "view", label: "Views" },
      // Stored routines — hidden on SQLite (no routines) and non-SQL engines
      // (their tree shows collections/keys/topics instead).
      ...(sqlEngine && session.engine !== "sqlite"
        ? [
            { kind: "function" as ObjectKind, label: "Functions" },
            { kind: "procedure" as ObjectKind, label: "Procedures" },
            { kind: "trigger" as ObjectKind, label: "Triggers" },
          ]
        : []),
      // SQLite does support triggers — keep that folder visible there.
      ...(session.engine === "sqlite"
        ? [{ kind: "trigger" as ObjectKind, label: "Triggers" }]
        : []),
      // PostgreSQL exposes these through the same introspection query today.
      ...(session.engine === "postgres"
        ? [
            { kind: "materialized_view" as ObjectKind, label: "Mat. Views" },
            { kind: "sequence" as ObjectKind, label: "Sequences" },
          ]
        : []),
      ...(session.engine === "sql_server"
        ? [{ kind: "sequence" as ObjectKind, label: "Sequences" }]
        : []),
    ];
    const realFolders = folders.map((folder) => {
      const fk = folderKey(session.id, db ?? "", schema ?? "", folder.kind);
      const expandKey = `f:${fk}`;
      const isOpen = expanded.has(expandKey);
      const objects = cache.objects[fk];
      const list = Array.isArray(objects) ? objects : [];
      const shown = list.filter((o) => filterMatch(o.name));
      return (
        <div key={fk}>
          <TreeRow
            depth={depth}
            caret
            expanded={isOpen}
            loading={objects === "loading"}
            icon={objectIcon(folder.kind, 12)}
            label={
              <span>
                {folder.label}
                {Array.isArray(objects) && (
                  <span className="text-slate-400"> ({shown.length})</span>
                )}
              </span>
            }
            onClick={(e) => {
              if (e.detail === 2) {
                void loadObjects(session.id, db, schema, folder.kind);
                return;
              }
              toggle(expandKey);
            }}
            onContextMenu={(e) =>
              openMenu(e, folderMenu(session, db, schema, folder.kind, folder.label))
            }
          />
          {isOpen &&
            shown.map((obj) => {
              const selected = isSelected(session.id, db, schema, obj.name);
              return (
                <TreeRow
                  key={`${fk}|${obj.name}`}
                  depth={depth + 1}
                  icon={objectIcon(obj.kind, 11, selected)}
                  selected={selected}
                  strong
                  label={<span>{obj.name}</span>}
                  trailing={
                    obj.rows_estimate !== undefined && obj.rows_estimate !== null ? (
                      <span
                        className={`text-[11px] ${
                          selected ? "text-white/80" : "text-slate-400"
                        }`}
                      >
                        {obj.rows_estimate.toLocaleString()}
                      </span>
                    ) : undefined
                  }
                  onClick={() =>
                    store.selectObject({
                      sessionId: session.id,
                      scope: {
                        database: db ?? undefined,
                        schema: schema ?? undefined,
                      },
                      table: obj.name,
                      kind: obj.kind,
                    })
                  }
                  onDoubleClick={() => {
                    const scope = {
                      database: db ?? undefined,
                      schema: schema ?? undefined,
                    };
                    if (
                      obj.kind === "function" ||
                      obj.kind === "procedure" ||
                      obj.kind === "trigger" ||
                      obj.kind === "sequence"
                    ) {
                      void openDdlTab(session, scope, obj);
                      return;
                    }
                    store.openTable(session.id, scope, obj.name);
                  }}
                  onContextMenu={(e) => openMenu(e, objectMenu(session, db, schema, obj))}
                />
              );
            })}
          {isOpen && Array.isArray(objects) && shown.length === 0 && (
            <div
              className="px-1 py-[2px] text-[13px] text-slate-400"
              style={{ paddingLeft: 4 + (depth + 1) * 13 }}
            >
              No objects
            </div>
          )}
        </div>
      );
    });
    // Virtual Navicat-style folders: Events (MySQL/MariaDB scheduler),
    // Queries (shortcuts) and Backups (shortcuts). They need no backend data.
    const showEvents = session.engine === "mysql" || session.engine === "mariadb";
    const showShortcuts = sqlEngine;
    return (
      <>
        {realFolders}
        {showEvents && renderVirtualFolder(session, db, schema, "events", "Events", depth)}
        {showShortcuts && renderVirtualFolder(session, db, schema, "queries", "Queries", depth)}
        {showShortcuts && renderVirtualFolder(session, db, schema, "backups", "Backups", depth)}
      </>
    );
  };

  const renderDatabase = (session: SessionInfo, db: DatabaseRef, depth: number) => {
    const dk = `db:${dbKey(session.id, db.name)}`;
    const isOpen = expanded.has(dk);
    const schemas = cache.schemas[dbKey(session.id, db.name)];
    const hasSchemas = session.capabilities.list_schemas;
    return (
      <div key={dk}>
        <TreeRow
          depth={depth}
          caret
          expanded={isOpen}
          loading={hasSchemas && schemas === "loading"}
          icon={<Database size={12} />}
          label={
            <span className="font-medium">
              {db.name}
              {db.is_system && (
                <span className="ml-1 text-[11px] font-normal text-slate-400">
                  system
                </span>
              )}
            </span>
          }
          onClick={(e) => {
            if (e.detail === 2) void loadSchemas(session.id, db.name, session.engine);
            else toggle(dk);
          }}
          onContextMenu={(e) => openMenu(e, databaseMenu(session, db))}
        />
        {isOpen && hasSchemas && Array.isArray(schemas) &&
          schemas
            .filter((s) => filterMatch(s.name))
            .map((schema) => {
              const sk = `sc:${schemaKey(session.id, db.name, schema.name)}`;
              const schemaOpen = expanded.has(sk);
              return (
                <div key={sk}>
                  <TreeRow
                    depth={depth + 1}
                    caret
                    expanded={schemaOpen}
                    icon={<Folder size={12} />}
                    label={<span className="font-medium">{schema.name}</span>}
                    onClick={() => toggle(sk)}
                    onContextMenu={(e) =>
                      openMenu(e, schemaMenu(session, db.name, schema.name))
                    }
                  />
                  {schemaOpen &&
                    renderFolders(session, db.name, schema.name, depth + 2)}
                </div>
              );
            })}
        {isOpen && !hasSchemas && renderFolders(session, db.name, null, depth + 1)}
      </div>
    );
  };

  // ------------------------------------------------------- broker (Kafka) tree
  // Brokers have no databases: a connected session expands to a Topics
  // folder whose rows are live topics (`list_objects` kind=table, cached
  // under the `f:<sid>|||table` key so the declarative loader fills it).
  // Double-click (or View Messages) opens a query tab running the consume
  // command, so the result grid, full-value viewer and export work unchanged.

  const brokerTopicsKey = (sessionId: string) => `f:${sessionId}|||table`;

  const deleteTopic = (session: SessionInfo, topic: string) => {
    void (async () => {
      if (
        !(await confirmDialog(
          `Delete Kafka topic "${topic}"? Its messages will be lost. This cannot be undone.`,
          { title: "Delete topic", kind: "warning" },
        ))
      ) {
        return;
      }
      try {
        await api.kafkaDeleteTopic(session.id, topic);
        useStore.getState().toast("success", `Deleted topic "${topic}"`);
        refreshTreeEvent();
      } catch (err) {
        useStore.getState().toast("error", errorText(err));
      }
    })();
  };

  const brokerTopicsMenu = (session: SessionInfo): ContextMenuItem[] => [
    {
      kind: "item",
      label: "New Topic…",
      onClick: () =>
        useStore
          .getState()
          .openObjectDialog("new-topic", { sessionId: session.id }),
    },
    {
      kind: "item",
      label: "Refresh",
      onClick: () => void loadObjects(session.id, null, null, "table"),
    },
  ];

  const brokerTopicMenu = (
    session: SessionInfo,
    topic: ObjectRef,
  ): ContextMenuItem[] => [
    {
      kind: "item",
      label: "View Messages",
      onClick: () => openMessages(session.id, topic.name),
    },
    {
      kind: "item",
      label: "Produce Message…",
      onClick: () =>
        useStore.getState().openObjectDialog("produce-message", {
          sessionId: session.id,
          table: topic.name,
        }),
    },
    { kind: "separator" },
    { kind: "item", label: "Copy Name", onClick: () => copyName(topic.name) },
    { kind: "separator" },
    {
      kind: "item",
      label: "Delete Topic…",
      danger: true,
      onClick: () => deleteTopic(session, topic.name),
    },
  ];

  const brokerSessionMenu = (session: SessionInfo): ContextMenuItem[] => {
    const profile = store.profiles.find((p) => p.id === session.profile_id);
    const items: ContextMenuItem[] = [
      { kind: "item", label: "New Query", onClick: () => store.newQueryTab(session.id) },
      {
        kind: "item",
        label: "New Topic…",
        onClick: () =>
          useStore
            .getState()
            .openObjectDialog("new-topic", { sessionId: session.id }),
      },
      {
        kind: "item",
        label: "Produce Message…",
        onClick: () =>
          useStore
            .getState()
            .openObjectDialog("produce-message", { sessionId: session.id }),
      },
      { kind: "item", label: "Refresh", onClick: () => refreshSession(session) },
      { kind: "separator" },
      { kind: "item", label: "Copy Name", onClick: () => copyName(session.name) },
    ];
    if (profile) {
      items.push(
        { kind: "separator" },
        { kind: "label", label: "Connection" },
        {
          kind: "item",
          label: "Edit Connection…",
          onClick: () => store.setConnectionModal(true, profile.id),
        },
        {
          kind: "item",
          label: "Duplicate Connection",
          onClick: () => duplicateConnection(profile),
        },
        {
          kind: "item",
          label: "Copy Connection To…",
          onClick: () => copyConnectionTo(profile),
        },
        {
          kind: "item",
          label: "Move Connection To…",
          onClick: () => moveConnectionTo(profile),
        },
      );
    }
    items.push(
      { kind: "separator" },
      {
        kind: "item",
        label: "Disconnect",
        danger: true,
        onClick: () => void store.closeSession(session.id),
      },
    );
    return items;
  };

  const renderBrokerTopic = (session: SessionInfo, topic: ObjectRef) => (
    <TreeRow
      key={`kt:${session.id}|${topic.name}`}
      depth={2}
      icon={<MessageSquare size={11} className="text-amber-600" />}
      label={<span>{topic.name}</span>}
      subtitle={topic.comment ?? undefined}
      onClick={() => store.setActiveSession(session.id)}
      onDoubleClick={() => openMessages(session.id, topic.name)}
      onContextMenu={(e) => openMenu(e, brokerTopicMenu(session, topic))}
    />
  );

  const renderBrokerTopics = (session: SessionInfo) => {
    const key = brokerTopicsKey(session.id);
    const isOpen = expanded.has(key);
    const topics = cache.objects[folderKey(session.id, "", "", "table")];
    return (
      <div key={key}>
        <TreeRow
          depth={1}
          caret
          expanded={isOpen}
          loading={topics === "loading"}
          icon={<Inbox size={12} />}
          label={<span className="font-medium">Topics</span>}
          subtitle={Array.isArray(topics) ? `${topics.length}` : undefined}
          onClick={() => toggle(key)}
          onToggle={() => toggle(key)}
          onContextMenu={(e) => openMenu(e, brokerTopicsMenu(session))}
        />
        {isOpen &&
          Array.isArray(topics) &&
          topics
            .filter((t) => filterMatch(t.name))
            .map((t) => renderBrokerTopic(session, t))}
        {isOpen && topics === "loading" && (
          <div className="px-4 py-0.5 text-[12px] text-slate-500">
            Loading topics…
          </div>
        )}
        {isOpen && Array.isArray(topics) && topics.length === 0 && (
          <div
            className="px-1 py-[2px] text-[13px] text-slate-400"
            style={{ paddingLeft: 4 + 2 * 13 }}
          >
            No topics — right-click for New Topic…
          </div>
        )}
      </div>
    );
  };

  const renderSession = (session: SessionInfo) => {
    const sk = `s:${session.id}`;
    const isOpen = expanded.has(sk);
    const databases = cache.databases[session.id];
    return (
      <div key={session.id}>
        <TreeRow
          depth={0}
          caret
          expanded={isOpen}
          loading={databases === "loading"}
          icon={<EngineGlyph engine={session.engine} />}
          label={<span className="font-medium">{session.name}</span>}
          subtitle={
            store.profiles.find((p) => p.id === session.profile_id)?.group ?? undefined
          }
          selected={store.activeSessionId === session.id}
          trailing={
            <span className="flex items-center gap-0.5">
              {(() => {
                const c = store.profiles.find((p) => p.id === session.profile_id)?.color;
                return c ? (
                  <span
                    className="inline-block h-2 w-2 shrink-0 rounded-full"
                    style={{ backgroundColor: c }}
                    title={c}
                  />
                ) : null;
              })()}
              {starButton(session.profile_id)}
              <button
                className="hidden h-5 w-5 place-items-center rounded-md text-slate-400 group-hover:grid hover:bg-slate-900/[0.06] hover:text-slate-600"
                title="New query"
                onClick={(e) => {
                  e.stopPropagation();
                  store.newQueryTab(session.id);
                }}
              >
                <FileCode2 size={11} />
              </button>
              <button
                className="hidden h-5 w-5 place-items-center rounded-md text-slate-400 group-hover:grid hover:bg-slate-900/[0.06] hover:text-rose-600"
                title="Disconnect"
                onClick={(e) => {
                  e.stopPropagation();
                  void store.closeSession(session.id);
                }}
              >
                <X size={11} />
              </button>
            </span>
          }
          onClick={(e) => {
            store.setActiveSession(session.id);
            if (!isBroker(session.engine) && e.detail === 2) {
              void loadDatabases(session.id);
            } else toggle(sk);
          }}
          onContextMenu={(e) => openMenu(e, sessionMenu(session))}
        />
        {isOpen &&
          (isBroker(session.engine) ? (
            renderBrokerTopics(session)
          ) : Array.isArray(databases) ? (
            databases.filter((d) => filterMatch(d.name)).map((db) => renderDatabase(session, db, 1))
          ) : databases === "loading" ? (
            <div className="px-4 py-0.5 text-[12px] text-slate-500">
              Loading databases…
            </div>
          ) : null)}
      </div>
    );
  };

  const renderProfile = (profile: ConnectionProfile) => {
    const session = store.sessions.find((s) => s.profile_id === profile.id);
    return (
      <TreeRow
        key={`p-${profile.id}`}
        depth={0}
        caret={false}
        icon={<EngineGlyph engine={profile.engine} muted={!session} />}
        label={<span>{profile.name}</span>}
        subtitle={profile.group ?? undefined}
        selected={!!session && store.activeSessionId === session.id}
        trailing={
          <span className="flex items-center gap-0.5">
            {profile.color ? (
              <span
                className="inline-block h-2 w-2 shrink-0 rounded-full"
                style={{ backgroundColor: profile.color }}
                title={profile.color}
              />
            ) : null}
            {starButton(profile.id)}
            {!session && (
              <button
                className="hidden h-5 w-5 place-items-center rounded-md text-slate-400 group-hover:grid hover:bg-slate-900/[0.06] hover:text-slate-600"
                title="Connect"
                onClick={(e) => {
                  e.stopPropagation();
                  void store.openSession(profile.id).catch((err) => {
                    store.toast("error", errorText(err));
                  });
                }}
              >
                <Plug size={11} />
              </button>
            )}
          </span>
        }
        onClick={() => {
          if (session) {
            store.setActiveSession(session.id);
            return;
          }
          // A saved connection connects straight away on click; edit or
          // delete it from the right-click menu.
          void store.openSession(profile.id).catch((err) => {
            store.toast("error", errorText(err));
          });
        }}
        onContextMenu={(e) => openMenu(e, profileMenu(profile))}
      />
    );
  };

  const connectedProfileIds = new Set(
    store.sessions.map((s) => s.profile_id).filter(Boolean),
  );
  const disconnected = store.profiles.filter(
    (p) => !connectedProfileIds.has(p.id),
  );

  const dbSessions = store.sessions.filter((s) => !isBroker(s.engine));
  const brokerSessions = store.sessions.filter((s) => isBroker(s.engine));
  const dbProfiles = disconnected.filter((p) => !isBroker(p.engine));
  const brokerProfiles = disconnected.filter((p) => isBroker(p.engine));

  const favoriteProfiles = store.profiles.filter((p) => favorites.has(p.id));

  const hasConnections = store.sessions.length > 0 || disconnected.length > 0;

  const activeSession = store.sessions.find(
    (s) => s.id === store.activeSessionId,
  );
  const activeProfile = store.profiles.find(
    (p) => p.id === activeSession?.profile_id,
  );

  return (
    <aside className="flex w-[240px] shrink-0 flex-col border-r border-hairline bg-sidebar">
      {/* search */}
      <div className="flex shrink-0 items-center gap-1 px-2 pt-2 pb-1">
        <div className="flex h-7 flex-1 items-center rounded-md bg-slate-900/[0.05] px-2">
          <Search size={12} className="mr-1.5 shrink-0 text-slate-500" />
          <input
            className="w-full border-none bg-transparent p-0 text-[13px] text-slate-900 placeholder:text-slate-500 focus:outline-none"
            placeholder="Search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {query && (
            <button
              onClick={() => setQuery("")}
              className="text-slate-400 hover:text-slate-600"
            >
              <X size={11} />
            </button>
          )}
        </div>
        <button
          className="grid h-7 w-7 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700"
          title="Reload tree"
          onClick={() => setCache(EMPTY_CACHE)}
        >
          <RefreshCw size={13} strokeWidth={1.6} />
        </button>
      </div>

      {/* tree */}
      <div className="custom-scrollbar flex-1 overflow-y-auto px-2 pb-2 text-[13px] select-none">
        {!hasConnections && (
          <div className="flex flex-col items-center px-3 pt-10 text-center">
            <div className="grid h-10 w-10 place-items-center rounded-full border border-slate-300 text-slate-400">
              <Plug size={16} strokeWidth={1.6} />
            </div>
            <div className="mt-3 text-[13px] font-medium text-slate-700">
              No Connections
            </div>
            <p className="mt-1 text-[12px] leading-relaxed text-slate-500">
              Connect to a database to get started.
            </p>
            <button
              className="mt-3 flex items-center gap-1 rounded-md px-2.5 py-1 text-[13px] font-medium text-accent hover:bg-accent/10"
              onClick={() => store.setConnectionModal(true)}
            >
              <Plus size={13} strokeWidth={1.6} />
              New Connection
            </button>
          </div>
        )}

        {favoriteProfiles.length > 0 && (
          <>
            <SectionLabel>Favorites</SectionLabel>
            {favoriteProfiles.map(renderProfile)}
          </>
        )}

        {hasConnections && (
          <>
            <SectionLabel>Connections</SectionLabel>
            {(() => {
              const groups = new Map<string, { sessions: typeof dbSessions; profiles: typeof dbProfiles }>();
              const gkey = (g?: string | null) => (g ?? "").trim();
              for (const s of dbSessions) {
                const p = store.profiles.find((x) => x.id === s.profile_id);
                const k = gkey(p?.group);
                if (!groups.has(k)) groups.set(k, { sessions: [], profiles: [] });
                groups.get(k)!.sessions.push(s);
              }
              for (const p of dbProfiles) {
                const k = gkey(p.group);
                if (!groups.has(k)) groups.set(k, { sessions: [], profiles: [] });
                groups.get(k)!.profiles.push(p);
              }
              const ordered = [...groups.entries()].sort(([a], [b]) =>
                a === "" ? -1 : b === "" ? 1 : a.localeCompare(b),
              );
              return ordered.map(([group, members]) => (
                <div key={group || "__ungrouped"}>
                  {group !== "" && (
                    <div
                      onContextMenu={(e) => {
                        e.preventDefault();
                        e.stopPropagation();
                        setMenu({
                          x: e.clientX,
                          y: e.clientY,
                          items: groupHeaderMenu(group),
                        });
                      }}
                    >
                      <TreeRow
                        depth={0}
                        caret={false}
                        icon={<Folder size={12} className="text-slate-400" />}
                        label={
                          <span className="font-medium">
                            {group}
                            <span className="ml-1 text-[11px] font-normal text-slate-400">
                              ({members.sessions.length + members.profiles.length})
                            </span>
                          </span>
                        }
                      />
                    </div>
                  )}
                  <div className={group !== "" ? "pl-2" : undefined}>
                    {members.sessions.map(renderSession)}
                    {members.profiles.map(renderProfile)}
                  </div>
                </div>
              ));
            })()}
          </>
        )}

        {(brokerSessions.length > 0 || brokerProfiles.length > 0) && (
          <>
            <SectionLabel>Messaging</SectionLabel>
            {brokerSessions.map(renderSession)}
            {brokerProfiles.map(renderProfile)}
          </>
        )}
      </div>

      {/* settings */}
      <div className="shrink-0 border-t border-slate-300/60 px-2 py-1.5">
        <button
          className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-[13px] text-slate-600 hover:bg-slate-900/[0.04] hover:text-slate-800"
          onClick={() => store.toast("info", "Settings — coming soon")}
        >
          <Settings size={13} strokeWidth={1.6} />
          Settings
        </button>
      </div>

      {/* footer */}
      <div className="flex h-6 shrink-0 items-center gap-1.5 px-3 text-[11px] text-slate-500">
        <span
          className={`inline-block h-1.5 w-1.5 rounded-full ${
            activeSession ? "bg-emerald-500" : "bg-slate-300"
          }`}
        />
        <span className="truncate">
          {activeProfile ? activeProfile.name : "No connection"}
        </span>
      </div>

      {menu && (
        <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={closeMenu} />
      )}
    </aside>
  );
}
