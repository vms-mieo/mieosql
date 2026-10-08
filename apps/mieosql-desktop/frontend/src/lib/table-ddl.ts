// Engine-aware CREATE TABLE builder + common type lists for the table
// designer dialog. Kept dependency-free so it can be unit-tested and reused.

import type { Engine, Scope } from "./bindings";

export type ColumnSpec = {
  name: string;
  /** Raw SQL type as typed by the user, e.g. `VARCHAR(255)`. */
  type: string;
  notNull: boolean;
  primaryKey: boolean;
  /** Raw default expression; empty means "no DEFAULT clause". */
  defaultText: string;
  autoIncrement: boolean;
};

export type BuildResult = {
  sql: string;
  warnings: string[];
};

/** Engines whose drivers support SQL DDL in this app. */
export const SQL_ENGINES: readonly Engine[] = [
  "mysql",
  "mariadb",
  "postgres",
  "sqlite",
  "sql_server",
];

export function supportsCreateTable(engine: Engine | null | undefined): boolean {
  return engine != null && SQL_ENGINES.includes(engine);
}

/** Identifier names accepted by the dialogs (no spaces, quotes or dots). */
export function isValidObjectName(name: string): boolean {
  return /^[A-Za-z0-9_$]+$/.test(name.trim());
}

/** Quote an identifier using the engine's convention. */
export function quoteIdent(engine: Engine | null | undefined, name: string): string {
  switch (engine) {
    case "mysql":
    case "mariadb":
      return `\`${name.replace(/`/g, "``")}\``;
    case "sql_server":
      return `[${name.replace(/\]/g, "]]")}]`;
    default:
      // PostgreSQL, SQLite (and unknown engines, for previews only).
      return `"${name.replace(/"/g, '""')}"`;
  }
}

/** Qualify a table name with the tree scope, when the engine supports it. */
export function qualifiedName(
  engine: Engine | null | undefined,
  table: string,
  scope?: Scope,
): string {
  const db = scope?.database?.trim();
  const schema = scope?.schema?.trim();
  switch (engine) {
    case "mysql":
    case "mariadb":
      return db ? `${quoteIdent(engine, db)}.${quoteIdent(engine, table)}` : quoteIdent(engine, table);
    case "postgres":
      return schema
        ? `${quoteIdent(engine, schema)}.${quoteIdent(engine, table)}`
        : quoteIdent(engine, table);
    case "sql_server": {
      const parts: string[] = [];
      if (db) parts.push(quoteIdent(engine, db));
      if (schema) parts.push(quoteIdent(engine, schema));
      parts.push(quoteIdent(engine, table));
      return parts.join(".");
    }
    default:
      // SQLite: the database in scope is the connection's file/db name — a
      // bare table name is the correct target.
      return quoteIdent(engine, table);
  }
}

/** Common column types offered as a datalist for the given engine. */
export function commonTypes(engine: Engine | null | undefined): readonly string[] {
  switch (engine) {
    case "mysql":
    case "mariadb":
      return [
        "INT",
        "BIGINT",
        "VARCHAR(255)",
        "TEXT",
        "DECIMAL(10,2)",
        "DATETIME",
        "TIMESTAMP",
        "JSON",
        "BOOLEAN",
      ];
    case "postgres":
      return [
        "serial",
        "bigserial",
        "INTEGER",
        "BIGINT",
        "VARCHAR(n)",
        "TEXT",
        "NUMERIC(12,2)",
        "TIMESTAMPTZ",
        "JSONB",
        "BOOLEAN",
        "UUID",
      ];
    case "sqlite":
      return ["INTEGER", "TEXT", "REAL", "BLOB", "NUMERIC"];
    case "sql_server":
      return [
        "INT",
        "BIGINT",
        "NVARCHAR(255)",
        "NVARCHAR(MAX)",
        "DECIMAL(18,2)",
        "DATETIME2",
        "BIT",
        "UNIQUEIDENTIFIER",
      ];
    default:
      return [];
  }
}

