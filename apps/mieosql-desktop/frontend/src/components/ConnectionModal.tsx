import { useEffect, useState } from "react";
import { Database, HardDrive, Loader2, Plug, Trash2, X } from "lucide-react";

import type { Engine, ProfileInput, SslMode } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { parseConnectionString } from "../lib/connection-string";
import {
  ENGINE_GROUPS,
  ENGINE_META,
  enginesInGroup,
  presetFor,
  presetsForEngine,
  type EnginePreset,
} from "../lib/engines";
import { useStore } from "../lib/store";

type FormState = {
  name: string;
  engine: Engine;
  /** Cloud preset flavor (`ProfileInput.variant`); `null` for a plain engine. */
  variant: string | null;
  host: string;
  port: string;
  user: string;
  password: string;
  passwordTouched: boolean;
  /** Navicat-style "Save password": checked persists encrypted, unchecked uses transient only. */
  savePassword: boolean;
  database: string;
  file: string;
  sslMode: SslMode;
  /** UI grouping folder (`ProfileInput.group`); empty = Ungrouped. */
  group: string;
  /** UI accent color (`ProfileInput.color`); empty = default. */
  color: string;
  /** Preset/engine parameters sent as `ProfileInput.params`. */
  params: Record<string, string>;
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
    variant: null,
    host: "127.0.0.1",
    port: String(ENGINE_META.postgres.defaultPort ?? ""),
    user: "",
    password: "",
    passwordTouched: false,
    savePassword: true,
    database: "",
    file: "",
    sslMode: "prefer",
    group: "",
    color: "",
    params: {},
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
  const [showConnStr, setShowConnStr] = useState(false);
  const [connStr, setConnStr] = useState("");
  const [connStrError, setConnStrError] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<{
    ok: boolean;
    text: string;
  } | null>(null);

  useEffect(() => {
    if (!open) return;
    setTestResult(null);
    setConnStr("");
    setConnStrError(null);
    setShowConnStr(false);
    if (editing) {
      setForm({
        name: editing.name,
        engine: editing.engine,
        variant: editing.variant ?? null,
        host: editing.host ?? "",
        port: editing.port ? String(editing.port) : "",
        user: editing.username ?? "",
        password: "",
        passwordTouched: false,
        savePassword: true,
        database: editing.database ?? "",
        file: editing.file ?? "",
        sslMode: editing.ssl_mode,
        group: editing.group ?? "",
        color: editing.color ?? "",
        params: { ...editing.params },
      });
    } else {
      setForm(emptyForm());
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, profileId]);

  if (!open) return null;

  const isFile = form.engine === "sqlite";
  const meta = ENGINE_META[form.engine];
  const presets = presetsForEngine(form.engine);
  const activePreset = presetFor(form.engine, form.variant);
  const hostPlaceholder =
    activePreset?.defaults.hostPlaceholder ??
    meta.hostPlaceholder ??
    "127.0.0.1";
  const databasePlaceholder = isFile
    ? "main (optional)"
    : (activePreset?.defaults.databaseHint ??
      meta.databasePlaceholder ??
      "default database (optional)");

  const set = (patch: Partial<FormState>) =>
    setForm((f) => ({ ...f, ...patch }));

  const changeEngine = (engine: Engine) => {
    const currentDefault = ENGINE_META[form.engine].defaultPort;
    const keepPort =
      form.port !== "" && Number(form.port) !== (currentDefault ?? -1);
    set({
      engine,
      variant: null,
      params: {},
      port: keepPort ? form.port : String(ENGINE_META[engine].defaultPort ?? ""),
      sslMode:
        engine === "sqlite"
          ? "disable"
          : form.engine === "sqlite"
            ? "prefer"
            : form.sslMode,
    });
  };

  const applyPreset = (preset: EnginePreset | null) => {
    if (!preset) {
      // Clear the chip: back to the plain engine with its defaults.
      set({
        variant: null,
        params: {},
        port: String(ENGINE_META[form.engine].defaultPort ?? ""),
        sslMode: form.engine === "sqlite" ? "disable" : "prefer",
      });
      return;
    }
    const { defaults } = preset;
    set({
      engine: preset.engine,
      variant: preset.variant,
      port: String(
        defaults.port ?? ENGINE_META[preset.engine].defaultPort ?? "",
      ),
      sslMode:
        defaults.ssl_mode ??
        (preset.engine === "sqlite" ? "disable" : "prefer"),
      params: { ...(defaults.params ?? {}) },
    });
  };

  const applyConnectionString = () => {
    try {
      const parsed = parseConnectionString(connStr);
      set({
        engine: parsed.engine,
        variant: null,
        host: parsed.host ?? "",
        port: String(parsed.port ?? ENGINE_META[parsed.engine].defaultPort ?? ""),
        user: parsed.username ?? "",
        password: parsed.password ?? "",
        passwordTouched: parsed.password !== undefined,
        savePassword: true,
        database: parsed.database ?? "",
        file: parsed.file ?? "",
        sslMode:
          parsed.sslMode ?? (parsed.engine === "sqlite" ? "disable" : "prefer"),
        params: parsed.params,
      });
      setConnStrError(null);
      setShowConnStr(false);
      store.toast("success", "Connection string applied");
    } catch (err) {
      setConnStrError(errorText(err));
    }
  };

  const buildInput = (): ProfileInput => ({
    id: editing ? editing.id : undefined,
    name: form.name.trim() || "Unnamed",
    engine: form.engine,
    variant: form.variant ?? undefined,
    host: isFile ? undefined : form.host.trim() || undefined,
    port: isFile
      ? undefined
      : form.port.trim()
        ? Number(form.port)
        : undefined,
    username: isFile ? undefined : form.user.trim() || undefined,
    database: form.database.trim() || undefined,
    file: isFile ? form.file.trim() || undefined : undefined,
    params: isFile ? {} : { ...form.params },
    ssl_mode: isFile ? "disable" : form.sslMode,
    ssh: undefined,
    color: form.color.trim() || undefined,
    group: form.group.trim() || undefined,
    // Save password unchecked → never persist: new connections send nothing,
    // edits send "" to clear any previously stored secret.
    password: !form.savePassword
      ? editing
        ? ""
        : undefined
      : form.passwordTouched
        ? form.password
        : undefined,
    ssh_password: undefined,
  });

  const onTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const input = buildInput();
      // Test always uses the typed password transiently, even when
      // "Save password" is unchecked.
      if (form.passwordTouched && form.password) {
        input.password = form.password;
      }
      const info = await api.testConnection(input);
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
      const transient = form.passwordTouched ? form.password : "";
      const profile = await store.saveProfile(buildInput());
      store.setConnectionModal(false);
      store.toast("success", `Connection “${profile.name}” saved`);
      void store.openSession(profile.id, transient || null).catch((err) => {
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
    "h-7 w-full rounded-md border border-slate-300 bg-white px-2 text-[13px] placeholder:text-slate-400 focus:ring-2 focus:ring-accent/40 focus:outline-none";
  const labelCls = "mb-0.5 block text-[11px] font-medium text-slate-500";

  return (
    <div
      className="fixed inset-0 z-[80] grid place-items-center bg-black/40"
      onMouseDown={() => store.setConnectionModal(false)}
    >
      <div
        className="w-[520px] max-w-[94vw] overflow-hidden rounded-xl border border-slate-300/70 bg-white shadow-[0_24px_70px_rgba(0,0,0,0.25)]"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-11 shrink-0 items-center justify-between border-b border-slate-200/80 px-4">
          <div className="flex items-center gap-2">
            <Plug size={14} strokeWidth={1.7} className="text-slate-500" />
            <span className="text-[15px] font-semibold text-slate-900">
              {editing ? "Connection Properties" : "New Connection"}
            </span>
          </div>
          <button
            className="grid h-7 w-7 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
            title="Close"
            onClick={() => store.setConnectionModal(false)}
          >
            <X size={14} strokeWidth={1.7} />
          </button>
        </div>

        <div className="space-y-3 p-4">
          <div>
            <button
              type="button"
              className="text-[12px] font-medium text-accent hover:underline"
              onClick={() => {
                setShowConnStr((v) => !v);
                setConnStrError(null);
              }}
            >
              {showConnStr ? "Hide connection string" : "Fill from connection string…"}
            </button>
            {showConnStr && (
              <div className="mt-2 space-y-1.5 rounded-md border border-slate-200 bg-slate-50 p-2.5">
                <div className="flex items-center gap-2">
                  <input
                    className={inputCls}
                    value={connStr}
                    placeholder="postgres://user:pass@host:5432/db?sslmode=require"
                    onChange={(e) => setConnStr(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") applyConnectionString();
                    }}
                  />
                  <button
                    type="button"
                    className="h-7 shrink-0 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover"
                    onClick={applyConnectionString}
                  >
                    Apply
                  </button>
                </div>
                <p className="text-[11px] leading-relaxed text-slate-400">
                  URL (postgres, mysql, sqlserver, mongodb, redis, kafka,
                  sqlite) or SQL Server key=value;… pairs. Fills host, port,
                  user, password and database below.
                </p>
                {connStrError && (
                  <p className="text-[11.5px] leading-relaxed text-rose-600">
                    {connStrError}
                  </p>
                )}
              </div>
            )}
          </div>

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
              {ENGINE_GROUPS.map((group) => (
                <optgroup key={group} label={group}>
                  {enginesInGroup(group).map((o) => (
                    <option
                      key={o.engine}
                      value={o.engine}
                      disabled={!o.implemented}
                    >
                      {o.implemented ? o.label : `${o.label} — ${o.roadmap}`}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          </div>

          {presets.length > 0 && (
            <div className="grid grid-cols-[130px_1fr] items-center gap-3">
              <label className={labelCls}>Cloud preset</label>
              <div className="flex flex-wrap items-center gap-1.5">
                {presets.map((preset) =>
                  activePreset?.id === preset.id ? (
                    <span
                      key={preset.id}
                      title={`Variant: ${preset.variant}`}
                      className="inline-flex h-6 items-center gap-1 rounded-md border border-accent/40 bg-accent/10 pr-1 pl-2 text-[11.5px] font-medium text-accent"
                    >
                      {preset.label}
                      <button
                        type="button"
                        title="Clear preset"
                        className="grid h-4 w-4 place-items-center rounded text-accent hover:bg-accent/15"
                        onClick={() => applyPreset(null)}
                      >
                        <X size={10} strokeWidth={1.8} />
                      </button>
                    </span>
                  ) : (
                    <button
                      key={preset.id}
                      type="button"
                      className="h-6 rounded-md border border-slate-300 bg-white px-2 text-[11.5px] text-slate-600 hover:bg-slate-50 hover:text-slate-900"
                      onClick={() => applyPreset(preset)}
                    >
                      {preset.label}
                    </button>
                  ),
                )}
              </div>
            </div>
          )}

          {isFile ? (
            <div className="grid grid-cols-[130px_1fr] items-center gap-3">
              <label className={labelCls}>Database file</label>
              <div className="flex items-center gap-2">
                <HardDrive size={14} strokeWidth={1.7} className="shrink-0 text-slate-500" />
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
                  placeholder={hostPlaceholder}
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
                  placeholder={
                    editing
                      ? form.savePassword
                        ? "(leave blank to keep stored secret)"
                        : "(not saved — used once for connect)"
                      : ""
                  }
                  onChange={(e) =>
                    set({ password: e.target.value, passwordTouched: true })
                  }
                />
              </div>
              <div className="grid grid-cols-[130px_1fr] items-center gap-3">
                <span />
                <label className="flex cursor-pointer items-center gap-2 text-[13px] text-slate-700">
                  <input
                    type="checkbox"
                    className="h-4 w-4 accent-[#007aff]"
                    checked={form.savePassword}
                    onChange={(e) => set({ savePassword: e.target.checked })}
                  />
                  <span>
                    Save password{" "}
                    <span className="text-slate-400">— encrypted, this user only</span>
                  </span>
                </label>
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
              <Database size={14} strokeWidth={1.7} className="shrink-0 text-slate-500" />
              <input
                className={inputCls}
                value={form.database}
                placeholder={databasePlaceholder}
                onChange={(e) => set({ database: e.target.value })}
              />
            </div>
          </div>

          <div className="grid grid-cols-[130px_1fr] items-center gap-3">
            <label className={labelCls}>Group</label>
            <input
              className={inputCls}
              value={form.group}
              placeholder="Ungrouped"
              onChange={(e) => set({ group: e.target.value })}
            />
          </div>

          <div className="grid grid-cols-[130px_1fr] items-center gap-3">
            <label className={labelCls}>Color</label>
            <div className="flex items-center gap-2">
              <input
                type="color"
                className="h-7 w-9 shrink-0 cursor-pointer rounded-md border border-slate-300 bg-white p-0.5"
                value={/^#[0-9a-fA-F]{6}$/.test(form.color) ? form.color : "#007aff"}
                onChange={(e) => set({ color: e.target.value })}
                title="Connection accent color"
              />
              <input
                className={inputCls}
                value={form.color}
                placeholder="default"
                onChange={(e) => set({ color: e.target.value })}
              />
              {form.color && (
                <button
                  type="button"
                  className="shrink-0 text-[12px] text-slate-500 hover:text-slate-700"
                  onClick={() => set({ color: "" })}
                >
                  Clear
                </button>
              )}
            </div>
          </div>

          {testResult && (
            <div
              className={`rounded-md border px-3 py-2 text-[12px] ${
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

        <div className="flex h-12 shrink-0 items-center justify-between border-t border-slate-200/80 bg-sidebar px-4">
          <div>
            {editing && (
              <button
                className="flex h-7 items-center gap-1 rounded-md px-2 text-[13px] text-rose-600 hover:bg-rose-50"
                onClick={onDelete}
              >
                <Trash2 size={12} strokeWidth={1.7} /> Delete
              </button>
            )}
          </div>
          <div className="flex items-center gap-2">
            <button
              className="flex h-7 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50 disabled:opacity-50"
              onClick={onTest}
              disabled={testing}
            >
              {testing ? (
                <Loader2 size={12} strokeWidth={1.7} className="animate-spin" />
              ) : (
                <Plug size={12} strokeWidth={1.7} />
              )}
              Test Connection
            </button>
            <button
              className="flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover disabled:opacity-50"
              onClick={onSave}
              disabled={saving}
            >
              {saving && <Loader2 size={12} strokeWidth={1.7} className="animate-spin" />}
              Save &amp; Connect
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
