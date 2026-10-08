// New Database dialog. Opened via `store.openObjectDialog("new-database",
// { sessionId })`. MySQL/MariaDB take a character set + matching collation
// (defaulted from the current database when known); PostgreSQL and SQL Server
// take a bare name. All other engines are refused with a notice.

import { useEffect, useState } from "react";
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

/**
 * Character sets with their collations (first entry = default). Covers the
 * common MySQL/MariaDB sets — `utf8mb4_general_ci` is the safe default that
 * exists on both MySQL and MariaDB (0900 collations are MySQL 8 only).
 */
const CHARSETS: readonly { charset: string; label?: string; collations: readonly string[] }[] = [
  {
    charset: "utf8mb4",
    label: "utf8mb4 (recommended)",
    collations: [
      "utf8mb4_general_ci",
      "utf8mb4_unicode_ci",
      "utf8mb4_unicode_520_ci",
      "utf8mb4_0900_ai_ci",
      "utf8mb4_vietnamese_ci",
      "utf8mb4_bin",
    ],
  },
  {
    charset: "utf8mb3",
    collations: [
      "utf8mb3_general_ci",
      "utf8mb3_unicode_ci",
      "utf8mb3_vietnamese_ci",
      "utf8mb3_bin",
    ],
  },
  {
    charset: "latin1",
    collations: [
      "latin1_swedish_ci",
      "latin1_general_ci",
      "latin1_general_cs",
      "latin1_bin",
    ],
  },
  { charset: "ascii", collations: ["ascii_general_ci", "ascii_bin"] },
  {
    charset: "utf16",
    collations: ["utf16_general_ci", "utf16_unicode_ci", "utf16_bin"],
  },
  { charset: "utf16le", collations: ["utf16le_general_ci", "utf16le_bin"] },
  {
    charset: "utf32",
    collations: ["utf32_general_ci", "utf32_unicode_ci", "utf32_bin"],
  },
  {
    charset: "ucs2",
    collations: ["ucs2_general_ci", "ucs2_unicode_ci", "ucs2_bin"],
  },
  { charset: "cp1251", collations: ["cp1251_general_ci", "cp1251_bin"] },
  { charset: "binary", collations: ["binary"] },
];

function collationsFor(charset: string, current?: string): readonly string[] {
  const known = CHARSETS.find((c) => c.charset === charset)?.collations ?? [];
  if (current && !known.includes(current)) return [current, ...known];
  return known.length > 0 ? known : [current ?? ""];
}

function defaultCollation(charset: string): string {
  return CHARSETS.find((c) => c.charset === charset)?.collations[0] ?? "";
}

function buildSql(
  engine: Engine,
  name: string,
  charset: string,
  collation: string,
): string {
  const ident = quoteIdent(engine, name);
  if (engine === "mysql" || engine === "mariadb") {
    const collate = collation ? ` COLLATE ${collation}` : "";
    return `CREATE DATABASE ${ident} CHARACTER SET ${charset}${collate}`;
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
  const [collation, setCollation] = useState(defaultCollation("utf8mb4"));
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const engine = session?.engine ?? null;
  const supported =
    engine !== null && CREATE_DATABASE_ENGINES.includes(engine);
  const nameValid = isValidObjectName(name);
  const showNameHint = name.trim().length > 0 && !nameValid;
  const sql =
    supported && engine && nameValid
      ? buildSql(engine, name.trim(), charset, collation)
      : null;
  const canRun = supported && nameValid && !running && sessionId !== null;
  const collations = collationsFor(charset, collation);

  // Default charset/collation from the database this session is connected to
  // (MySQL/MariaDB report both in `list_databases`).
  useEffect(() => {
    if (!sessionId || (engine !== "mysql" && engine !== "mariadb")) return;
    let cancelled = false;
    void api
      .listDatabases(sessionId)
      .then((list) => {
        if (cancelled) return;
        const current = list.find((d) => d.name === session?.database);
        if (!current?.charset) return;
        setCharset(current.charset);
        setCollation(
          current.collation ?? defaultCollation(current.charset),
        );
      })
      .catch(() => {
        /* defaults are fine when the catalog is not readable */
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, engine, session?.database]);

  const changeCharset = (next: string) => {
    setCharset(next);
    setCollation(defaultCollation(next));
  };

  const run = async () => {
    if (!sessionId || !engine || !nameValid) return;
    setRunning(true);
    setError(null);
    try {
      const res = await api.executeBatch(
        sessionId,
        [buildSql(engine, name.trim(), charset, collation)],
        false,
      );
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
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className={DLG_LABEL}>Character set</label>
              <select
                className={DLG_INPUT}
                value={charset}
                disabled={running}
                onChange={(e) => changeCharset(e.target.value)}
              >
                {CHARSETS.map((c) => (
                  <option key={c.charset} value={c.charset}>
                    {c.label ?? c.charset}
                  </option>
                ))}
                {!CHARSETS.some((c) => c.charset === charset) && (
                  <option value={charset}>{charset}</option>
                )}
              </select>
            </div>
            <div>
              <label className={DLG_LABEL}>Collation</label>
              <select
                className={DLG_INPUT}
                value={collation}
                disabled={running}
                onChange={(e) => setCollation(e.target.value)}
              >
                {collations.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
            </div>
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