function columnDef(engine: Engine, col: ColumnSpec, inlinePk: boolean): string {
  const q = quoteIdent(engine, col.name.trim());
  const type = col.type.trim();
  const notNull = col.notNull ? " NOT NULL" : "";
  const def = col.defaultText.trim() ? ` DEFAULT ${col.defaultText.trim()}` : "";
  const pk = inlinePk ? " PRIMARY KEY" : "";

  switch (engine) {
    case "sqlite": {
      const auto = col.autoIncrement && inlinePk && col.primaryKey && /^integer$/i.test(type);
      if (auto) return `${q} ${type} PRIMARY KEY AUTOINCREMENT`;
      return `${q} ${type}${notNull}${def}${pk}`;
    }
    case "mysql":
    case "mariadb": {
      const auto = col.autoIncrement ? " AUTO_INCREMENT" : "";
      // MySQL column grammar: … [NOT NULL] [DEFAULT …] [AUTO_INCREMENT] [PRIMARY KEY].
      return `${q} ${type}${notNull}${def}${auto}${pk}`;
    }
    case "sql_server": {
      // T-SQL column grammar: … [IDENTITY(1,1)] [NOT NULL] [DEFAULT …].
      const auto = col.autoIncrement ? " IDENTITY(1,1)" : "";
      return `${q} ${type}${auto}${notNull}${def}${pk}`;
    }
    default:
      // PostgreSQL: auto-increment is expressed through the chosen type
      // (serial / bigserial / GENERATED … AS IDENTITY), never a keyword.
      return `${q} ${type}${notNull}${def}${pk}`;
  }
}

/**
 * Build a `CREATE TABLE` statement (without trailing semicolon).
 * Returns human-readable warnings for ignored/invalid flags.
 */
export function buildCreateTable(
  engine: Engine,
  table: string,
  columns: ColumnSpec[],
  scope?: Scope,
): BuildResult {
  const warnings: string[] = [];
  if (!supportsCreateTable(engine)) {
    warnings.push(`CREATE TABLE is not supported for “${engine}” connections.`);
  }

  const pkCols = columns.filter((c) => c.primaryKey);
  const inlinePk = pkCols.length === 1;
  const autoCols = columns.filter((c) => c.autoIncrement);

  if (engine === "postgres" && autoCols.length > 0) {
    warnings.push(
      'PostgreSQL has no AUTO_INCREMENT keyword — pick "serial"/"bigserial" as the type instead.',
    );
  }
  if ((engine === "mysql" || engine === "mariadb") && autoCols.length > 1) {
    warnings.push("MySQL allows only one AUTO_INCREMENT column per table.");
  }
  for (const col of autoCols) {
    if ((engine === "mysql" || engine === "mariadb") && !col.primaryKey) {
      warnings.push(
        `“${col.name.trim()}” is AUTO_INCREMENT but not a primary key — MySQL requires it to be a key.`,
      );
    }
    if (engine === "sqlite") {
      if (!col.primaryKey || !/^integer$/i.test(col.type.trim())) {
        warnings.push(
          `“${col.name.trim()}”: SQLite AUTOINCREMENT requires INTEGER PRIMARY KEY — ignored.`,
        );
      } else if (!inlinePk) {
        warnings.push(
          `“${col.name.trim()}”: SQLite AUTOINCREMENT needs a single-column primary key — ignored.`,
        );
      }
    }
  }

  const defs = columns.map((col) => columnDef(engine, col, inlinePk && col.primaryKey));
  if (pkCols.length > 1) {
    defs.push(
      `PRIMARY KEY (${pkCols.map((c) => quoteIdent(engine, c.name.trim())).join(", ")})`,
    );
  }

  const sql = `CREATE TABLE ${qualifiedName(engine, table, scope)} (\n  ${defs.join(",\n  ")}\n)`;
  return { sql, warnings };
}

