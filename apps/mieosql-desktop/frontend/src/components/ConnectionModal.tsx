import { useEffect, useState } from "react";
import { Database, HardDrive, Loader2, Plug, Trash2, X } from "lucide-react";

import type { Engine, ProfileInput, SslMode } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { useStore } from "../lib/store";

type FormState = {
  name: string;
  engine: Engine;
  host: string;
  port: string;
  user: string;
  password: string;
  passwordTouched: boolean;
  database: string;
  file: string;
  sslMode: SslMode;
};

const ENGINE_OPTIONS: { value: Engine; label: string; enabled: boolean }[] = [
  { value: "postgres", label: "PostgreSQL", enabled: true },
  { value: "mysql", label: "MySQL", enabled: true },
  { value: "mariadb", label: "MariaDB", enabled: true },
  { value: "sqlite", label: "SQLite (file)", enabled: true },
  { value: "sql_server", label: "SQL Server — roadmap P1", enabled: false },
  { value: "mongodb", label: "MongoDB — roadmap P2", enabled: false },
  { value: "redis", label: "Redis — roadmap P2", enabled: false },
  { value: "oracle", label: "Oracle — roadmap P3", enabled: false },
  { value: "snowflake", label: "Snowflake — roadmap P4", enabled: false },
];

const DEFAULT_PORTS: Partial<Record<Engine, number>> = {
  postgres: 5432,
  mysql: 3306,
  mariadb: 3306,
  sql_server: 1433,
  mongodb: 27017,
  redis: 6379,
  oracle: 1521,
};

const SSL_MODES: { value: SslMode; label: string }[] = [
  { value: "prefer", label: "Prefer (default)" },
  { value: "disable", label: "Disable" },
  { value: "require", label: "Require" },
  { value: "verify_ca", label: "Verify CA" },
  { value: "verify_full", label: "Verify Full" },
];

function emptyForm(): FormState {
  return {
    name: "",
    engine: "postgres",
    host: "127.0.0.1",
    port: "5432",
    user: "",
    password: "",
    passwordTouched: false,
    database: "",
    file: "",
    sslMode: "prefer",
  };
}

