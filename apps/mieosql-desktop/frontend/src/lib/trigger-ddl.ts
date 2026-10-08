// Trigger helpers: parse the per-table marker from trigger comments and
// build DROP / template SQL for the engines whose triggers we can manage.

import type { Engine, Scope } from "./bindings";
import { quoteIdent, qualified } from "./quoting";

/**
 * The table a trigger belongs to, parsed from the catalog comment the drivers
 * attach (`ON users · BEFORE INSERT`, `ON users`, …).
 */
export function triggerTable(comment: string | null | undefined): string | null {
  const m = /^ON\s+([^\s·]+)/.exec(comment ?? "");
  return m?.[1] ?? null;
}

export function supportsTriggers(engine: Engine | null | undefined): boolean {
  return (
    engine === "mysql" ||
    engine === "mariadb" ||
    engine === "postgres" ||
    engine === "sqlite" ||
    engine === "sql_server"
  );
}

export function buildDropTrigger(
  engine: Engine,
  scope: Scope | null | undefined,
  table: string,
  name: string,
): string | null {
  switch (engine) {
    case "mysql":
    case "mariadb": {
      const trigger = scope?.database
        ? `${quoteIdent(engine, scope.database)}.${quoteIdent(engine, name)}`
        : quoteIdent(engine, name);
      return `DROP TRIGGER IF EXISTS ${trigger}`;
    }
    case "postgres":
      return `DROP TRIGGER IF EXISTS ${quoteIdent(engine, name)} ON ${qualified(engine, scope, table)}`;
    case "sqlite":
      return `DROP TRIGGER IF EXISTS ${quoteIdent(engine, name)}`;
    case "sql_server":
      return `DROP TRIGGER IF EXISTS ${qualified(engine, scope, name)}`;
    default:
      return null;
  }
}

/** Commented starting point for a new trigger, opened in a query tab. */
export function triggerTemplate(
  engine: Engine,
  scope: Scope | null | undefined,
  table: string,
): string | null {
  const t = qualified(engine, scope, table);
  switch (engine) {
    case "postgres":
      return [
        `-- New trigger on ${t} (PostgreSQL).`,
        "-- 1) Create (or replace) the trigger function, 2) attach it.",
        "CREATE OR REPLACE FUNCTION handle_change() RETURNS trigger AS $$",
        "BEGIN",
        "  -- ...",
        "  RETURN NEW;",
        "END;",
        "$$ LANGUAGE plpgsql;",
        "",
        `CREATE TRIGGER ${quoteIdent(engine, `${table}_trigger`)}`,
        `AFTER INSERT ON ${t}`,
        "FOR EACH ROW EXECUTE FUNCTION handle_change();",
      ].join("\n");
    case "mysql":
    case "mariadb":
      return [
        `-- New trigger on ${t} (MySQL/MariaDB).`,
        `CREATE TRIGGER ${quoteIdent(engine, `${table}_trigger`)}`,
        `AFTER INSERT ON ${t}`,
        "FOR EACH ROW",
        "BEGIN",
        "  -- ...",
        "END;",
      ].join("\n");
    case "sqlite":
      return [
        `-- New trigger on ${t} (SQLite).`,
        `CREATE TRIGGER IF NOT EXISTS ${quoteIdent(engine, `${table}_trigger`)}`,
        `AFTER INSERT ON ${t}`,
        "BEGIN",
        "  -- ...",
        "END;",
      ].join("\n");
    case "sql_server":
      return [
        `-- New trigger on ${t} (SQL Server).`,
        `CREATE TRIGGER ${quoteIdent(engine, `${table}_trigger`)} ON ${t}`,
        "AFTER INSERT",
        "AS",
        "BEGIN",
        "  -- ...",
        "END;",
      ].join("\n");
    default:
      return null;
  }
}
