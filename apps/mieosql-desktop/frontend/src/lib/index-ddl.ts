// Index DDL builders — mirror `crates/mieosql-api/src/sync.rs` so the UI
// emits exactly the statements structure-sync would.

import type { Engine, Scope } from "./bindings";
import { quoteIdent, qualified } from "./quoting";

export type IndexDraft = {
  name: string;
  columns: string[];
  unique: boolean;
};

/**
 * Engines whose indexes this editor manages. ClickHouse "indexes" are data
 * skipping indexes with a different syntax — not covered here.
 */
export function supportsIndexEditor(engine: Engine | null | undefined): boolean {
  return (
    engine === "mysql" ||
    engine === "mariadb" ||
    engine === "postgres" ||
    engine === "sqlite" ||
    engine === "sql_server"
  );
}

export function buildCreateIndex(
  engine: Engine,
  scope: Scope | null | undefined,
  table: string,
  draft: IndexDraft,
): string | null {
  if (!supportsIndexEditor(engine)) return null;
  const name = draft.name.trim();
  if (!name || draft.columns.length === 0) return null;

  const columns = draft.columns.map((c) => quoteIdent(engine, c)).join(", ");
  const unique = draft.unique ? "UNIQUE " : "";
  if (engine === "sqlite") {
    // SQLite rejects a schema-qualified table here; the index name carries
    // the database instead (`CREATE INDEX main.ix ON t (...)`).
    const indexName = scope?.database
      ? `${quoteIdent(engine, scope.database)}.${quoteIdent(engine, name)}`
      : quoteIdent(engine, name);
    return `CREATE ${unique}INDEX ${indexName} ON ${quoteIdent(engine, table)} (${columns})`;
  }
  return `CREATE ${unique}INDEX ${quoteIdent(engine, name)} ON ${qualified(engine, scope, table)} (${columns})`;
}

export function buildDropIndex(
  engine: Engine,
  scope: Scope | null | undefined,
  table: string,
  name: string,
): string | null {
  switch (engine) {
    case "mysql":
    case "mariadb":
      return `DROP INDEX ${quoteIdent(engine, name)} ON ${qualified(engine, scope, table)}`;
    case "postgres": {
      const index = scope?.schema
        ? `${quoteIdent(engine, scope.schema)}.${quoteIdent(engine, name)}`
        : quoteIdent(engine, name);
      return `DROP INDEX ${index}`;
    }
    case "sqlite": {
      const index = scope?.database
        ? `${quoteIdent(engine, scope.database)}.${quoteIdent(engine, name)}`
        : quoteIdent(engine, name);
      return `DROP INDEX ${index}`;
    }
    case "sql_server":
      // T-SQL wants the table here; `DROP INDEX [db].[name]` is invalid.
      return `DROP INDEX ${quoteIdent(engine, name)} ON ${qualified(engine, scope, table)}`;
    default:
      return null;
  }
}

/** Default index name suggestion: `ix_<table>_<column>`. */
export function suggestIndexName(table: string, columns: string[]): string {
  const clean = (s: string) => s.replace(/[^A-Za-z0-9_]/g, "_");
  const suffix = columns.length > 0 ? `_${clean(columns[0]!)}` : "";
  const short = table.length > 20 ? table.slice(0, 20) : table;
  return `ix_${clean(short)}${suffix}`;
}