// ------------------------------------------------------------------ ALTER --

export type AlterResult = {
  /** One `ALTER TABLE …` statement per entry (without trailing semicolons). */
  sql: string[];
  /** Human-readable notes: skipped operations, destructive heads-ups, … */
  warnings: string[];
  /** Subset of `warnings` describing irreversible data loss (column drops). */
  destructive: string[];
};

function normalizeTypeText(value: string): string {
  return value.trim().replace(/\s+/g, " ").toLowerCase();
}

function defaultClause(defaultText: string): string {
  const expr = defaultText.trim();
  return expr ? ` DEFAULT ${expr}` : "";
}

/** Full column tail for MySQL `ADD`/`MODIFY`/`CHANGE` (never touches PK). */
function mysqlColumnTail(col: ColumnSpec): string {
  const type = col.type.trim();
  const notNull = col.notNull ? " NOT NULL" : "";
  const auto = col.autoIncrement ? " AUTO_INCREMENT" : "";
  return `${type}${notNull}${defaultClause(col.defaultText)}${auto}`;
}

/** Nullability fragment for T-SQL `ALTER COLUMN` (restating it is required). */
function sqlServerNullability(notNull: boolean): string {
  return notNull ? " NOT NULL" : " NULL";
}

/**
 * Build an `ALTER TABLE … RENAME COLUMN` statement for an explicit
 * old→new pair. `col` is the NEW spec (target name + desired attributes;
 * only MySQL needs the attributes). Returns `sql: null` with a warning for
 * engines without a safe single-statement rename (SQL Server needs
 * `sp_rename`, which this dialog avoids).
 */
export function buildRenameColumn(
  engine: Engine,
  table: string,
  scope: Scope | undefined,
  from: string,
  col: ColumnSpec,
): { sql: string | null; warning: string | null } {
  const target = qualifiedName(engine, table, scope);
  const oldName = quoteIdent(engine, from);
  const newName = quoteIdent(engine, col.name.trim());
  switch (engine) {
    case "mysql":
    case "mariadb":
      return {
        sql: `ALTER TABLE ${target} CHANGE COLUMN ${oldName} ${newName} ${mysqlColumnTail(col)}`,
        warning: null,
      };
    case "postgres":
    case "sqlite":
      return {
        sql: `ALTER TABLE ${target} RENAME COLUMN ${oldName} TO ${newName}`,
        warning: null,
      };
    case "sql_server":
      return {
        sql: null,
        warning: `Renaming “${from}” is not supported for SQL Server here (it needs sp_rename) — skipped. Run it in the SQL editor instead.`,
      };
    default:
      return {
        sql: null,
        warning: `Renaming “${from}” is not supported for “${engine}” connections — skipped.`,
      };
  }
}

/**
 * Diff two column lists into `ALTER TABLE` statements (one per entry).
 *
 * Columns are matched by exact, case-sensitive name: names only in `newCols`
 * become `ADD COLUMN`, names only in `oldCols` become `DROP COLUMN`
 * (destructive), and shared names are compared attribute-by-attribute
 * (type, nullability, default, auto-increment) with per-engine syntax.
 * Renames are NOT detected here — the caller handles explicit old→new pairs
 * with {@link buildRenameColumn} (passing the OLD name through in `newCols`
 * so attribute changes still diff against the right column).
 *
 * Primary key membership is never touched: PK differences only produce
 * warnings, added columns are emitted without `PRIMARY KEY`, and dropping a
 * PK column is skipped.
 */
