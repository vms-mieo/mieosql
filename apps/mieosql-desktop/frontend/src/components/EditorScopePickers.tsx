// Connection / database / schema pickers for the query toolbar — pick the
// target right in the editor ("no connection" tabs can connect from here).
// The dropdowns are custom (not <select>) so long lists paginate.

import { useEffect, useRef, useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Database,
  Folder,
  Loader2,
  Plug,
} from "lucide-react";

import { api, errorText } from "../lib/api";
import type { DatabaseRef, SchemaRef } from "../lib/bindings";
import type { Tab } from "../lib/store";
import { useStore } from "../lib/store";
import { EngineGlyph } from "./EngineGlyph";

const PAGE_SIZE = 8;

type PickerItem = {
  id: string;
  label: string;
  sub?: string;
  icon?: React.ReactNode;
  current?: boolean;
  disabled?: boolean;
};

function PickerDropdown({
  label,
  icon,
  items,
  disabled,
  loading,
  emptyText,
  title,
  onSelect,
}: {
  label: string;
  icon?: React.ReactNode;
  items: PickerItem[];
  disabled?: boolean;
  loading?: boolean;
  emptyText?: string;
  title?: string;
  onSelect: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  useEffect(() => {
    if (open) setPage(0);
  }, [open, items.length]);

  const pages = Math.max(1, Math.ceil(items.length / PAGE_SIZE));
  const clamped = Math.min(page, pages - 1);
  const slice = items.slice(clamped * PAGE_SIZE, clamped * PAGE_SIZE + PAGE_SIZE);

  return (
    <div ref={rootRef} className="relative flex min-w-0">
      <button
        className={`flex h-7 min-w-0 max-w-[180px] items-center gap-1.5 rounded-md bg-slate-900/[0.04] px-2 text-[12px] transition-colors ${
          disabled
            ? "text-slate-400"
            : "text-slate-600 hover:bg-slate-900/[0.07]"
        }`}
        onClick={() => !disabled && setOpen((v) => !v)}
        disabled={disabled}
        title={title ?? label}
      >
        {loading ? (
          <Loader2 size={11} className="shrink-0 animate-spin text-slate-400" />
        ) : (
          icon
        )}
        <span className="min-w-0 flex-1 truncate text-left font-medium">{label}</span>
        <ChevronDown size={11} className="shrink-0 text-slate-400" />
      </button>

      {open && (
        <div className="absolute top-full right-0 z-50 mt-1 w-[248px] overflow-hidden rounded-md border border-slate-300/70 bg-white py-1 shadow-[0_10px_34px_rgba(0,0,0,0.16)]">
          <div className="min-h-[36px]">
            {slice.length === 0 && (
              <div className="px-3 py-2 text-[12px] text-slate-400">
                {emptyText ?? "—"}
              </div>
            )}
            {slice.map((it) => (
              <button
                key={it.id}
                disabled={it.disabled}
                className={`group flex w-full items-center gap-2 px-2.5 py-[5px] text-left text-[12.5px] ${
                  it.disabled
                    ? "text-slate-300"
                    : it.current
                      ? "font-medium text-accent"
                      : "text-slate-800 hover:bg-accent hover:text-white"
                }`}
                onClick={() => {
                  setOpen(false);
                  onSelect(it.id);
                }}
              >
                {it.icon}
                <span className="min-w-0 flex-1 truncate">{it.label}</span>
                {it.sub && (
                  <span className="shrink-0 text-[11px] text-slate-400 group-hover:text-white/75">
                    {it.sub}
                  </span>
                )}
                {it.current && <Check size={12} className="shrink-0" />}
              </button>
            ))}
          </div>
          {pages > 1 && (
            <div className="mx-1 mt-1 flex items-center justify-between border-t border-slate-200/70 pt-1">
              <button
                className="grid h-6 w-6 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.06] disabled:opacity-30"
                disabled={clamped === 0}
                onClick={() => setPage(clamped - 1)}
                title="Previous page"
              >
                <ChevronLeft size={13} />
              </button>
              <span className="text-[11px] text-slate-500">
                {clamped + 1} / {pages} · {items.length}
              </span>
              <button
                className="grid h-6 w-6 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.06] disabled:opacity-30"
                disabled={clamped >= pages - 1}
                onClick={() => setPage(clamped + 1)}
                title="Next page"
              >
                <ChevronRight size={13} />
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

const SQL_ENGINES = new Set(["mysql", "mariadb", "postgres", "sqlite", "sql_server"]);

export function EditorScopePickers({ tab }: { tab: Tab }) {
  const sessions = useStore((s) => s.sessions);
  const profiles = useStore((s) => s.profiles);
  const session = sessions.find((s) => s.id === tab.sessionId) ?? null;
  const engine = session?.engine ?? null;
  const scope = tab.scope;

  const [dbs, setDbs] = useState<Record<string, DatabaseRef[] | "loading">>({});
  const [schemas, setSchemas] = useState<Record<string, SchemaRef[] | "loading">>({});

  const sqlEngine = engine !== null && SQL_ENGINES.has(engine);

  // ------------------------------------------------------------- loaders
  const loadDbs = async (sid: string) => {
    setDbs((c) => ({ ...c, [sid]: "loading" }));
    try {
      const list = await api.listDatabases(sid);
      setDbs((c) => ({ ...c, [sid]: list }));
    } catch {
      setDbs((c) => ({ ...c, [sid]: [] }));
    }
  };

  const loadSchemas = async (sid: string, db: string | null) => {
    const key = `${sid}|${db ?? ""}`;
    setSchemas((c) => ({ ...c, [key]: "loading" }));
    try {
      const list = await api.listSchemas(sid, db);
      setSchemas((c) => ({ ...c, [key]: list }));
    } catch {
      setSchemas((c) => ({ ...c, [key]: [] }));
    }
  };

  useEffect(() => {
    if (!session || !sqlEngine) return;
    if (
      session.capabilities.list_databases &&
      engine !== "sqlite" &&
      !dbs[session.id]
    ) {
      void loadDbs(session.id);
    }
  }, [session?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!session?.capabilities.list_schemas) return;
    const key = `${session.id}|${session.database ?? ""}`;
    if (!schemas[key]) void loadSchemas(session.id, session.database ?? null);
  }, [session?.id, session?.database]); // eslint-disable-line react-hooks/exhaustive-deps

  // ------------------------------------------------------------- pickers
  const pickConnection = async (id: string) => {
    const st = useStore.getState();
    if (id.startsWith("p:")) {
      const profileId = id.slice(2);
      try {
        await st.openSession(profileId);
        const sess = useStore
          .getState()
          .sessions.find((s) => s.profile_id === profileId);
        if (sess) useStore.getState().setActiveSession(sess.id);
        useStore.getState().setTabScope(tab.id, {});
      } catch (err) {
        st.toast("error", errorText(err));
      }
      return;
    }
    st.setActiveSession(id);
    useStore.getState().setTabScope(tab.id, {});
  };

  const pickDatabase = async (db: string) => {
    if (!session) return;
    try {
      await api.useDatabase(session.id, db);
      await useStore.getState().refreshSessionInfo(session.id);
      // New database → schema context resets to default.
      useStore.getState().setTabScope(tab.id, { database: db });
    } catch (err) {
      useStore.getState().toast("error", errorText(err));
    }
  };

  const pickSchema = async (id: string) => {
    if (!session) return;
    const schema = id === "~default" ? undefined : id;
    useStore.getState().setTabScope(tab.id, {
      database: session.database ?? undefined,
      schema,
    });
    if (session.engine === "postgres") {
      try {
        const sql = schema
          ? `SET search_path TO "${schema.replace(/"/g, '""')}"`
          : "SET search_path TO DEFAULT";
        await api.executeBatch(session.id, [sql], false);
      } catch (err) {
        useStore.getState().toast("error", errorText(err));
      }
    }
  };

  // ------------------------------------------------------------- items
  const offline = profiles.filter(
    (p) => !sessions.some((s) => s.profile_id === p.id),
  );
  const connItems: PickerItem[] = [
    ...sessions.map((s) => ({
      id: s.id,
      label: s.name,
      sub: s.database ?? undefined,
      icon: <EngineGlyph engine={s.engine} size={13} />,
      current: session?.id === s.id,
    })),
    ...offline.map((p) => ({
      id: `p:${p.id}`,
      label: p.name,
      sub: "offline",
      icon: <EngineGlyph engine={p.engine} size={13} muted />,
    })),
  ];

  const dbState = session ? dbs[session.id] : undefined;
  const dbList = Array.isArray(dbState) ? dbState : [];
  const showDb =
    session !== null &&
    sqlEngine &&
    session.capabilities.list_databases &&
    engine !== "sqlite";

  const schemaState = session
    ? schemas[`${session.id}|${session.database ?? ""}`]
    : undefined;
  const schemaList = Array.isArray(schemaState) ? schemaState : [];
  const showSchema = session !== null && session.capabilities.list_schemas;

  return (
    <div className="flex min-w-0 items-center gap-1.5">
      <PickerDropdown
        label={session ? session.name : "No connection"}
        icon={
          session ? (
            <EngineGlyph engine={session.engine} size={13} />
          ) : (
            <Plug size={12} className="shrink-0 text-slate-400" />
          )
        }
        items={connItems}
        emptyText="No saved connections"
        title="Choose the connection for this query"
        onSelect={(id) => void pickConnection(id)}
      />

      {showDb && (
        <PickerDropdown
          label={session.database ?? "default database"}
          icon={
            <Database size={12} className="shrink-0 text-slate-400" />
          }
          loading={dbState === "loading"}
          items={dbList.map((db) => ({
            id: db.name,
            label: db.name,
            sub: db.is_system ? "system" : undefined,
            current: session.database === db.name,
          }))}
          emptyText="No databases"
          title="Choose the default database"
          onSelect={(id) => void pickDatabase(id)}
        />
      )}

      {showSchema && (
        <PickerDropdown
          label={scope?.schema ?? "default schema"}
          icon={<Folder size={12} className="shrink-0 text-slate-400" />}
          loading={schemaState === "loading"}
          items={[
            {
              id: "~default",
              label: "default",
              current: !scope?.schema,
            },
            ...schemaList.map((sc) => ({
              id: sc.name,
              label: sc.name,
              sub: sc.is_system ? "system" : undefined,
              current: scope?.schema === sc.name,
            })),
          ]}
          emptyText="No schemas"
          title="Choose the active schema"
          onSelect={(id) => void pickSchema(id)}
        />
      )}
    </div>
  );
}
