import { ENGINE_META } from "./engines";
import type { Engine, SslMode } from "./bindings";

export type ParsedConnection = {
  engine: Engine;
  host?: string;
  port?: number;
  username?: string;
  password?: string;
  database?: string;
  file?: string;
  params: Record<string, string>;
  sslMode?: SslMode;
};

const SCHEME_ENGINE: Record<string, Engine> = {
  postgres: "postgres",
  postgresql: "postgres",
  mysql: "mysql",
  mariadb: "mariadb",
  sqlserver: "sql_server",
  mssql: "sql_server",
  "microsoft-sql": "sql_server",
  mongodb: "mongodb",
  "mongodb+srv": "mongodb",
  redis: "redis",
  rediss: "redis",
  kafka: "kafka",
  sqlite: "sqlite",
  file: "sqlite",
};

function decode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function parsePort(raw: string, engine: Engine): number | undefined {
  if (!raw) return ENGINE_META[engine].defaultPort ?? undefined;
  const n = Number(raw);
  return Number.isFinite(n) && n > 0 && n <= 65535 ? Math.floor(n) : undefined;
}

function postgresSsl(value: string | null): SslMode | undefined {
  switch ((value ?? "").toLowerCase()) {
    case "disable":
      return "disable";
    case "allow":
    case "prefer":
      return "prefer";
    case "require":
      return "require";
    case "verify-ca":
      return "verify_ca";
    case "verify-full":
      return "verify_full";
    default:
      return undefined;
  }
}

function mysqlSsl(value: string | null): SslMode | undefined {
  switch ((value ?? "").toLowerCase()) {
    case "disabled":
      return "disable";
    case "preferred":
      return "prefer";
    case "required":
      return "require";
    case "verify_ca":
      return "verify_ca";
    case "verify_identity":
      return "verify_full";
    default:
      return undefined;
  }
}

/**
 * Parse `Server=host,port;Database=db;User Id=u;Password=p;Encrypt=true`
 * (ADO.NET / SqlClient style, common for SQL Server copy-paste).
 */
function parseAdoNet(input: string): ParsedConnection {
  const pairs = new Map<string, string>();
  for (const part of input.split(";")) {
    const eq = part.indexOf("=");
    if (eq < 0) continue;
    const key = part.slice(0, eq).trim().toLowerCase().replace(/\s+/g, "");
    const value = part.slice(eq + 1).trim();
    if (key && value) pairs.set(key, value);
  }
  const get = (...keys: string[]): string | undefined => {
    for (const k of keys) {
      const v = pairs.get(k);
      if (v) return v;
    }
    return undefined;
  };
  const server = get("server", "datasource", "addr", "address", "networkaddress") ?? "";
  // `host,port` or `host:port` or `host\instance` (instance kept as port-less host).
  let host = server;
  let port: number | undefined;
  const comma = server.lastIndexOf(",");
  const colon = server.lastIndexOf(":");
  const sep = Math.max(comma, colon);
  if (sep > 0 && sep < server.length - 1) {
    const maybePort = Number(server.slice(sep + 1));
    if (Number.isFinite(maybePort) && maybePort > 0) {
      host = server.slice(0, sep);
      port = Math.floor(maybePort);
    }
  }
  const encrypt = (get("encrypt") ?? "").toLowerCase();
  return {
    engine: "sql_server",
    host: host || undefined,
    port: port ?? ENGINE_META.sql_server.defaultPort ?? undefined,
    username: get("userid", "uid", "user", "username"),
    password: get("password", "pwd"),
    database: get("database", "initialcatalog", "dbname"),
    params: {},
    sslMode:
      encrypt === "true" || encrypt === "yes" || encrypt === "mandatory"
        ? "require"
        : encrypt === "false" || encrypt === "no"
          ? "disable"
          : undefined,
  };
}

/**
 * Parse a pasted connection string/URL into connection-form fields.
 * Supports URL style (`postgres://user:pass@host:port/db?...`,
 * `mysql://…`, `mongodb://…`, `redis://…`, `kafka://h1:9092,h2:9092…`,
 * `sqlite:///path.db`) and ADO.NET `key=value;…` style for SQL Server.
 * Throws with a human-readable message when the string is not understood.
 */
