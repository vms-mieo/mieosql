// Engine-aware SQL identifier quoting and small DDL builders used by the
// connection-tree context menu (drop / rename / truncate) and table export.
// Mirrors the quoting rules of the Rust backend (`mieosql-api::service`).

import type { Engine, Scope } from "./bindings";

/** Engines that speak SQL and support quoting, DDL and SELECT previews. */
export function isSqlEngine(engine: Engine): boolean {
  return (
    engine === "mysql" ||
    engine === "mariadb" ||
    engine === "postgres" ||
    engine === "sqlite" ||
    engine === "sql_server"
  );
}

/** Quote one identifier according to the engine's rules. */
export function quoteIdent(engine: Engine, name: string): string {
  switch (engine) {
    case "mysql":
    case "mariadb":
      return `\`${name.replace(/`/g, "``")}\``;
    case "sql_server":
      return `[${name.replace(/]/g, "]]")}]`;
    default:
      return `"${name.replace(/"/g, '""')}"`;
  }
}

/**
 * Qualify an object name the same way the backend does: PostgreSQL and SQL
 * Server use `schema.table` (a connection is pinned to one database), every
 * other SQL engine uses `database.table`.
 */
export function qualified(
  engine: Engine,
  scope: Scope | null | undefined,
  name: string,
): string {
  const table = quoteIdent(engine, name);
  if (engine === "postgres" || engine === "sql_server") {
    const schema = scope?.schema;
    return schema ? `${quoteIdent(engine, schema)}.${table}` : table;
  }
  const database = scope?.database;
  return database ? `${quoteIdent(engine, database)}.${table}` : table;
}

/** `DROP DATABASE` for engines that support it; null when unsupported. */
export function dropDatabaseSql(engine: Engine, name: string): string | null {
  switch (engine) {
    case "mysql":
    case "mariadb":
    case "postgres":
    case "sql_server":
      return `DROP DATABASE ${quoteIdent(engine, name)}`;
    default:
      return null;
  }
}

/**
 * Whether table rename is supported. SQL Server needs `sp_rename` with a
 * `schema.name` string argument — skipped rather than risking a bad statement.
 */
export function canRenameTable(engine: Engine): boolean {
  return (
    engine === "mysql" ||
    engine === "mariadb" ||
    engine === "postgres" ||
    engine === "sqlite"
  );
}

/** Engine-aware `RENAME` statement; null when unsupported. */
export function renameTableSql(
  engine: Engine,
  scope: Scope,
  from: string,
  to: string,
): string | null {
  const newName = quoteIdent(engine, to);
  switch (engine) {
    case "mysql":
    case "mariadb":
      return `RENAME TABLE ${qualified(engine, scope, from)} TO ${newName}`;
    case "postgres":
    case "sqlite":
      return `ALTER TABLE ${qualified(engine, scope, from)} RENAME TO ${newName}`;
    default:
      return null;
  }
}

/** `TRUNCATE TABLE`, or `DELETE FROM` where the engine lacks/forces it. */
export function truncateTableSql(
  engine: Engine,
  scope: Scope,
  table: string,
): string | null {
  switch (engine) {
    case "mysql":
    case "mariadb":
    case "postgres":
      return `TRUNCATE TABLE ${qualified(engine, scope, table)}`;
    case "sqlite":
    case "sql_server":
      return `DELETE FROM ${qualified(engine, scope, table)}`;
    default:
      return null;
  }
}

/** `DROP TABLE` for SQL engines; null otherwise. */
export function dropTableSql(
  engine: Engine,
  scope: Scope,
  table: string,
): string | null {
  return isSqlEngine(engine)
    ? `DROP TABLE ${qualified(engine, scope, table)}`
    : null;
}

/** `DROP VIEW` (or materialized view) for SQL engines; null otherwise. */
export function dropViewSql(
  engine: Engine,
  scope: Scope,
  name: string,
  materialized = false,
): string | null {
  if (!isSqlEngine(engine)) return null;
  if (materialized && (engine === "mysql" || engine === "mariadb" || engine === "sqlite" || engine === "sql_server")) {
    // These engines have no materialized views — fall back to plain DROP VIEW
    // so the menu still offers a destructive action instead of nothing.
    return `DROP VIEW ${qualified(engine, scope, name)}`;
  }
  const kind = materialized ? "MATERIALIZED VIEW" : "VIEW";
  return `DROP ${kind} ${qualified(engine, scope, name)}`;
}

/** `DROP SCHEMA`; null when the engine has no schemas or unsupported. */
export function dropSchemaSql(engine: Engine, schema: string): string | null {
  switch (engine) {
    case "postgres":
    case "sql_server":
      return `DROP SCHEMA ${quoteIdent(engine, schema)}`;
    default:
      return null;
  }
}

/** `SELECT * FROM t LIMIT n` preview used by Generate SQL. */
export function selectStarSql(
  engine: Engine,
  scope: Scope,
  table: string,
  limit = 100,
): string | null {
  if (!isSqlEngine(engine)) return null;
  return `SELECT * FROM ${qualified(engine, scope, table)} LIMIT ${limit};`;
}

/** `DROP FUNCTION` / `DROP PROCEDURE` / `DROP TRIGGER`; null when unsupported. */
export function dropRoutineSql(
  engine: Engine,
  scope: Scope,
  name: string,
  kind: "function" | "procedure" | "trigger",
): string | null {
  if (!isSqlEngine(engine)) return null;
  if (kind === "trigger" && engine === "postgres") {
    // Postgres needs `DROP TRIGGER name ON table` — the tree node does not
    // carry its table, so offer no statement rather than a broken one.
    return null;
  }
  const keyword =
    kind === "function" ? "FUNCTION" : kind === "procedure" ? "PROCEDURE" : "TRIGGER";
  return `DROP ${keyword} ${qualified(engine, scope, name)}`;
}

/** `SELECT COUNT(*) FROM t` used by Row Count. */
export function countSql(
  engine: Engine,
  scope: Scope,
  table: string,
): string | null {
  if (!isSqlEngine(engine)) return null;
  return `SELECT COUNT(*) AS row_count FROM ${qualified(engine, scope, table)};`;
}

/**
 * Duplicate-table statement (`CREATE TABLE new AS SELECT *` / `SELECT INTO`).
 * Returns null for non-SQL engines or SQL Server rename-style edge cases.
 */
export function duplicateTableSql(
  engine: Engine,
  scope: Scope,
  from: string,
  to: string,
): string | null {
  if (!isSqlEngine(engine)) return null;
  const src = qualified(engine, scope, from);
  if (engine === "sql_server") {
    // T-SQL: SELECT * INTO new FROM old (same schema when qualified).
    const dst = qualified(engine, scope, to);
    return `SELECT * INTO ${dst} FROM ${src};`;
  }
  const dst = qualified(engine, scope, to);
  return `CREATE TABLE ${dst} AS SELECT * FROM ${src};`;
}
