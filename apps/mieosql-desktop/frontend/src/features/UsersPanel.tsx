// Users & Roles manager panel (feature tab "users"): Navicat-style user
// editor. Left: principals table. Right: General / Advanced / Members Of /
// Members / Server Privileges / Object Privileges / SQL Preview tabs.
// Mutations stage into pending ops (previewable SQL) and apply in order.

import { useEffect, useMemo, useState } from "react";

import type {
  DbPrincipal,
  DbPrincipalDetail,
  Engine,
} from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { quoteIdent } from "../lib/quoting";
import { useStore, type Tab } from "../lib/store";

// ---------------------------------------------------------------------------
// Small UI atoms (match the feature-dialog conventions)
// ---------------------------------------------------------------------------

const inputCls =
  "h-7 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-800 placeholder:text-slate-500 focus:ring-2 focus:ring-accent/40 focus:outline-none disabled:bg-slate-100 disabled:text-slate-400";
const labelCls = "mb-1 block text-[11px] font-medium text-slate-500";
const btnPrimary =
  "flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white transition-colors hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50";
const btnSecondary =
  "flex h-7 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 transition-colors hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50";
const btnDanger =
  "flex h-7 items-center gap-1.5 rounded-md border border-rose-300 bg-white px-2.5 text-[13px] text-rose-600 transition-colors hover:bg-rose-50 disabled:cursor-not-allowed disabled:opacity-50";

const qstr = (v: string) => `'${v.replace(/'/g, "''")}'`;

// ---------------------------------------------------------------------------
// Privilege vocabularies per engine
// ---------------------------------------------------------------------------

const MYSQL_GLOBAL = [
  "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "REFERENCES",
  "INDEX", "ALTER", "SHOW DATABASES", "SUPER", "CREATE TEMPORARY TABLES",
  "LOCK TABLES", "EXECUTE", "REPLICATION SLAVE", "REPLICATION CLIENT",
  "CREATE VIEW", "SHOW VIEW", "CREATE ROUTINE", "ALTER ROUTINE", "EVENT",
  "TRIGGER", "CREATE USER",
];
const MYSQL_OBJ = [
  "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "REFERENCES",
  "INDEX", "ALTER", "CREATE VIEW", "SHOW VIEW", "TRIGGER", "EXECUTE",
];
const PG_OBJ = ["SELECT", "INSERT", "UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"];
const PG_ATTRS: { key: string; sql: string; noSql: string }[] = [
  { key: "LOGIN", sql: "LOGIN", noSql: "NOLOGIN" },
  { key: "SUPERUSER", sql: "SUPERUSER", noSql: "NOSUPERUSER" },
  { key: "CREATEDB", sql: "CREATEDB", noSql: "NOCREATEDB" },
  { key: "CREATEROLE", sql: "CREATEROLE", noSql: "NOCREATEROLE" },
  { key: "REPLICATION", sql: "REPLICATION", noSql: "NOREPLICATION" },
];
const MSSQL_OBJ = [
  "SELECT", "INSERT", "UPDATE", "DELETE", "EXECUTE", "REFERENCES",
  "VIEW DEFINITION", "ALTER", "CONTROL",
];
const MSSQL_DB = ["CONNECT", "CREATE TABLE", "CREATE VIEW", "CREATE PROCEDURE", "ALTER", "CONTROL"];

// ---------------------------------------------------------------------------
// Pending ops (staged mutations with SQL preview)
// ---------------------------------------------------------------------------

type PendingOp = {
  id: number;
  label: string;
  preview: string;
  run: () => Promise<void>;
};

let opSeq = 1;

// ---------------------------------------------------------------------------
// SQL builders (identifiers quoted per engine; literals via qstr)
// ---------------------------------------------------------------------------

function mysqlAccount(engine: Engine, name: string, host?: string | null): string {
  void engine;
  return `${quoteIdent("mysql", name)}@${quoteIdent("mysql", host?.trim() || "%")}`;
}


function pgTable(engine: Engine, schema: string | null | undefined, table: string): string {
  void engine;
  const sc = schema?.trim() || "public";
  return `${quoteIdent("postgres", sc)}.${quoteIdent("postgres", table)}`;
}

// ---------------------------------------------------------------------------
// Main panel
// ---------------------------------------------------------------------------

type DetailTab =
  | "general"
  | "advanced"
  | "memberof"
  | "members"
  | "server"
  | "objects"
  | "sql";

const DETAIL_TABS: { id: DetailTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "advanced", label: "Advanced" },
  { id: "memberof", label: "Members Of" },
  { id: "members", label: "Members" },
  { id: "server", label: "Server Privileges" },
  { id: "objects", label: "Object Privileges" },
  { id: "sql", label: "SQL Preview" },
];