export function ConnectionModal() {
  const store = useStore();
  const { open, profileId } = store.connectionModal;
  const editing = profileId
    ? store.profiles.find((p) => p.id === profileId)
    : undefined;

  const [form, setForm] = useState<FormState>(emptyForm());
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [testResult, setTestResult] = useState<{
    ok: boolean;
    text: string;
  } | null>(null);

  useEffect(() => {
    if (!open) return;
    setTestResult(null);
    if (editing) {
      setForm({
        name: editing.name,
        engine: editing.engine,
        host: editing.host ?? "",
        port: editing.port ? String(editing.port) : "",
        user: editing.username ?? "",
        password: "",
        passwordTouched: false,
        database: editing.database ?? "",
        file: editing.file ?? "",
        sslMode: editing.ssl_mode,
      });
    } else {
      setForm(emptyForm());
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, profileId]);

  if (!open) return null;

  const isFile = form.engine === "sqlite";

  const set = (patch: Partial<FormState>) =>
    setForm((f) => ({ ...f, ...patch }));

  const changeEngine = (engine: Engine) => {
    const currentDefault = DEFAULT_PORTS[form.engine];
    const keepPort =
      form.port !== "" && Number(form.port) !== (currentDefault ?? -1);
    set({
      engine,
      port: keepPort ? form.port : String(DEFAULT_PORTS[engine] ?? ""),
      sslMode: engine === "sqlite" ? "disable" : form.sslMode,
    });
  };

  const buildInput = (): ProfileInput => ({
    id: editing ? editing.id : undefined,
    name: form.name.trim() || "Unnamed",
    engine: form.engine,
    host: isFile ? undefined : form.host.trim() || undefined,
    port: isFile
      ? undefined
      : form.port.trim()
        ? Number(form.port)
        : undefined,
    username: isFile ? undefined : form.user.trim() || undefined,
    database: form.database.trim() || undefined,
    file: isFile ? form.file.trim() || undefined : undefined,
    params: {},
    ssl_mode: isFile ? "disable" : form.sslMode,
    ssh: undefined,
    color: undefined,
    group: undefined,
    password: form.passwordTouched ? form.password : undefined,
    ssh_password: undefined,
  });

  const onTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const info = await api.testConnection(buildInput());
      setTestResult({
        ok: true,
        text: `${info.engine} ${info.version}${
          info.current_database ? ` · db ${info.current_database}` : ""
        }`,
      });
    } catch (err) {
      setTestResult({ ok: false, text: errorText(err) });
    } finally {
      setTesting(false);
    }
  };

  const onSave = async () => {
    setSaving(true);
    try {
      const profile = await store.saveProfile(buildInput());
      store.setConnectionModal(false);
      store.toast("success", `Connection “${profile.name}” saved`);
      void store.openSession(profile.id).catch((err) => {
        store.toast("error", errorText(err));
      });
    } catch (err) {
      store.toast("error", errorText(err));
    } finally {
      setSaving(false);
    }
  };

  const onDelete = async () => {
    if (!editing) return;
    try {
      await store.deleteProfile(editing.id);
      store.setConnectionModal(false);
    } catch (err) {
      store.toast("error", errorText(err));
    }
  };

  const inputCls =
    "w-full rounded border border-slate-300 bg-white px-2 py-1 text-[11px] text-slate-800 focus:border-sky-400 focus:outline-none";
  const labelCls = "mb-0.5 block text-[10px] font-medium text-slate-500 uppercase tracking-wide";

  return (
    <div
      className="fixed inset-0 z-[80] grid place-items-center bg-slate-900/30"
      onMouseDown={() => store.setConnectionModal(false)}
    >
      <div
        className="w-[520px] max-w-[94vw] rounded border border-slate-300 bg-[#f8fafc] shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-10 items-center justify-between border-b border-slate-200 px-3">
          <div className="flex items-center gap-2">
            <Plug size={15} className="text-sky-600" />
            <span className="text-[13px] font-semibold text-slate-800">
              {editing ? "Connection Properties" : "New Connection"}
            </span>
          </div>
          <button
            className="rounded p-1 text-slate-400 hover:bg-slate-200 hover:text-slate-700"
            onClick={() => store.setConnectionModal(false)}
          >
            <X size={14} />
          </button>
        </div>

        <div className="space-y-3 p-4">
          <div className="grid grid-cols-[130px_1fr] items-center gap-3">
            <label className={labelCls}>Connection name</label>
            <input
              className={inputCls}
              value={form.name}
              placeholder="My database"
              onChange={(e) => set({ name: e.target.value })}
              autoFocus
            />
          </div>

          <div className="grid grid-cols-[130px_1fr] items-center gap-3">
            <label className={labelCls}>Engine</label>
            <select
              className={inputCls}
              value={form.engine}
              onChange={(e) => changeEngine(e.target.value as Engine)}
            >
              {ENGINE_OPTIONS.map((o) => (
                <option key={o.value} value={o.value} disabled={!o.enabled}>
                  {o.label}
                </option>
              ))}
            </select>
          </div>

          {isFile ? (
            <div className="grid grid-cols-[130px_1fr] items-center gap-3">
              <label className={labelCls}>Database file</label>
              <div className="flex items-center gap-2">
                <HardDrive size={13} className="shrink-0 text-slate-400" />
                <input
                  className={inputCls}
                  value={form.file}
                  placeholder="/path/to/database.db  (or :memory:)"
                  onChange={(e) => set({ file: e.target.value })}
                />
              </div>
            </div>
          ) : (
            <>
              <div className="grid grid-cols-[130px_1fr] items-center gap-3">
                <label className={labelCls}>Host</label>
                <input
                  className={inputCls}
                  value={form.host}
                  onChange={(e) => set({ host: e.target.value })}
                />
              </div>
              <div className="grid grid-cols-[130px_1fr] items-center gap-3">
                <label className={labelCls}>Port</label>
                <input
                  className={inputCls}
                  value={form.port}
                  onChange={(e) =>
                    set({ port: e.target.value.replace(/[^0-9]/g, "") })
                  }
                />
              </div>
              <div className="grid grid-cols-[130px_1fr] items-center gap-3">
                <label className={labelCls}>User name</label>
                <input
                  className={inputCls}
                  value={form.user}
                  onChange={(e) => set({ user: e.target.value })}
                />
              </div>
              <div className="grid grid-cols-[130px_1fr] items-center gap-3">
                <label className={labelCls}>Password</label>
                <input
                  className={inputCls}
                  type="password"
                  value={form.password}
                  placeholder={editing ? "(leave blank to keep stored secret)" : ""}
                  onChange={(e) =>
                    set({ password: e.target.value, passwordTouched: true })
                  }
                />
              </div>
              <div className="grid grid-cols-[130px_1fr] items-center gap-3">
                <label className={labelCls}>SSL mode</label>
                <select
                  className={inputCls}
                  value={form.sslMode}
                  onChange={(e) => set({ sslMode: e.target.value as SslMode })}
                >
                  {SSL_MODES.map((m) => (
                    <option key={m.value} value={m.value}>
                      {m.label}
                    </option>
                  ))}
                </select>
              </div>
            </>
          )}

          <div className="grid grid-cols-[130px_1fr] items-center gap-3">
            <label className={labelCls}>Database</label>
            <div className="flex items-center gap-2">
              <Database size={13} className="shrink-0 text-slate-400" />
              <input
                className={inputCls}
                value={form.database}
                placeholder={isFile ? "main (optional)" : "default database (optional)"}
                onChange={(e) => set({ database: e.target.value })}
              />
            </div>
          </div>

          <p className="text-[10px] leading-relaxed text-slate-400">
            Credentials are stored in an encrypted vault (XChaCha20-Poly1305,
            separate master key). SSH tunnels arrive in phase P1.
          </p>

          {testResult && (
            <div
              className={`rounded border px-3 py-2 text-[11px] ${
                testResult.ok
                  ? "border-emerald-200 bg-emerald-50 text-emerald-800"
                  : "border-rose-200 bg-rose-50 text-rose-700"
              }`}
            >
              {testResult.ok ? "✓ Connected: " : "✗ "}
              <span className="font-code">{testResult.text}</span>
            </div>
          )}
        </div>

        <div className="flex items-center justify-between border-t border-slate-200 px-3 py-2">
          <div>
            {editing && (
              <button
                className="flex items-center gap-1 rounded px-2 py-1 text-[11px] text-rose-600 hover:bg-rose-50"
                onClick={onDelete}
              >
                <Trash2 size={12} /> Delete
              </button>
            )}
          </div>
          <div className="flex items-center gap-2">
            <button
              className="flex items-center gap-1.5 rounded border border-slate-300 bg-white px-3 py-1 text-[11px] text-slate-700 hover:bg-slate-100 disabled:opacity-50"
              onClick={onTest}
              disabled={testing}
            >
              {testing ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                <Plug size={12} />
              )}
              Test Connection
            </button>
            <button
              className="flex items-center gap-1.5 rounded bg-sky-600 px-3 py-1 text-[11px] font-medium text-white hover:bg-sky-700 disabled:opacity-50"
              onClick={onSave}
              disabled={saving}
            >
              {saving && <Loader2 size={12} className="animate-spin" />}
              Save &amp; Connect
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