export function parseConnectionString(input: string): ParsedConnection {
  const text = input.trim();
  if (!text) throw new Error("Connection string is empty.");

  // ADO.NET style has no `://` but has `=` pairs.
  if (!text.includes("://")) {
    if (text.includes("=") && text.includes(";")) return parseAdoNet(text);
    // Bare SQLite path (`/data/app.db`, `C:\data\app.db`, `:memory:` …).
    if (
      text === ":memory:" ||
      /[/\\]/.test(text) ||
      /\.(db|db3|sqlite|sqlite3)\b/i.test(text)
    ) {
      return { engine: "sqlite", file: text, params: {} };
    }
    throw new Error(
      "Unrecognized format. Paste a URL like postgres://user:pass@host:5432/db, an SQLite file path, or SQL Server key=value;… pairs.",
    );
  }

  const scheme = text.slice(0, text.indexOf("://")).toLowerCase();
  const engine = SCHEME_ENGINE[scheme];
  if (!engine) {
    throw new Error(
      `Unsupported scheme "${scheme}". Use postgres, mysql, mariadb, sqlserver, mongodb, redis, kafka or sqlite.`,
    );
  }

  // SQLite file URL.
  if (engine === "sqlite") {
    const withoutScheme = text.slice(text.indexOf("://") + 3);
    const q = withoutScheme.indexOf("?");
    const file = decode(q >= 0 ? withoutScheme.slice(0, q) : withoutScheme);
    if (!file) throw new Error("SQLite URL has no file path (e.g. sqlite:///data/app.db).");
    // `sqlite:///:memory:` keeps the leading colon form.
    return { engine, file, params: {} };
  }

  // Kafka and MongoDB accept comma-separated host lists, which are not valid
  // URL authorities — normalize to the first host for parsing, then restore.
  let normalized = text;
  let hostList: string | undefined;
  let isSrv = false;
  if (engine === "kafka" || engine === "mongodb") {
    isSrv = scheme === "mongodb+srv";
    const m = text.match(/^([a-zA-Z][a-zA-Z0-9+.-]*:\/\/)([^/?#]*)([\/?#].*)?$/);
    if (m) {
      const authority = m[2] ?? "";
      // Strip `user:pass@` — credentials are parsed from the URL separately.
      const at = authority.lastIndexOf("@");
      hostList = at >= 0 ? authority.slice(at + 1) : authority;
      const first = hostList.split(",")[0]?.trim() ?? "";
      normalized = `${m[1]}${at >= 0 ? `${authority.slice(0, at + 1)}` : ""}${first}${m[3] ?? ""}`;
    }
  }

  let url: URL;
  try {
    url = new URL(normalized);
  } catch {
    throw new Error("Could not parse the connection string as a URL.");
  }

  const username = url.username ? decode(url.username) : undefined;
  const password = url.password ? decode(url.password) : undefined;
  // mongodb+srv resolves hosts via DNS — never pin a port for it.
  const port = isSrv ? undefined : parsePort(url.port, engine);

  // Database: first path segment (mongodb may carry options after `/db`).
  let database: string | undefined;
  const pathName = url.pathname.replace(/^\/+/, "");
  if (pathName && pathName !== "/") {
    database = decode(pathName.split("/")[0]!);
  }

  const params: Record<string, string> = {};
  let sslMode: SslMode | undefined;
  const query = url.searchParams;
  const consume = (fn: (key: string, value: string) => void) => {
    for (const [k, v] of query) fn(k, v);
  };
  if (engine === "postgres") {
    const mode = postgresSsl(query.get("sslmode"));
    if (mode) sslMode = mode;
    consume((k, v) => {
      if (k.toLowerCase() !== "sslmode") params[k] = v;
    });
  } else if (engine === "mysql" || engine === "mariadb") {
    const mode =
      mysqlSsl(query.get("ssl-mode") ?? query.get("sslMode") ?? query.get("sslmode"));
    if (mode) sslMode = mode;
    consume((k, v) => {
      const lk = k.toLowerCase();
      if (lk !== "ssl-mode" && lk !== "sslmode") params[k] = v;
    });
  } else if (engine === "mongodb") {
    const tls = (query.get("tls") ?? query.get("ssl") ?? "").toLowerCase();
    if (tls === "true") sslMode = "require";
    else if (tls === "false") sslMode = "disable";
    consume((k, v) => {
      params[k] = v;
    });
  } else if (engine === "redis") {
    if (scheme === "rediss") sslMode = "require";
    consume((k, v) => {
      params[k] = v;
    });
  } else if (engine === "kafka") {
    consume((k, v) => {
      params[k] = v;
    });
  } else if (engine === "sql_server") {
    const encrypt = (query.get("encrypt") ?? "").toLowerCase();
    if (encrypt === "true" || encrypt === "yes" || encrypt === "mandatory") {
      sslMode = "require";
    } else if (encrypt === "false" || encrypt === "no") {
      sslMode = "disable";
    }
    consume((k, v) => {
      if (k.toLowerCase() !== "encrypt") params[k] = v;
    });
  }

  // Restore the full host list for Kafka / MongoDB (single hosts stay clean —
  // drivers append the port themselves; multi-host lists keep their ports).
  let host = url.hostname || undefined;
  if ((engine === "kafka" || engine === "mongodb") && hostList) {
    host = hostList.includes(",") ? hostList : url.hostname || undefined;
  }

  return { engine, host, port, username, password, database, params, sslMode };
}
