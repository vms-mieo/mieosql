import { useCallback, useEffect, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Database,
  Eye,
  FileCode2,
  Folder,
  Layers,
  ListOrdered,
  Loader2,
  Plug,
  Plus,
  Radio,
  RefreshCw,
  Search,
  Settings,
  Star,
  Table2,
  Unplug,
  X,
} from "lucide-react";

import { api, errorText } from "../lib/api";
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
import { engineVersionBadge } from "../lib/format";
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

const ENGINE_LABEL: Record<Engine, string> = {
  mysql: "MySQL",
  mariadb: "MariaDB",
  postgres: "PostgreSQL",
  sqlite: "SQLite",
  sql_server: "SQL Server",
  mongodb: "MongoDB",
  redis: "Redis",
  kafka: "Kafka",
  oracle: "Oracle",
  snowflake: "Snowflake",
};

/** Message brokers get their own "Messaging" section in the tree. */
function isBroker(engine: Engine): boolean {
  return engine === "kafka";
}

function engineIcon(engine: Engine): React.ReactNode {
  return engine === "kafka" ? <Radio size={12} /> : <Plug size={12} />;
}

function objectIcon(kind: ObjectKind, size = 11): React.ReactNode {
  switch (kind) {
    case "view":
      return <Eye size={size} />;
    case "materialized_view":
      return <Layers size={size} />;
    case "sequence":
      return <ListOrdered size={size} />;
    default:
      return <Table2 size={size} />;
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
  loading,
  onClick,
  onToggle,
  onDoubleClick,
}: {
  depth: number;
  caret?: boolean;
  icon: React.ReactNode;
  label: React.ReactNode;
  subtitle?: React.ReactNode;
  trailing?: React.ReactNode;
  expanded?: boolean;
  selected?: boolean;
  loading?: boolean;
  onClick?: (e: React.MouseEvent) => void;
  onToggle?: () => void;
  onDoubleClick?: () => void;
}) {
  return (
    <div
      className={`group flex min-h-6 cursor-default items-center gap-1 rounded-md pr-1 ${
        subtitle ? "py-[3px]" : "py-px"
      } ${
        selected
          ? "bg-accent/[0.12] text-accent"
          : "text-slate-700 hover:bg-slate-900/[0.04]"
      }`}
      style={{ paddingLeft: 4 + depth * 13 }}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
    >
      <button
        className={`grid h-3.5 w-3.5 shrink-0 place-items-center rounded ${
          caret
            ? "text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
            : "invisible"
        }`}
        onClick={(e) => {
          e.stopPropagation();
          onToggle?.();
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
      <span className={`shrink-0 ${selected ? "text-accent" : "text-slate-500"}`}>
        {icon}
      </span>
      <span className="flex min-w-0 flex-1 flex-col leading-tight">
        <span className="truncate text-[13px]">{label}</span>
        {subtitle && (
          <span className="truncate text-[11px] text-slate-500">{subtitle}</span>
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
  // cache, load it (also covers "refresh tree").
  useEffect(() => {
    for (const key of expanded) {
      if (key.startsWith("s:")) {
        const sid = key.slice(2);
        if (!cache.databases[sid]) void loadDatabases(sid);
      } else if (key.startsWith("db:")) {
        const [, sid, db] = key.split("|");
        const session = store.sessions.find((s) => s.id === sid);
        if (!sid || !db || !session) continue;
        if (!cache.schemas[dbKey(sid, db)]) {
          void loadSchemas(sid, db, session.engine);
        }
      } else if (key.startsWith("f:")) {
        const [, sid, db, schema, kind] = key.split("|");
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

  const renderFolders = (
    session: SessionInfo,
    db: string | null,
    schema: string | null,
    depth: number,
  ) => {
    const folders: { kind: ObjectKind; label: string }[] = [
      { kind: "table", label: "Tables" },
      { kind: "view", label: "Views" },
      // PostgreSQL exposes these through the same introspection query today.
      ...(session.engine === "postgres"
        ? [
            { kind: "materialized_view" as ObjectKind, label: "Mat. Views" },
            { kind: "sequence" as ObjectKind, label: "Sequences" },
          ]
        : []),
    ];
    return folders.map((folder) => {
      const fk = folderKey(session.id, db ?? "", schema ?? "", folder.kind);
      const isOpen = expanded.has(fk);
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
              toggle(fk);
            }}
          />
          {isOpen &&
            shown.map((obj) => (
              <TreeRow
                key={`${fk}|${obj.name}`}
                depth={depth + 1}
                icon={objectIcon(obj.kind, 11)}
                selected={isSelected(session.id, db, schema, obj.name)}
                label={<span>{obj.name}</span>}
                trailing={
                  obj.rows_estimate !== undefined && obj.rows_estimate !== null ? (
                    <span className="text-[11px] text-slate-400">
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
                onDoubleClick={() =>
                  store.openTable(
                    session.id,
                    {
                      database: db ?? undefined,
                      schema: schema ?? undefined,
                    },
                    obj.name,
                  )
                }
              />
            ))}
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

  const renderSession = (session: SessionInfo) => {
    const sk = `s:${session.id}`;
    const isOpen = expanded.has(sk);
    const databases = cache.databases[session.id];
    const serverInfo = store.serverInfos[session.id];
    const badge = serverInfo
      ? engineVersionBadge(serverInfo.engine, serverInfo.version)
      : session.engine.toUpperCase();
    return (
      <div key={session.id}>
        <TreeRow
          depth={0}
          caret
          expanded={isOpen}
          loading={databases === "loading"}
          icon={engineIcon(session.engine)}
          label={<span className="font-medium">{session.name}</span>}
          subtitle={ENGINE_LABEL[session.engine]}
          selected={store.activeSessionId === session.id}
          trailing={
            <span className="flex items-center gap-0.5">
              <span className="rounded bg-slate-900/[0.06] px-1 text-[10.5px] font-medium text-slate-600">
                {badge}
              </span>
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
            if (e.detail === 2) void loadDatabases(session.id);
            else toggle(sk);
          }}
        />
        {isOpen &&
          (Array.isArray(databases) ? (
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
        icon={session ? engineIcon(profile.engine) : <Unplug size={11} />}
        label={<span>{profile.name}</span>}
        subtitle={ENGINE_LABEL[profile.engine]}
        selected={!!session && store.activeSessionId === session.id}
        trailing={
          <span className="flex items-center gap-0.5">
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
        onDoubleClick={() => {
          if (!session) {
            void store.openSession(profile.id).catch((err) => {
              store.toast("error", errorText(err));
            });
          }
        }}
        onClick={() => {
          if (session) store.setActiveSession(session.id);
          else store.setConnectionModal(true, profile.id);
        }}
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
            {dbSessions.map(renderSession)}
            {dbProfiles.map(renderProfile)}
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
    </aside>
  );
}
