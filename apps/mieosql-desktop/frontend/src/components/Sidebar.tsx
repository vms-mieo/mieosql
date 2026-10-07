import { useCallback, useEffect, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Database,
  Eye,
  FileCode2,
  Folder,
  Loader2,
  Plug,
  Plus,
  RefreshCw,
  Search,
  Table2,
  Unplug,
  X,
} from "lucide-react";

import { api, errorText } from "../lib/api";
import type {
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

function dbKey(sid: string, db: string) {
  return `${sid}|${db}`;
}
function schemaKey(sid: string, db: string, schema: string) {
  return `${sid}|${db}|${schema}`;
}
function folderKey(sid: string, db: string, schema: string, kind: string) {
  return `${sid}|${db}|${schema}|${kind}`;
}

function TreeRow({
  depth,
  caret,
  icon,
  label,
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
      className={`group flex h-6 cursor-default items-center gap-1 rounded-md pr-1 text-[13px] ${
        selected
          ? "bg-accent/[0.12] text-slate-900"
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
      <span className="shrink-0">{icon}</span>
      <span className="flex-1 truncate">{label}</span>
      {trailing}
    </div>
  );
}

export function Sidebar() {
  const store = useStore();
  const [cache, setCache] = useState<Cache>(EMPTY_CACHE);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");

  const toggle = useCallback((key: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
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

  // Refresh tree event (from the ribbon / File menu).
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

  const renderFolders = (
    session: SessionInfo,
    db: string | null,
    schema: string | null,
    depth: number,
  ) => {
    const folders: { kind: ObjectKind; label: string; icon: React.ReactNode }[] = [
      {
        kind: "table",
        label: "Tables",
        icon: <Table2 size={12} className="text-slate-500" />,
      },
      {
        kind: "view",
        label: "Views",
        icon: <Eye size={12} className="text-slate-500" />,
      },
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
            icon={folder.icon}
            label={
              <span className="text-slate-700">
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
                icon={
                  folder.kind === "table" ? (
                    <Table2 size={11} className="text-slate-500" />
                  ) : (
                    <Eye size={11} className="text-slate-400" />
                  )
                }
                selected={isSelected(session.id, db, schema, obj.name)}
                label={<span className="text-slate-700">{obj.name}</span>}
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
              style={{ paddingLeft: 4 + (depth + 1) * 14 }}
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
          icon={<Database size={12} className="text-slate-500" />}
          label={
            <span className="font-medium text-slate-700">
              {db.name}
              {db.is_system && (
                <span className="ml-1 text-[13px] text-slate-400">system</span>
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
                    icon={<Folder size={12} className="text-slate-400" />}
                    label={
                      <span className="font-medium text-slate-600">
                        {schema.name}
                      </span>
                    }
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
          icon={<Plug size={12} className="text-slate-500" />}
          label={
            <span className="font-medium text-slate-900">
              {session.name}
            </span>
          }
          selected={store.activeSessionId === session.id}
          trailing={
            <span className="flex items-center gap-0.5">
              <span className="rounded bg-slate-900/[0.06] px-1 text-[10.5px] font-medium text-slate-600">
                {badge}
              </span>
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
            <div className="px-4 py-0.5 text-[13px] text-slate-400">
              Loading databases…
            </div>
          ) : null)}
      </div>
    );
  };

  const connectedProfileIds = new Set(
    store.sessions.map((s) => s.profile_id).filter(Boolean),
  );
  const disconnected = store.profiles.filter(
    (p) => !connectedProfileIds.has(p.id),
  );

  const activeSession = store.sessions.find(
    (s) => s.id === store.activeSessionId,
  );
  const activeProfile = store.profiles.find(
    (p) => p.id === activeSession?.profile_id,
  );

  return (
    <aside className="flex w-[240px] shrink-0 flex-col bg-sidebar">
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
          <RefreshCw size={13} strokeWidth={1.8} />
        </button>
      </div>

      {/* tree */}
      <div className="custom-scrollbar flex-1 overflow-y-auto px-2 pb-2 text-[13px] select-none">
        {(store.sessions.length > 0 || disconnected.length > 0) && (
          <div className="px-2 pt-2 pb-1 text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
            Connections
          </div>
        )}
        {store.sessions.length === 0 && disconnected.length === 0 && (
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
              <Plus size={13} strokeWidth={1.8} />
              New Connection
            </button>
          </div>
        )}
        {store.sessions.map(renderSession)}
        {disconnected.map((profile) => (
          <TreeRow
            key={profile.id}
            depth={0}
            icon={<Unplug size={11} className="text-slate-400" />}
            label={<span className="text-slate-600">{profile.name}</span>}
            onDoubleClick={() => void store.openSession(profile.id)}
            trailing={
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
            }
            onClick={() => store.setConnectionModal(true, profile.id)}
          />
        ))}
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
