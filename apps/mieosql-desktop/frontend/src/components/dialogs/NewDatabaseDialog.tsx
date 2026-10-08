// New Database dialog. Opened via `store.openObjectDialog("new-database",
// { sessionId })`. MySQL/MariaDB optionally take a character set; PostgreSQL
// and SQL Server take a bare name. All other engines are refused with a notice.

import { useState } from "react";
import { Database, Loader2, Play } from "lucide-react";

import type { Engine } from "../../lib/bindings";
import { api, errorText } from "../../lib/api";
import { ENGINE_META } from "../../lib/engines";
import { quoteIdent, isValidObjectName } from "../../lib/table-ddl";
import { useStore } from "../../lib/store";
import {
  DLG_BTN,
  DLG_BTN_PRIMARY,
  DLG_INPUT,
  DLG_LABEL,
  DLG_PRE,
  DialogError,
  DialogNotice,
  ObjectDialogShell,
} from "./DialogChrome";

const CREATE_DATABASE_ENGINES: readonly Engine[] = [
  "mysql",
  "mariadb",
  "postgres",
  "sql_server",
];

function buildSql(engine: Engine, name: string, charset: string): string {
  const ident = quoteIdent(engine, name);
  if (engine === "mysql" || engine === "mariadb") {
    return `CREATE DATABASE ${ident} CHARACTER SET ${charset}`;
  }
  return `CREATE DATABASE ${ident}`;
}

export function NewDatabaseDialog({
  sessionId,
  onClose,
}: {
  sessionId: string | null;
  onClose: () => void;
}) {
  const session = useStore(
    (s) => s.sessions.find((x) => x.id === sessionId) ?? null,
  );
  const toast = useStore((s) => s.toast);

  const [name, setName] = useState("");
  const [charset, setCharset] = useState("utf8mb4");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const engine = session?.engine ?? null;
  const supported =
    engine !== null && CREATE_DATABASE_ENGINES.includes(engine);
  const nameValid = isValidObjectName(name);
  const showNameHint = name.trim().length > 0 && !nameValid;
  const sql =
    supported && engine && nameValid ? buildSql(engine, name.trim(), charset) : null;
  const canRun = supported && nameValid && !running && sessionId !== null;

  const run = async () => {
    if (!sessionId || !engine || !nameValid) return;
    setRunning(true);
    setError(null);
    try {
      const res = await api.executeBatch(sessionId, [buildSql(engine, name.trim(), charset)], false);
      if (res.failed > 0) {
        setError(res.errors.join("\n") || "The backend did not apply the statement.");
        return;
      }
      toast("success", `Database “${name.trim()}” created`);
      window.dispatchEvent(new CustomEvent("mieosql:refresh-tree"));
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setRunning(false);
    }
  };

  return (
    <ObjectDialogShell
      title="New Database"
      width={560}
      busy={running}
      onClose={onClose}
      footer={
        <>
          <button className={DLG_BTN} onClick={onClose} disabled={running}>
            Cancel
          </button>
          <button
            className={DLG_BTN_PRIMARY}
            onClick={() => void run()}
            disabled={!canRun}
            title={!supported ? "Unsupported engine" : undefined}
          >
            {running ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <Play size={13} strokeWidth={1.8} />
            )}
            Create Database
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        <div className="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12px] text-slate-600">
          <Database size={13} strokeWidth={1.7} className="shrink-0 text-slate-500" />
          {session ? (
            <>
              <span className="font-medium text-slate-800">{session.name}</span>
              <span>{ENGINE_META[session.engine].label}</span>
              {session.database && <span>· current db {session.database}</span>}
            </>
          ) : (
            <span>No active session.</span>
          )}
        </div>

        {!session && (
          <DialogError text="Open a connection before creating a database." />
        )}

        {session && !supported && engine && (
          <DialogNotice>
            {ENGINE_META[engine].label} connections do not support CREATE
            DATABASE. Use the New Table dialog instead.
          </DialogNotice>
        )}

        <div>
          <label className={DLG_LABEL}>Database name *</label>
          <input
            className={DLG_INPUT}
            value={name}
            placeholder="analytics"
            autoFocus
            disabled={running}
            onChange={(e) => setName(e.target.value)}
          />
          {showNameHint && (
            <p className="mt-1 text-[11.5px] text-rose-600">
              Use letters, digits, &quot;_&quot; and &quot;$&quot; only — no
              spaces or quotes.
            </p>
          )}
        </div>

        {(engine === "mysql" || engine === "mariadb") && (
          <div>
            <label className={DLG_LABEL}>Character set</label>
            <select
              className={DLG_INPUT}
              value={charset}
              disabled={running}
              onChange={(e) => setCharset(e.target.value)}
            >
              <option value="utf8mb4">utf8mb4 (recommended)</option>
              <option value="utf8mb3">utf8mb3</option>
            </select>
          </div>
        )}

        <div>
          <label className={DLG_LABEL}>Preview</label>
          {sql ? (
            <pre className={DLG_PRE}>{sql};</pre>
          ) : (
            <pre className={`${DLG_PRE} text-slate-400`}>—</pre>
          )}
        </div>

        {error && <DialogError text={error} />}
      </div>
    </ObjectDialogShell>
  );
}