export function buildAlterTable(
  engine: Engine,
  table: string,
  scope: Scope | undefined,
  oldCols: ColumnSpec[],
  newCols: ColumnSpec[],
): AlterResult {
  const sql: string[] = [];
  const warnings: string[] = [];
  const destructive: string[] = [];

  if (!supportsCreateTable(engine)) {
    warnings.push(`ALTER TABLE is not supported for “${engine}” connections.`);
    return { sql, warnings, destructive };
  }

  const target = qualifiedName(engine, table, scope);
  const oldByName = new Map(oldCols.map((c) => [c.name.trim(), c]));
  const newByName = new Map(newCols.map((c) => [c.name.trim(), c]));
  const oldPk = new Set(
    oldCols.filter((c) => c.primaryKey).map((c) => c.name.trim()),
  );
  const newPk = new Set(
    newCols.filter((c) => c.primaryKey).map((c) => c.name.trim()),
  );
  const samePk =
    oldPk.size === newPk.size && [...oldPk].every((n) => newPk.has(n));
  if (!samePk) {
    warnings.push(
      "Primary key changes are not applied by this dialog — add or drop keys in the SQL editor instead.",
    );
  }

  // ---------------------------------------------------------- added columns
  for (const col of newCols) {
    const name = col.name.trim();
    if (oldByName.has(name)) continue;
    const q = quoteIdent(engine, name);
    const type = col.type.trim();
    if (col.primaryKey) {
      warnings.push(
        `“${name}” was added without PRIMARY KEY — primary key changes are not applied by this dialog.`,
      );
    }
    switch (engine) {
      case "mysql":
      case "mariadb": {
        if (col.autoIncrement && !col.primaryKey) {
          warnings.push(
            `“${name}” is AUTO_INCREMENT but not a primary key — MySQL requires it to be a key.`,
          );
        }
        sql.push(`ALTER TABLE ${target} ADD COLUMN ${q} ${mysqlColumnTail(col)}`);
        break;
      }
      case "postgres": {
        if (col.autoIncrement) {
          warnings.push(
            `“${name}”: PostgreSQL has no AUTO_INCREMENT keyword — pick "serial"/"bigserial" as the type instead.`,
          );
        }
        sql.push(
          `ALTER TABLE ${target} ADD COLUMN ${q} ${type}${col.notNull ? " NOT NULL" : ""}${defaultClause(col.defaultText)}`,
        );
        break;
      }
      case "sqlite": {
        if (col.autoIncrement) {
          warnings.push(
            `“${name}”: SQLite AUTOINCREMENT requires INTEGER PRIMARY KEY — ignored.`,
          );
        }
        if (col.notNull && !col.defaultText.trim()) {
          warnings.push(
            `“${name}”: SQLite cannot ADD a NOT NULL column without a non-null DEFAULT — the statement may fail.`,
          );
        }
        sql.push(
          `ALTER TABLE ${target} ADD COLUMN ${q} ${type}${col.notNull ? " NOT NULL" : ""}${defaultClause(col.defaultText)}`,
        );
        break;
      }
      case "sql_server": {
        if (col.autoIncrement) {
          warnings.push(
            `“${name}”: SQL Server IDENTITY cannot be added to an existing column — ignored.`,
          );
        }
        sql.push(
          `ALTER TABLE ${target} ADD ${q} ${type}${sqlServerNullability(col.notNull)}${defaultClause(col.defaultText)}`,
        );
        break;
      }
      default:
        break;
    }
  }

  // -------------------------------------------------------- changed columns
  for (const col of newCols) {
    const name = col.name.trim();
    const old = oldByName.get(name);
    if (!old) continue;
    if (old.primaryKey !== col.primaryKey) {
      warnings.push(
        `Primary key flag on “${name}” changed — primary key changes are not applied by this dialog.`,
      );
    }
    const typeChanged =
      normalizeTypeText(old.type) !== normalizeTypeText(col.type);
    const nullChanged = old.notNull !== col.notNull;
    const defaultChanged = old.defaultText.trim() !== col.defaultText.trim();
    const autoChanged = old.autoIncrement !== col.autoIncrement;
    if (!typeChanged && !nullChanged && !defaultChanged && !autoChanged) {
      continue;
    }
    const q = quoteIdent(engine, name);

    if (engine === "mysql" || engine === "mariadb") {
      // MySQL has no per-attribute ALTERs — one MODIFY restates the column.
      if (col.autoIncrement && !col.primaryKey) {
        warnings.push(
          `“${name}” is AUTO_INCREMENT but not a primary key — MySQL requires it to be a key.`,
        );
      }
      sql.push(`ALTER TABLE ${target} MODIFY COLUMN ${q} ${mysqlColumnTail(col)}`);
      continue;
    }
    if (engine === "postgres") {
      if (typeChanged) {
        sql.push(`ALTER TABLE ${target} ALTER COLUMN ${q} TYPE ${col.type.trim()}`);
        warnings.push(
          `Changing the type of “${name}” may fail if existing values cannot be cast — add a USING clause in the SQL editor when needed.`,
        );
      }
      if (nullChanged) {
        sql.push(
          `ALTER TABLE ${target} ALTER COLUMN ${q} ${col.notNull ? "SET NOT NULL" : "DROP NOT NULL"}`,
        );
      }
      if (defaultChanged) {
        const expr = col.defaultText.trim();
        sql.push(
          expr
            ? `ALTER TABLE ${target} ALTER COLUMN ${q} SET DEFAULT ${expr}`
            : `ALTER TABLE ${target} ALTER COLUMN ${q} DROP DEFAULT`,
        );
      }
      if (autoChanged) {
        warnings.push(
          `“${name}”: PostgreSQL identity/serial cannot be toggled with a plain ALTER — skipped. Change it in the SQL editor instead.`,
        );
      }
      continue;
    }
    if (engine === "sqlite") {
      // SQLite needs a table rebuild for all of these — out of scope here.
      if (typeChanged) {
        warnings.push(
          `“${name}”: SQLite cannot change a column type without rebuilding the table — skipped (table rebuild is TODO).`,
        );
      }
      if (nullChanged) {
        warnings.push(
          `“${name}”: SQLite cannot change NOT NULL without rebuilding the table — skipped (table rebuild is TODO).`,
        );
      }
      if (defaultChanged) {
        warnings.push(
          `“${name}”: SQLite cannot change a column DEFAULT without rebuilding the table — skipped (table rebuild is TODO).`,
        );
      }
      if (autoChanged) {
        warnings.push(
          `“${name}”: SQLite AUTOINCREMENT cannot be toggled without rebuilding the table — skipped (table rebuild is TODO).`,
        );
      }
      continue;
    }
    if (engine === "sql_server") {
      // T-SQL restates type + nullability together; defaults live in named
      // constraints, so DEFAULT changes are left to the SQL editor.
      if (typeChanged || nullChanged) {
        sql.push(
          `ALTER TABLE ${target} ALTER COLUMN ${q} ${col.type.trim()}${sqlServerNullability(col.notNull)}`,
        );
      }
      if (defaultChanged) {
        warnings.push(
          `“${name}”: SQL Server defaults live in named constraints — change the DEFAULT in the SQL editor instead.`,
        );
      }
      if (autoChanged) {
        warnings.push(
          `“${name}”: SQL Server IDENTITY cannot be toggled on an existing column — skipped.`,
        );
      }
      continue;
    }
  }

  // -------------------------------------------------------- dropped columns
  for (const col of oldCols) {
    const name = col.name.trim();
    if (newByName.has(name)) continue;
    if (oldPk.has(name)) {
      warnings.push(
        `“${name}” is part of the primary key — the DROP was skipped. Drop the key in the SQL editor first.`,
      );
      continue;
    }
    sql.push(`ALTER TABLE ${target} DROP COLUMN ${quoteIdent(engine, name)}`);
    const msg = `Dropping column “${name}” permanently deletes its data.`;
    warnings.push(msg);
    destructive.push(msg);
  }

  return { sql, warnings, destructive };
}