export function UsersPanel({ tab }: { tab: Tab }) {
  const store = useStore();
  const toast = useStore((s) => s.toast);
  const props = (tab.featureProps ?? {}) as { sessionId?: string };
  const [sessionId, setSessionId] = useState(
    props.sessionId ?? store.activeSessionId ?? store.sessions[0]?.id ?? "",
  );
  const session = store.sessions.find((s) => s.id === sessionId) ?? null;
  const engine: Engine | null = session?.engine ?? null;
  const supported = !!session?.capabilities.user_management;

  const [users, setUsers] = useState<DbPrincipal[]>([]);
  const [roles, setRoles] = useState<DbPrincipal[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [kindFilter, setKindFilter] = useState<"all" | "user" | "role">("all");
  const [search, setSearch] = useState("");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [detail, setDetail] = useState<DbPrincipalDetail | null>(null);
  const [detailBusy, setDetailBusy] = useState(false);
  const [detailTab, setDetailTab] = useState<DetailTab>("general");
  const [pending, setPending] = useState<PendingOp[]>([]);
  const [applying, setApplying] = useState(false);
  const [showNew, setShowNew] = useState(false);

  const keyOf = (p: DbPrincipal) => `${p.name}\u0000${p.host ?? ""}`;
  const all = useMemo(() => [...users, ...roles], [users, roles]);
  const selected = useMemo(
    () => all.find((p) => keyOf(p) === selectedKey) ?? null,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [all, selectedKey],
  );
  const shown = useMemo(() => {
    const q = search.trim().toLowerCase();
    return all.filter(
      (p) =>
        (kindFilter === "all" || p.kind === kindFilter) &&
        (!q ||
          p.name.toLowerCase().includes(q) ||
          (p.host ?? "").toLowerCase().includes(q)),
    );
  }, [all, kindFilter, search]);

  const pushOp = (label: string, preview: string, run: () => Promise<void>) => {
    setPending((prev) => [...prev, { id: opSeq++, label, preview, run }]);
    setDetailTab("sql");
    toast("info", `Staged: ${label} — review in SQL Preview`);
  };

  const load = async (keepSelection = true) => {
    if (!sessionId) return;
    setBusy(true);
    setError(null);
    try {
      const [u, r] = await Promise.all([
        api.listDbUsers(sessionId),
        api.listDbRoles(sessionId),
      ]);
      setUsers(u);
      setRoles(r);
      if (!keepSelection) setSelectedKey(null);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    setSelectedKey(null);
    setDetail(null);
    setPending([]);
    void load(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  useEffect(() => {
    if (!selected || !sessionId) {
      setDetail(null);
      return;
    }
    setDetailBusy(true);
    api
      .dbPrincipalDetail(sessionId, selected.name, selected.host ?? null)
      .then(setDetail)
      .catch((err) => {
        setDetail(null);
        toast("error", errorText(err));
      })
      .finally(() => setDetailBusy(false));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedKey, sessionId]);

  const applyPending = async () => {
    if (!sessionId || pending.length === 0 || applying) return;
    setApplying(true);
    let ok = 0;
    try {
      for (const op of pending) {
        try {
          await op.run();
          ok++;
        } catch (err) {
          toast("error", `Failed: ${op.label} — ${errorText(err)}`);
          break;
        }
      }
      if (ok === pending.length) toast("success", `Applied ${ok} change(s)`);
      else if (ok > 0) toast("info", `Applied ${ok}/${pending.length} — stopped at first error`);
      setPending((prev) => prev.slice(ok));
      await load();
      if (selected) {
        try {
          setDetail(await api.dbPrincipalDetail(sessionId, selected.name, selected.host ?? null));
        } catch {
          /* selection may be gone after drop */
        }
      }
    } finally {
      setApplying(false);
    }
  };

  // Run raw SQL on SQL engines through the batch path.
  const runSql = (sql: string) => async () => {
    if (!sessionId) throw new Error("no session");
    const res = await api.executeBatch(sessionId, [sql], false);
    if (res.failed > 0) throw new Error(res.errors[0] ?? "statement failed");
  };

  if (!session) {
    return (
      <div className="grid flex-1 place-items-center bg-surface text-[13px] text-slate-500">
        Open a connection first, then manage its users from the Users tab.
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-surface">
      {/* toolbar */}
      <div className="flex h-10 shrink-0 flex-wrap items-center gap-2 border-b border-slate-200/80 bg-white px-3">
        <select
          className="h-7 rounded-md border border-slate-300 bg-white px-1.5 text-[12.5px] text-slate-800 focus:outline-none"
          value={sessionId}
          onChange={(e) => setSessionId(e.target.value)}
        >
          {store.sessions.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name} ({s.engine})
            </option>
          ))}
        </select>
        <div className="flex overflow-hidden rounded-md border border-slate-300">
          {(["all", "user", "role"] as const).map((k) => (
            <button
              key={k}
              className={`h-7 px-2.5 text-[12px] capitalize ${kindFilter === k ? "bg-accent font-medium text-white" : "bg-white text-slate-600 hover:bg-slate-50"}`}
              onClick={() => setKindFilter(k)}
            >
              {k === "all" ? "All" : `${k}s`}
            </button>
          ))}
        </div>
        <input
          className="h-7 w-44 rounded-md border border-slate-300 bg-white px-2 text-[12.5px] placeholder:text-slate-400 focus:outline-none"
          placeholder="Search users…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <button className={btnSecondary} disabled={busy} onClick={() => void load()}>
          {busy ? "Loading…" : "Refresh"}
        </button>
        {supported && (
          <button className={btnSecondary} onClick={() => setShowNew((v) => !v)}>
            {showNew ? "Hide" : "+ New User"}
          </button>
        )}
        <div className="ml-auto flex items-center gap-2">
          {pending.length > 0 && (
            <span className="text-[12px] font-medium text-amber-600">
              {pending.length} staged
            </span>
          )}
          <button className={btnSecondary} disabled={pending.length === 0} onClick={() => setPending([])}>
            Discard
          </button>
          <button className={btnPrimary} disabled={pending.length === 0 || applying} onClick={() => void applyPending()}>
            {applying ? "Applying…" : `Apply${pending.length > 0 ? ` (${pending.length})` : ""}`}
          </button>
        </div>
      </div>

      {!supported ? (
        <div className="grid flex-1 place-items-center p-6 text-center text-[13px] text-slate-500">
          User management is not available for {session.engine} connections.
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          {/* principals table */}
          <div className="custom-scrollbar w-[38%] min-w-[280px] shrink-0 overflow-auto border-r border-slate-200/70">
            {error && <div className="p-3 text-[12.5px] text-rose-600">{error}</div>}
            {showNew && engine && (
              <NewUserForm
                engine={engine}
                sessionId={sessionId}
                onStage={(label, preview, run) => {
                  pushOp(label, preview, run);
                  setShowNew(false);
                }}
              />
            )}
            <table className="w-full border-collapse text-left text-[12.5px]">
              <thead className="sticky top-0 bg-sidebar">
                <tr className="text-slate-500">
                  <th className="px-3 py-1.5 text-[11.5px] font-semibold">Name</th>
                  <th className="px-3 py-1.5 text-[11.5px] font-semibold">Host/DB</th>
                  <th className="px-3 py-1.5 text-[11.5px] font-semibold">Type</th>
                  <th className="px-3 py-1.5 text-[11.5px] font-semibold">Flags</th>
                </tr>
              </thead>
              <tbody>
                {shown.map((p) => {
                  const sel = keyOf(p) === selectedKey;
                  return (
                    <tr
                      key={keyOf(p)}
                      className={`cursor-default border-b border-slate-200/70 ${sel ? "bg-accent text-white" : "hover:bg-slate-900/[0.03]"}`}
                      onClick={() => {
                        setSelectedKey(keyOf(p));
                        setDetailTab("general");
                      }}
                    >
                      <td className="px-3 py-1.5 font-medium">{p.name}</td>
                      <td className={`px-3 py-1.5 font-code text-[12px] ${sel ? "text-white/85" : "text-slate-500"}`}>
                        {p.host ?? "—"}
                      </td>
                      <td className="px-3 py-1.5">
                        <span className={`rounded px-1 text-[10.5px] font-semibold ${p.kind === "role" ? "bg-violet-100 text-violet-700" : "bg-sky-100 text-sky-700"}`}>
                          {p.kind}
                        </span>
                      </td>
                      <td className={`px-3 py-1.5 text-[11.5px] ${sel ? "text-white/85" : "text-slate-500"}`}>
                        {[p.is_super && "SUPER", p.can_login ? null : "NOLOGIN", p.comment].filter(Boolean).join(" · ")}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            {shown.length === 0 && !busy && (
              <div className="p-4 text-[12.5px] text-slate-400">No users or roles found.</div>
            )}
          </div>

          {/* detail */}
          <div className="flex min-w-0 flex-1 flex-col">
            {!selected ? (
              <div className="grid flex-1 place-items-center text-[13px] text-slate-400">
                Select a user or role to inspect and grant privileges.
              </div>
            ) : (
              <>
                <div className="flex h-9 shrink-0 items-center gap-0.5 overflow-x-auto border-b border-slate-200/80 bg-sidebar px-2">
                  {DETAIL_TABS.map((t) => (
                    <button
                      key={t.id}
                      className={`relative flex h-full items-center px-3 text-[12.5px] ${detailTab === t.id ? "font-medium text-slate-900" : "text-slate-500 hover:text-slate-700"}`}
                      onClick={() => setDetailTab(t.id)}
                    >
                      {detailTab === t.id && (
                        <span className="absolute inset-x-2 bottom-0 h-[2px] rounded-full bg-accent" />
                      )}
                      {t.label}
                      {t.id === "sql" && pending.length > 0 && (
                        <span className="ml-1 rounded-full bg-amber-100 px-1 text-[10.5px] font-semibold text-amber-700">
                          {pending.length}
                        </span>
                      )}
                    </button>
                  ))}
                </div>
                <div className="custom-scrollbar min-h-0 flex-1 overflow-auto p-4">
                  {detailBusy && <div className="text-[12.5px] text-slate-500">Loading…</div>}
                  {!detailBusy && detail && engine && (
                    <DetailTabs
                      engine={engine}
                      sessionId={sessionId}
                      detail={detail}
                      tab={detailTab}
                      pushOp={pushOp}
                      runSql={runSql}
                      sessionDb={session.database ?? null}
                      allRoles={roles}
                      allUsers={users}
                      pending={pending}
                      onApply={() => void applyPending()}
                      onDiscard={() => setPending([])}
                    />
                  )}
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// New user form
// ---------------------------------------------------------------------------

function NewUserForm({
  engine,
  sessionId,
  onStage,
}: {
  engine: Engine;
  sessionId: string;
  onStage: (label: string, preview: string, run: () => Promise<void>) => void;
}) {
  const [name, setName] = useState("");
  const [host, setHost] = useState("%");
  const [database, setDatabase] = useState("");
  const [password, setPassword] = useState("");
  const [login, setLogin] = useState(true);
  const [superuser, setSuperuser] = useState(false);
  const isMongo = engine === "mongodb";

  const canRun = name.trim().length > 0;
  return (
    <div className="space-y-2 border-b border-slate-200/70 bg-sidebar p-3">
      <div className="text-[12px] font-semibold text-slate-800">New User</div>
      <div>
        <label className={labelCls}>Name *</label>
        <input className={inputCls} value={name} placeholder="username" onChange={(e) => setName(e.target.value)} />
      </div>
      {(engine === "mysql" || engine === "mariadb") && (
        <div>
          <label className={labelCls}>Host</label>
          <input className={inputCls} value={host} placeholder="%" onChange={(e) => setHost(e.target.value)} />
        </div>
      )}
      {isMongo && (
        <div>
          <label className={labelCls}>Auth database (optional)</label>
          <input className={inputCls} value={database} placeholder="session database" onChange={(e) => setDatabase(e.target.value)} />
        </div>
      )}
      <div>
        <label className={labelCls}>Password</label>
        <input className={inputCls} type="password" value={password} placeholder="optional" onChange={(e) => setPassword(e.target.value)} />
      </div>
      <label className="flex items-center gap-1.5 text-[12.5px] text-slate-700">
        <input type="checkbox" className="accent-accent" checked={login} onChange={(e) => setLogin(e.target.checked)} />
        Can log in
      </label>
      <label className="flex items-center gap-1.5 text-[12.5px] text-slate-700">
        <input type="checkbox" className="accent-accent" checked={superuser} onChange={(e) => setSuperuser(e.target.checked)} />
        Superuser
      </label>
      <NewUserButton
        engine={engine}
        sessionId={sessionId}
        name={name}
        host={host}
        database={database}
        password={password}
        login={login}
        superuser={superuser}
        canRun={canRun}
        onStage={onStage}
      />
    </div>
  );
}

function NewUserButton(props: {
  engine: Engine;
  sessionId: string;
  name: string;
  host: string;
  database: string;
  password: string;
  login: boolean;
  superuser: boolean;
  canRun: boolean;
  onStage: (label: string, preview: string, run: () => Promise<void>) => void;
}) {
  const { engine, sessionId, name, host, database, password, login, superuser, canRun, onStage } = props;
  const stage = () => {
    const n = name.trim();
    if (!n) return;
    const pw = password || undefined;
    if (engine === "mysql" || engine === "mariadb") {
      const h = host.trim() || "%";
      const account = `${quoteIdent(engine, n)}@${quoteIdent(engine, h)}`;
      const sql = `CREATE USER ${account}${pw ? ` IDENTIFIED BY ${qstr(pw)}` : ""};`;
      onStage(`Create user ${n}@${h}`, sql, () => api.createDbUser(sessionId, {
        name: n, host: h, password: pw, superuser: false, login: true,
      }));
    } else if (engine === "postgres") {
      const opts = [`${login ? "LOGIN" : "NOLOGIN"}`, superuser ? "SUPERUSER" : null, pw ? `PASSWORD ${qstr(pw)}` : null]
        .filter(Boolean)
        .join(" ");
      const sql = `CREATE ROLE ${quoteIdent(engine, n)} WITH ${opts};`;
      onStage(`Create role ${n}`, sql, () => api.createDbUser(sessionId, {
        name: n, password: pw, superuser, login,
      }));
    } else if (engine === "sql_server") {
      const q = quoteIdent(engine, n);
      const stmts = pw
        ? [`CREATE LOGIN ${q} WITH PASSWORD = ${qstr(pw)};`, `CREATE USER ${q} FOR LOGIN ${q};`]
        : [`CREATE USER ${q} WITHOUT LOGIN;`];
      if (superuser && pw) stmts.push(`ALTER SERVER ROLE [sysadmin] ADD MEMBER ${q};`);
      onStage(`Create user ${n}`, stmts.join("\n"), () => api.createDbUser(sessionId, {
        name: n, password: pw, superuser, login,
      }));
    } else if (engine === "mongodb") {
      const db = database.trim() || undefined;
      const preview = `db.getSiblingDB(${qstr(db ?? "<session db>")}).createUser({ user: ${qstr(n)}, pwd: "•••", roles: [] })`;
      onStage(`Create user ${n}`, preview, () => api.createDbUser(sessionId, {
        name: n, database: db, password: pw, superuser, login,
      }));
    } else if (engine === "redis") {
      const preview = `ACL SETUSER ${n} on ${pw ? ">" + "•".repeat(Math.min(pw.length, 8)) : "nopass"} -@all`;
      onStage(`Create user ${n}`, preview, () => api.createDbUser(sessionId, {
        name: n, password: pw, superuser: false, login,
      }));
    }
  };
  return (
    <button className={btnPrimary} disabled={!canRun} onClick={stage}>
      Stage Create
    </button>
  );
}

// ---------------------------------------------------------------------------
// Detail tabs
// ---------------------------------------------------------------------------

function PrivGrid({
  options,
  checked,
  onToggle,
}: {
  options: string[];
  checked: Set<string>;
  onToggle: (name: string, next: boolean) => void;
}) {
  return (
    <div className="grid grid-cols-2 gap-x-6 gap-y-1.5 md:grid-cols-3">
      {options.map((p) => (
        <label key={p} className="flex cursor-pointer items-center gap-1.5 text-[12.5px] text-slate-700">
          <input
            type="checkbox"
            className="accent-accent"
            checked={checked.has(p)}
            onChange={(e) => onToggle(p, e.target.checked)}
          />
          <span className="font-code text-[12px]">{p}</span>
        </label>
      ))}
    </div>
  );
}

type DetailProps = {
  engine: Engine;
  sessionId: string;
  detail: DbPrincipalDetail;
  tab: DetailTab;
  pushOp: (label: string, preview: string, run: () => Promise<void>) => void;
  runSql: (sql: string) => () => Promise<void>;
  sessionDb: string | null;
  allRoles: DbPrincipal[];
  allUsers: DbPrincipal[];
  pending: PendingOp[];
  onApply: () => void;
  onDiscard: () => void;
};

function DetailTabs(props: DetailProps) {
  const { engine, detail, tab } = props;
  const isRole = detail.principal.kind === "role";
  if (tab === "general") return <GeneralTab {...props} />;
  if (tab === "advanced") return <AdvancedTab detail={detail} />;
  if (tab === "memberof") return <MembersOfTab {...props} />;
  if (tab === "members") {
    if (!isRole) return <Note text="Only roles have members." />;
    if (engine === "redis") return <Note text="Redis has no roles." />;
    return <MembersTab {...props} />;
  }
  if (tab === "server") {
    if (engine === "mongodb" || engine === "redis") return <Note text="No server-level privileges on this engine — use Roles." />;
    return <ServerTab {...props} />;
  }
  if (tab === "objects") {
    if (engine === "redis") return <Note text="Redis permissions are command/key selectors — see Server Privileges." />;
    return <ObjectsTab {...props} />;
  }
  return <SqlTab pending={props.pending} onApply={props.onApply} onDiscard={props.onDiscard} />;
}

function Note({ text }: { text: string }) {
  return <div className="text-[12.5px] text-slate-500">{text}</div>;
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="mb-5">
      <div className="mb-2 text-[12px] font-semibold text-slate-800">{title}</div>
      {children}
    </div>
  );
}

// --- General ---
function GeneralTab({ engine, sessionId, detail, pushOp, runSql }: DetailProps) {
  const p = detail.principal;
  const [pw, setPw] = useState("");
  const [pw2, setPw2] = useState("");
  const canLoginToggle = engine === "postgres" || engine === "sql_server" || engine === "redis";
  const canSuperToggle = engine === "postgres" || engine === "mysql" || engine === "mariadb" || engine === "sql_server";

  const stagePassword = () => {
    if (!pw) return;
    if (pw !== pw2) {
      useStore.getState().toast("error", "Passwords do not match");
      return;
    }
    const label = `Set password for ${p.name}`;
    if (engine === "mongodb" || engine === "redis") {
      const preview = engine === "mongodb"
        ? `db.updateUser(${qstr(p.name)}, { pwd: "•••" })`
        : `ACL SETUSER ${p.name} resetpass >•••`;
      pushOp(label, preview, () => api.setUserPassword(sessionId, p.name, p.host ?? null, pw));
    } else if (engine === "mysql" || engine === "mariadb") {
      const sql = `ALTER USER ${mysqlAccount(engine, p.name, p.host)} IDENTIFIED BY ${qstr(pw)};`;
      pushOp(label, sql, runSql(sql));
    } else if (engine === "postgres") {
      const sql = `ALTER ROLE ${quoteIdent(engine, p.name)} WITH PASSWORD ${qstr(pw)};`;
      pushOp(label, sql, runSql(sql));
    } else if (engine === "sql_server") {
      const sql = `ALTER LOGIN ${quoteIdent(engine, p.name)} WITH PASSWORD = ${qstr(pw)};`;
      pushOp(label, sql, runSql(sql));
    }
    setPw("");
    setPw2("");
  };

  const stageDrop = () => {
    const label = `Drop ${p.kind} ${p.name}`;
    if (engine === "mongodb" || engine === "redis") {
      const preview = engine === "mongodb" ? `db.dropUser(${qstr(p.name)})` : `ACL DELUSER ${p.name}`;
      pushOp(label, preview, () => api.dropDbPrincipal(sessionId, { name: p.name, host: p.host, kind: p.kind }));
      return;
    }
    const sql = dropPrincipalSql(engine, p.name, p.host, p.kind);
    if (sql) pushOp(label, sql, runSql(sql));
  };

  const toggleLogin = (next: boolean) => {
    if (engine === "postgres") {
      const sql = `ALTER ROLE ${quoteIdent(engine, p.name)} WITH ${next ? "LOGIN" : "NOLOGIN"};`;
      pushOp(`${next ? "Enable" : "Disable"} login for ${p.name}`, sql, runSql(sql));
    } else if (engine === "sql_server") {
      const sql = `ALTER LOGIN ${quoteIdent(engine, p.name)} ${next ? "ENABLE" : "DISABLE"};`;
      pushOp(`${next ? "Enable" : "Disable"} login for ${p.name}`, sql, runSql(sql));
    } else if (engine === "redis") {
      const token = next ? "on" : "off";
      pushOp(
        `${next ? "Enable" : "Disable"} user ${p.name}`,
        `ACL SETUSER ${p.name} ${token}`,
        () => api.grant(sessionId, { grantee: p.name, target: { privileges: { privileges: token } }, with_grant_option: false }),
      );
    }
  };

  const toggleSuper = (next: boolean) => {
    if (engine === "postgres") {
      const sql = `ALTER ROLE ${quoteIdent(engine, p.name)} WITH ${next ? "SUPERUSER" : "NOSUPERUSER"};`;
      pushOp(`${next ? "Grant" : "Revoke"} superuser for ${p.name}`, sql, runSql(sql));
    } else if (engine === "mysql" || engine === "mariadb") {
      const account = mysqlAccount(engine, p.name, p.host);
      const sql = next
        ? `GRANT ALL PRIVILEGES ON *.* TO ${account} WITH GRANT OPTION;`
        : `REVOKE ALL PRIVILEGES, GRANT OPTION FROM ${account};`;
      pushOp(`${next ? "Grant" : "Revoke"} superuser for ${p.name}`, sql, runSql(sql));
    } else if (engine === "sql_server") {
      const q = quoteIdent(engine, p.name);
      const sql = `ALTER SERVER ROLE [sysadmin] ${next ? "ADD" : "DROP"} MEMBER ${q};`;
      pushOp(`${next ? "Grant" : "Revoke"} sysadmin for ${p.name}`, sql, runSql(sql));
    }
  };

  return (
    <div>
      <Section title="Identity">
        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className={labelCls}>Name</label>
            <input className={inputCls} value={p.name} disabled readOnly />
          </div>
          <div>
            <label className={labelCls}>{engine === "mongodb" ? "Auth database" : "Host"}</label>
            <input className={inputCls} value={p.host ?? "—"} disabled readOnly />
          </div>
        </div>
        <div className="mt-2 flex flex-wrap gap-x-6 gap-y-2">
          <span className="text-[12.5px] text-slate-600">
            Type: <b>{p.kind}</b>
            {p.is_super && <span className="ml-2 rounded bg-rose-100 px-1 text-[10.5px] font-semibold text-rose-700">SUPER</span>}
            {!p.can_login && <span className="ml-2 rounded bg-slate-200 px-1 text-[10.5px] font-semibold text-slate-600">NOLOGIN</span>}
          </span>
          {p.comment && <span className="text-[12.5px] text-slate-500">{p.comment}</span>}
        </div>
      </Section>
      {(canLoginToggle || canSuperToggle) && (
        <Section title="Access">
          <div className="flex flex-wrap gap-x-6 gap-y-2">
            {canLoginToggle && (
              <label className="flex cursor-pointer items-center gap-1.5 text-[12.5px] text-slate-700">
                <input type="checkbox" className="accent-accent" checked={p.can_login} onChange={(e) => toggleLogin(e.target.checked)} />
                Can log in
              </label>
            )}
            {canSuperToggle && (
              <label className="flex cursor-pointer items-center gap-1.5 text-[12.5px] text-slate-700">
                <input type="checkbox" className="accent-accent" checked={p.is_super} onChange={(e) => toggleSuper(e.target.checked)} />
                Superuser
              </label>
            )}
          </div>
        </Section>
      )}
      <Section title="Change password">
        <div className="flex max-w-[420px] gap-2">
          <input className={inputCls} type="password" placeholder="New password" value={pw} onChange={(e) => setPw(e.target.value)} />
          <input className={inputCls} type="password" placeholder="Confirm" value={pw2} onChange={(e) => setPw2(e.target.value)} />
          <button className={btnSecondary} disabled={!pw} onClick={stagePassword}>Stage</button>
        </div>
      </Section>
      <Section title="Danger zone">
        <button className={btnDanger} onClick={stageDrop}>
          Stage Drop {p.kind} “{p.name}”
        </button>
      </Section>
    </div>
  );
}

function dropPrincipalSql(engine: Engine, name: string, host: string | null | undefined, kind: string): string | null {
  if (engine === "mysql" || engine === "mariadb") {
    const account = mysqlAccount(engine, name, host);
    return kind === "role" ? `DROP ROLE ${account};` : `DROP USER ${account};`;
  }
  if (engine === "postgres") return `DROP ROLE ${quoteIdent(engine, name)};`;
  if (engine === "sql_server") return `DROP USER ${quoteIdent(engine, name)};`;
  return null;
}

// --- Advanced (read-only attributes) ---
function AdvancedTab({ detail }: { detail: DbPrincipalDetail }) {
  if (detail.attributes.length === 0) {
    return <Note text="No advanced attributes exposed for this principal." />;
  }
  return (
    <table className="w-full max-w-[560px] border-collapse text-left text-[12.5px]">
      <thead>
        <tr className="text-slate-500">
          <th className="border-b border-slate-200 px-3 py-1.5 text-[11.5px] font-semibold">Attribute</th>
          <th className="border-b border-slate-200 px-3 py-1.5 text-[11.5px] font-semibold">Value</th>
        </tr>
      </thead>
      <tbody>
        {detail.attributes.map((a) => (
          <tr key={a.key} className="border-b border-slate-200/70">
            <td className="px-3 py-1.5 font-medium text-slate-700">{a.key}</td>
            <td className="px-3 py-1.5 font-code text-[12px] text-slate-600">{a.value}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

// --- Members Of (role checkboxes for easy granting) ---
function MembersOfTab({ engine, sessionId, detail, allRoles, pushOp, runSql }: DetailProps) {
  if (engine === "redis") return <Note text="Redis has no roles." />;
  const memberKeys = new Set(detail.member_of.map((g) => `${g.role}\u0000${g.role_host ?? ""}`));
  const toggle = (role: DbPrincipal, next: boolean) => {
    const m = detail.principal;
    if (engine === "mysql" || engine === "mariadb") {
      const r = `${quoteIdent(engine, role.name)}@${quoteIdent(engine, role.host ?? "%")}`;
      const t = mysqlAccount(engine, m.name, m.host);
      const sql = next ? `GRANT ${r} TO ${t};` : `REVOKE ${r} FROM ${t};`;
      pushOp(`${next ? "Grant" : "Revoke"} role ${role.name} ${next ? "to" : "from"} ${m.name}`, sql, runSql(sql));
    } else if (engine === "postgres") {
      const sql = next
        ? `GRANT ${quoteIdent(engine, role.name)} TO ${quoteIdent(engine, m.name)};`
        : `REVOKE ${quoteIdent(engine, role.name)} FROM ${quoteIdent(engine, m.name)};`;
      pushOp(`${next ? "Grant" : "Revoke"} role ${role.name} ${next ? "to" : "from"} ${m.name}`, sql, runSql(sql));
    } else if (engine === "sql_server") {
      const serverRole = role.host === "server" || (role.comment ?? "").includes("server");
      const verb = serverRole ? "SERVER ROLE" : "ROLE";
      const sql = `ALTER ${verb} ${quoteIdent(engine, role.name)} ${next ? "ADD" : "DROP"} MEMBER ${quoteIdent(engine, m.name)};`;
      pushOp(`${next ? "Grant" : "Revoke"} role ${role.name} ${next ? "to" : "from"} ${m.name}`, sql, runSql(sql));
    } else if (engine === "mongodb") {
      const preview = next
        ? `db.grantRolesToUser(${qstr(m.name)}, [{ role: ${qstr(role.name)}, db: ${qstr(role.host ?? "")} }])`
        : `db.revokeRolesFromUser(${qstr(m.name)}, [{ role: ${qstr(role.name)}, db: ${qstr(role.host ?? "")} }])`;
      pushOp(`${next ? "Grant" : "Revoke"} role ${role.name} ${next ? "to" : "from"} ${m.name}`, preview, () =>
        next
          ? api.grant(sessionId, { grantee: m.name, grantee_host: m.host ?? undefined, target: { role: { role: role.name, host: role.host } }, with_grant_option: false })
          : api.revoke(sessionId, { grantee: m.name, grantee_host: m.host ?? undefined, target: { role: { role: role.name, host: role.host } } }),
      );
    }
  };
  if (allRoles.length === 0) return <Note text="No roles on this server." />;
  return (
    <div className="grid grid-cols-1 gap-x-6 gap-y-1.5 md:grid-cols-2">
      {allRoles.map((r) => {
        const checked = memberKeys.has(`${r.name}\u0000${r.host ?? ""}`);
        return (
          <label key={`${r.name}\u0000${r.host ?? ""}`} className="flex cursor-pointer items-center gap-1.5 text-[12.5px] text-slate-700">
            <input type="checkbox" className="accent-accent" checked={checked} onChange={(e) => toggle(r, e.target.checked)} />
            <span className="font-code text-[12px]">{r.name}</span>
            {r.host && <span className="text-[11px] text-slate-400">@{r.host}</span>}
          </label>
        );
      })}
    </div>
  );
}

// --- Members (of a role) ---
function MembersTab({ engine, sessionId, detail, allUsers, pushOp, runSql }: DetailProps) {
  const [pick, setPick] = useState("");
  const role = detail.principal;
  const remove = (member: string, memberHost?: string | null) => {
    if (engine === "mysql" || engine === "mariadb") {
      const r = `${quoteIdent(engine, role.name)}@${quoteIdent(engine, role.host ?? "%")}`;
      const sql = `REVOKE ${r} FROM ${mysqlAccount(engine, member, memberHost)};`;
      pushOp(`Remove ${member} from ${role.name}`, sql, runSql(sql));
    } else if (engine === "postgres") {
      const sql = `REVOKE ${quoteIdent(engine, role.name)} FROM ${quoteIdent(engine, member)};`;
      pushOp(`Remove ${member} from ${role.name}`, sql, runSql(sql));
    } else if (engine === "sql_server") {
      const serverRole = role.host === "server" || (role.comment ?? "").includes("server");
      const verb = serverRole ? "SERVER ROLE" : "ROLE";
      const sql = `ALTER ${verb} ${quoteIdent(engine, role.name)} DROP MEMBER ${quoteIdent(engine, member)};`;
      pushOp(`Remove ${member} from ${role.name}`, sql, runSql(sql));
    } else if (engine === "mongodb") {
      const preview = `db.revokeRolesFromUser(${qstr(member)}, [{ role: ${qstr(role.name)}, db: ${qstr(role.host ?? "")} }])`;
      pushOp(`Remove ${member} from ${role.name}`, preview, () =>
        api.revoke(sessionId, { grantee: member, grantee_host: memberHost ?? undefined, target: { role: { role: role.name, host: role.host } } }),
      );
    }
  };
  const add = () => {
    const m = allUsers.find((u) => u.name === pick);
    if (!m) return;
    if (engine === "mysql" || engine === "mariadb") {
      const r = `${quoteIdent(engine, role.name)}@${quoteIdent(engine, role.host ?? "%")}`;
      const sql = `GRANT ${r} TO ${mysqlAccount(engine, m.name, m.host)};`;
      pushOp(`Add ${m.name} to ${role.name}`, sql, runSql(sql));
    } else if (engine === "postgres") {
      const sql = `GRANT ${quoteIdent(engine, role.name)} TO ${quoteIdent(engine, m.name)};`;
      pushOp(`Add ${m.name} to ${role.name}`, sql, runSql(sql));
    } else if (engine === "sql_server") {
      const serverRole = role.host === "server" || (role.comment ?? "").includes("server");
      const verb = serverRole ? "SERVER ROLE" : "ROLE";
      const sql = `ALTER ${verb} ${quoteIdent(engine, role.name)} ADD MEMBER ${quoteIdent(engine, m.name)};`;
      pushOp(`Add ${m.name} to ${role.name}`, sql, runSql(sql));
    } else if (engine === "mongodb") {
      const preview = `db.grantRolesToUser(${qstr(m.name)}, [{ role: ${qstr(role.name)}, db: ${qstr(role.host ?? "")} }])`;
      pushOp(`Add ${m.name} to ${role.name}`, preview, () =>
        api.grant(sessionId, { grantee: m.name, grantee_host: m.host ?? undefined, target: { role: { role: role.name, host: role.host } }, with_grant_option: false }),
      );
    }
    setPick("");
  };
  return (
    <div>
      {detail.members.length === 0 && (
        <div className="mb-3 text-[12.5px] text-slate-400">No members yet.</div>
      )}
      <div className="mb-3 space-y-1">
        {detail.members.map((g, i) => (
          <div key={i} className="flex items-center gap-2 text-[12.5px]">
            <span className="font-code text-[12px] text-slate-800">{g.member}</span>
            {g.member_host && <span className="text-[11px] text-slate-400">@{g.member_host}</span>}
            {g.admin_option && <span className="text-[11px] text-amber-600">admin</span>}
            <button className="text-[12px] text-rose-600 hover:underline" onClick={() => remove(g.member, g.member_host)}>
              Remove
            </button>
          </div>
        ))}
      </div>
      {allUsers.length > 0 && (
        <div className="flex max-w-[420px] gap-2">
          <select className={inputCls} value={pick} onChange={(e) => setPick(e.target.value)}>
            <option value="">Add member…</option>
            {allUsers.map((u) => (
              <option key={u.name} value={u.name}>{u.name}</option>
            ))}
          </select>
          <button className={btnSecondary} disabled={!pick} onClick={add}>Add</button>
        </div>
      )}
    </div>
  );
}

// --- Server Privileges ---
function ServerTab({ engine, detail, pushOp, runSql }: DetailProps) {
  if (engine === "mysql" || engine === "mariadb") {
    const cur = new Set(detail.global_privileges.map((p) => p.toUpperCase()));
    const [grantOpt, setGrantOpt] = useState(
      detail.attributes.some((a) => a.key === "Grant option"),
    );
    const toggle = (priv: string, next: boolean) => {
      const account = mysqlAccount(engine, detail.principal.name, detail.principal.host);
      const sql = next
        ? `GRANT ${priv} ON *.* TO ${account};`
        : `REVOKE ${priv} ON *.* FROM ${account};`;
      pushOp(`${next ? "Grant" : "Revoke"} ${priv} globally`, sql, runSql(sql));
    };
    const toggleGrantOpt = (next: boolean) => {
      const account = mysqlAccount(engine, detail.principal.name, detail.principal.host);
      const sql = next
        ? `GRANT USAGE ON *.* TO ${account} WITH GRANT OPTION;`
        : `REVOKE ALL PRIVILEGES, GRANT OPTION FROM ${account};`;
      setGrantOpt(next);
      pushOp(`${next ? "Grant" : "Revoke"} grant option`, sql, runSql(sql));
    };
    return (
      <div>
        <Section title="Global privileges">
          <PrivGrid options={MYSQL_GLOBAL} checked={cur} onToggle={toggle} />
        </Section>
        <label className="flex cursor-pointer items-center gap-1.5 text-[12.5px] text-slate-700">
          <input type="checkbox" className="accent-accent" checked={grantOpt} onChange={(e) => toggleGrantOpt(e.target.checked)} />
          WITH GRANT OPTION
        </label>
      </div>
    );
  }
  if (engine === "postgres") {
    const attrs = new Map(detail.attributes.map((a) => [a.key, a.value]));
    const isYes = (k: string) => attrs.get(k) === "yes";
    const toggle = (attr: { key: string; sql: string; noSql: string }, next: boolean, label: string) => {
      const sql = `ALTER ROLE ${quoteIdent(engine, detail.principal.name)} WITH ${next ? attr.sql : attr.noSql};`;
      pushOp(`${next ? "Grant" : "Revoke"} ${label}`, sql, runSql(sql));
    };
    return (
      <div>
        <Section title="Role attributes">
          <div className="grid grid-cols-2 gap-x-6 gap-y-1.5 md:grid-cols-3">
            {PG_ATTRS.map((a) => (
              <label key={a.key} className="flex cursor-pointer items-center gap-1.5 text-[12.5px] text-slate-700">
                <input
                  type="checkbox"
                  className="accent-accent"
                  checked={a.key === "LOGIN" ? detail.principal.can_login : a.key === "SUPERUSER" ? detail.principal.is_super : isYes(a.key === "CREATEDB" ? "Create DB" : a.key === "CREATEROLE" ? "Create role" : a.key === "REPLICATION" ? "Replication" : a.key)}
                  onChange={(e) => toggle(a, e.target.checked, a.key)}
                />
                <span className="font-code text-[12px]">{a.key}</span>
              </label>
            ))}
          </div>
        </Section>
        <Note text="Table-level privileges live under Object Privileges." />
      </div>
    );
  }
  if (engine === "sql_server") {
    return (
      <div>
        <Section title="Server roles">
          <Note text="Use Members Of to manage server-role membership (roles marked “server”)." />
        </Section>
        <Section title="Login state">
          <Note text="Toggle login / password from the General tab." />
        </Section>
      </div>
    );
  }
  return <Note text="No server-level privileges on this engine." />;
}

// --- Object Privileges ---
function ObjectsTab({ engine, detail, pushOp, runSql, sessionDb }: DetailProps) {
  const [db, setDb] = useState(sessionDb ?? "");
  const [schema, setSchema] = useState(engine === "postgres" ? "public" : engine === "sql_server" ? "dbo" : "");
  const [table, setTable] = useState("");
  const [mode, setMode] = useState<"table" | "database">("table");
  const vocab = engine === "postgres" ? PG_OBJ : engine === "sql_server" && mode === "database" ? MSSQL_DB : engine === "sql_server" ? MSSQL_OBJ : MYSQL_OBJ;
  const [sel, setSel] = useState<Set<string>>(new Set());

  if (engine === "mongodb") {
    if (detail.object_privileges.length === 0) {
      return <Note text="No custom privileges — grant roles instead." />;
    }
    return (
      <table className="w-full border-collapse text-left text-[12.5px]">
        <thead>
          <tr className="text-slate-500">
            <th className="border-b border-slate-200 px-3 py-1.5 text-[11.5px] font-semibold">Resource</th>
            <th className="border-b border-slate-200 px-3 py-1.5 text-[11.5px] font-semibold">Actions</th>
          </tr>
        </thead>
        <tbody>
          {detail.object_privileges.map((o, i) => (
            <tr key={i} className="border-b border-slate-200/70">
              <td className="px-3 py-1.5 font-code text-[12px]">{o.database ?? "—"}{o.table ? `.${o.table}` : ".*"}</td>
              <td className="px-3 py-1.5 font-code text-[12px] text-slate-600">{o.privileges.join(", ")}</td>
            </tr>
          ))}
        </tbody>
      </table>
    );
  }

  const revokeRow = (o: (typeof detail.object_privileges)[number]) => {
    const p = detail.principal;
    const privs = o.privileges.join(", ");
    if (engine === "mysql" || engine === "mariadb") {
      const obj = o.table ? `${quoteIdent(engine, o.database ?? "*")}.${quoteIdent(engine, o.table)}` : `${quoteIdent(engine, o.database ?? "*")}.*`;
      const sql = `REVOKE ${privs} ON ${obj} FROM ${mysqlAccount(engine, p.name, p.host)};`;
      pushOp(`Revoke ${privs} on ${o.database ?? "*"}${o.table ? "." + o.table : ""}`, sql, runSql(sql));
    } else if (engine === "postgres") {
      const obj = o.table ? `TABLE ${pgTable(engine, o.schema, o.table)}` : `DATABASE ${quoteIdent(engine, o.database ?? "")}`;
      const sql = `REVOKE ${privs} ON ${obj} FROM ${quoteIdent(engine, p.name)};`;
      pushOp(`Revoke ${privs}`, sql, runSql(sql));
    } else if (engine === "sql_server") {
      const obj = o.table
        ? `OBJECT::${quoteIdent(engine, o.schema ?? "dbo")}.${quoteIdent(engine, o.table)}`
        : o.schema
          ? `SCHEMA::${quoteIdent(engine, o.schema)}`
          : `DATABASE::${quoteIdent(engine, o.database ?? sessionDb ?? "")}`;
      const sql = `REVOKE ${privs} ON ${obj} TO ${quoteIdent(engine, p.name)};`;
      pushOp(`Revoke ${privs}`, sql, runSql(sql));
    }
  };

  const grantNew = () => {
    if (sel.size === 0) return;
    const p = detail.principal;
    const privs = [...sel].join(", ");
    if (engine === "mysql" || engine === "mariadb") {
      const d = db.trim() || "*";
      const obj = mode === "table" && table.trim()
        ? `${quoteIdent(engine, d)}.${quoteIdent(engine, table.trim())}`
        : `${quoteIdent(engine, d)}.*`;
      const sql = `GRANT ${privs} ON ${obj} TO ${mysqlAccount(engine, p.name, p.host)};`;
      pushOp(`Grant ${privs} on ${d}${mode === "table" && table.trim() ? "." + table.trim() : ".*"}`, sql, runSql(sql));
    } else if (engine === "postgres") {
      if (mode === "table") {
        if (!table.trim()) return;
        const obj = `TABLE ${pgTable(engine, schema, table.trim())}`;
        const sql = `GRANT ${privs} ON ${obj} TO ${quoteIdent(engine, p.name)};`;
        pushOp(`Grant ${privs} on ${table.trim()}`, sql, runSql(sql));
      } else {
        if (!db.trim()) return;
        const sql = `GRANT ${privs} ON DATABASE ${quoteIdent(engine, db.trim())} TO ${quoteIdent(engine, p.name)};`;
        pushOp(`Grant ${privs} on database ${db.trim()}`, sql, runSql(sql));
      }
    } else if (engine === "sql_server") {
      const obj = mode === "table" && table.trim()
        ? `OBJECT::${quoteIdent(engine, schema || "dbo")}.${quoteIdent(engine, table.trim())}`
        : mode === "table"
          ? `SCHEMA::${quoteIdent(engine, schema || "dbo")}`
          : `DATABASE::${quoteIdent(engine, db.trim() || sessionDb || "")}`;
      const sql = `GRANT ${privs} ON ${obj} TO ${quoteIdent(engine, p.name)};`;
      pushOp(`Grant ${privs}`, sql, runSql(sql));
    }
    setSel(new Set());
  };

  return (
    <div>
      <Section title="Current grants">
        {detail.object_privileges.length === 0 && (
          <div className="mb-2 text-[12.5px] text-slate-400">No object grants.</div>
        )}
        <div className="space-y-1">
          {detail.object_privileges.map((o, i) => (
            <div key={i} className="flex items-center gap-2 text-[12.5px]">
              <span className="font-code text-[12px] text-slate-800">
                {o.database ?? o.schema ?? ""}{o.table ? (o.database ? `.${o.table}` : o.schema ? `.${o.table}` : o.table) : o.table ? "" : ".*"}
              </span>
              <span className="font-code text-[11.5px] text-slate-500">{o.privileges.join(", ")}</span>
              {o.grantable && <span className="text-[11px] text-amber-600">grantable</span>}
              <button className="text-[12px] text-rose-600 hover:underline" onClick={() => revokeRow(o)}>
                Revoke
              </button>
            </div>
          ))}
        </div>
      </Section>
      <Section title="Grant new">
        <div className="mb-2 flex gap-2">
          {(["table", "database"] as const).map((m) => (
            <button
              key={m}
              className={`h-7 rounded-md px-2.5 text-[12.5px] capitalize ${mode === m ? "bg-accent font-medium text-white" : "border border-slate-300 bg-white text-slate-600"}`}
              onClick={() => setMode(m)}
            >
              {m === "table" ? (engine === "postgres" || engine === "sql_server" ? "Table / schema" : "Table") : "Database"}
            </button>
          ))}
        </div>
        <div className="mb-2 grid max-w-[560px] grid-cols-3 gap-2">
          <div>
            <label className={labelCls}>{engine === "postgres" || engine === "sql_server" ? "Schema" : "Database (* = all)"}</label>
            {engine === "mysql" || engine === "mariadb" ? (
              <input className={inputCls} value={db} placeholder="*" onChange={(e) => setDb(e.target.value)} />
            ) : (
              <input className={inputCls} value={schema} placeholder={engine === "postgres" ? "public" : "dbo"} onChange={(e) => setSchema(e.target.value)} />
            )}
          </div>
          <div>
            <label className={labelCls}>Table {mode === "table" && engine !== "sql_server" ? "*" : "(optional)"}</label>
            <input className={inputCls} value={table} placeholder={mode === "table" ? "required" : "empty = level above"} onChange={(e) => setTable(e.target.value)} disabled={mode === "database"} />
          </div>
          {(engine === "postgres" || engine === "sql_server") && mode === "database" && (
            <div>
              <label className={labelCls}>Database *</label>
              <input className={inputCls} value={db} placeholder={sessionDb ?? "database"} onChange={(e) => setDb(e.target.value)} />
            </div>
          )}
        </div>
        <PrivGrid
          options={vocab}
          checked={sel}
          onToggle={(name, next) =>
            setSel((prev) => {
              const n = new Set(prev);
              if (next) n.add(name);
              else n.delete(name);
              return n;
            })
          }
        />
        <div className="mt-2">
          <button className={btnPrimary} disabled={sel.size === 0} onClick={grantNew}>
            Stage Grant
          </button>
        </div>
      </Section>
    </div>
  );
}

// --- SQL Preview ---
function SqlTab({
  pending,
  onApply,
  onDiscard,
}: {
  pending: PendingOp[];
  onApply: () => void;
  onDiscard: () => void;
}) {
  if (pending.length === 0) {
    return (
      <Note text="No staged changes. Toggle a privilege, grant a role, create or drop a user — each change stages its SQL here for review before Apply." />
    );
  }
  return (
    <div>
      <div className="mb-2 flex gap-2">
        <button className={btnPrimary} onClick={onApply}>
          Apply {pending.length} change(s)
        </button>
        <button className={btnSecondary} onClick={onDiscard}>
          Discard all
        </button>
      </div>
      <div className="space-y-2">
        {pending.map((op) => (
          <div key={op.id} className="overflow-hidden rounded-md border border-slate-200">
            <div className="border-b border-slate-200/70 bg-sidebar px-3 py-1.5 text-[12px] font-medium text-slate-700">
              {op.id}. {op.label}
            </div>
            <pre className="custom-scrollbar max-h-[220px] overflow-auto bg-white p-3 font-code text-[12px] whitespace-pre-wrap text-slate-800">
              {op.preview}
            </pre>
          </div>
        ))}
      </div>
    </div>
  );
}
