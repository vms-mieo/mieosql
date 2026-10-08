// Code Insight: schema-aware completion + hover tooltips for the SQL editor.
//
// Schema data is fetched through `api.schemaSnapshot` and cached per
// (sessionId + database/schema) for 5 minutes. All insight features degrade
// gracefully: when the snapshot fails, completion still offers keywords and
// snippets, and hover falls back to a short keyword description.

import {
  autocompletion,
  snippetCompletion,
  type Completion,
  type CompletionSource,
} from "@codemirror/autocomplete";
import type { Extension } from "@codemirror/state";
import { hoverTooltip, type Tooltip } from "@codemirror/view";

import { api } from "./api";
import type {
  ColumnDef,
  Engine,
  ForeignKeyDef,
  ObjectKind,
  Scope,
  TableSnapshot,
} from "./bindings";
import { identifierAt, isCodePosition, scanSqlSegments, type SqlWord } from "./sql-format";

// ---------------------------------------------------------------------------
// Schema cache
// ---------------------------------------------------------------------------

export type InsightTable = {
  name: string;
  kind: ObjectKind;
  comment?: string;
  rowsEstimate?: number | null;
  columns: ColumnDef[];
  foreignKeys: ForeignKeyDef[];
};

export type InsightSchema = {
  list: InsightTable[];
  /** keyed by lower-cased table name */
  tables: Map<string, InsightTable>;
  generatedAt: string;
  truncated: boolean;
};

const SCHEMA_TTL_MS = 5 * 60 * 1000;
const FAILURE_TTL_MS = 20 * 1000;

type CacheEntry = {
  at: number;
  schema: InsightSchema | null;
  inflight: Promise<InsightSchema | null> | null;
};

const cache = new Map<string, CacheEntry>();

function cacheKey(sessionId: string, scope: Scope | null | undefined): string {
  return [sessionId, scope?.database ?? "", scope?.schema ?? ""].join("\u0000");
}

function buildSchema(snapshot: {
  tables: TableSnapshot[];
  generated_at: string;
  truncated: boolean;
}): InsightSchema {
  const list: InsightTable[] = [];
  const tables = new Map<string, InsightTable>();
  for (const t of snapshot.tables) {
    const table: InsightTable = {
      name: t.name,
      kind: t.kind,
      comment: t.comment,
      rowsEstimate: t.rows_estimate,
      columns: t.columns,
      foreignKeys: t.foreign_keys,
    };
    list.push(table);
    tables.set(t.name.toLowerCase(), table);
  }
  return { list, tables, generatedAt: snapshot.generated_at, truncated: snapshot.truncated };
}

async function loadSchema(
  sessionId: string,
  scope: Scope | null | undefined,
): Promise<InsightSchema | null> {
  try {
    const snapshot = await api.schemaSnapshot(sessionId, scope ?? {}, false, 200);
    return buildSchema(snapshot);
  } catch {
    return null;
  }
}

/** Cached schema for the scope, even when slightly stale (null when never fetched). */
export function peekSchema(
  sessionId: string,
  scope: Scope | null | undefined,
): InsightSchema | null {
  return cache.get(cacheKey(sessionId, scope))?.schema ?? null;
}

function ensureLoaded(
  sessionId: string,
  scope: Scope | null | undefined,
): Promise<InsightSchema | null> {
  const key = cacheKey(sessionId, scope);
  const now = Date.now();
  const entry = cache.get(key);
  if (entry?.inflight) {
    return entry.schema ? Promise.resolve(entry.schema) : entry.inflight;
  }
  if (entry?.schema && now - entry.at < SCHEMA_TTL_MS) {
    return Promise.resolve(entry.schema);
  }
  if (!entry?.schema && entry && now - entry.at < FAILURE_TTL_MS) {
    return Promise.resolve(null);
  }
  const promise = loadSchema(sessionId, scope);
  cache.set(key, { at: entry?.at ?? now, schema: entry?.schema ?? null, inflight: promise });
  promise.then((schema) => {
    const current = cache.get(key);
    if (current?.inflight === promise) {
      cache.set(key, { at: Date.now(), schema, inflight: null });
    }
  });
  // Serve a stale schema immediately; the refresh continues in the background.
  return entry?.schema ? Promise.resolve(entry.schema) : promise;
}

/** Load (or refresh) the schema snapshot for a session scope. Never throws. */
export function getSchema(
  sessionId: string,
  scope: Scope | null | undefined,
): Promise<InsightSchema | null> {
  return ensureLoaded(sessionId, scope);
}

/** Drop cached snapshots (optionally only for one session). */
export function invalidateSchema(sessionId?: string): void {
  if (!sessionId) {
    cache.clear();
    return;
  }
  for (const key of cache.keys()) {
    if (key.startsWith(`${sessionId}\u0000`)) cache.delete(key);
  }
}

// ---------------------------------------------------------------------------
// Keywords & snippets
// ---------------------------------------------------------------------------

const BASE_KEYWORDS: string[] = [
  "SELECT",
  "FROM",
  "WHERE",
  "AND",
  "OR",
  "NOT",
  "NULL",
  "IS",
  "IN",
  "EXISTS",
  "BETWEEN",
  "LIKE",
  "CASE",
  "WHEN",
  "THEN",
  "ELSE",
  "END",
  "JOIN",
  "INNER",
  "LEFT",
  "RIGHT",
  "FULL",
  "OUTER",
  "CROSS",
  "ON",
  "USING",
  "GROUP",
  "BY",
  "ORDER",
  "HAVING",
  "LIMIT",
  "OFFSET",
  "UNION",
  "ALL",
  "DISTINCT",
  "AS",
  "ASC",
  "DESC",
  "INSERT",
  "INTO",
  "VALUES",
  "UPDATE",
  "SET",
  "DELETE",
  "CREATE",
  "TABLE",
  "VIEW",
  "INDEX",
  "ALTER",
  "DROP",
  "ADD",
  "COLUMN",
  "PRIMARY",
  "KEY",
  "FOREIGN",
  "REFERENCES",
  "UNIQUE",
  "DEFAULT",
  "CONSTRAINT",
  "TRUNCATE",
  "BEGIN",
  "COMMIT",
  "ROLLBACK",
  "TRANSACTION",
  "WITH",
  "RECURSIVE",
  "RETURNING",
  "CAST",
  "COUNT",
  "SUM",
  "AVG",
  "MIN",
  "MAX",
  "COALESCE",
  "NULLIF",
  "CURRENT_DATE",
  "CURRENT_TIMESTAMP",
  "NOW",
  "EXPLAIN",
  "ANALYZE",
  "OVER",
  "PARTITION",
  "WINDOW",
];

const ENGINE_KEYWORDS: Partial<Record<Engine, string[]>> = {
  mysql: [
    "AUTO_INCREMENT",
    "ENGINE",
    "IFNULL",
    "GROUP_CONCAT",
    "REPLACE",
    "DUPLICATE",
    "IGNORE",
    "STRAIGHT_JOIN",
    "SHOW",
    "USE",
  ],
  mariadb: [
    "AUTO_INCREMENT",
    "ENGINE",
    "IFNULL",
    "GROUP_CONCAT",
    "REPLACE",
    "DUPLICATE",
    "IGNORE",
    "STRAIGHT_JOIN",
    "SHOW",
    "USE",
  ],
  postgres: [
    "ILIKE",
    "SERIAL",
    "BIGSERIAL",
    "JSONB",
    "GENERATED",
    "IDENTITY",
    "CONFLICT",
    "DO",
    "NOTHING",
    "LATERAL",
    "VACUUM",
  ],
  sqlite: [
    "PRAGMA",
    "AUTOINCREMENT",
    "ROWID",
    "GLOB",
    "VACUUM",
    "ATTACH",
    "WITHOUT",
    "REPLACE",
  ],
};

function keywordsFor(engine: Engine | null): string[] {
  const extra = engine ? ENGINE_KEYWORDS[engine] ?? [] : [];
  return extra.length > 0 ? [...BASE_KEYWORDS, ...extra] : BASE_KEYWORDS;
}

const RESERVED_WORDS = new Set<string>([
  ...BASE_KEYWORDS,
  ...Object.values(ENGINE_KEYWORDS).flat(),
  "WHERE",
  "ON",
  "SET",
  "VALUES",
  "GROUP",
  "ORDER",
  "HAVING",
  "LIMIT",
  "UNION",
  "JOIN",
  "INNER",
  "LEFT",
  "RIGHT",
  "FULL",
  "OUTER",
  "CROSS",
  "NATURAL",
  "AS",
  "SELECT",
  "FROM",
  "INSERT",
  "UPDATE",
  "DELETE",
  "INTO",
  "USING",
  "WHEN",
  "THEN",
  "ELSE",
  "END",
  "AND",
  "OR",
  "NOT",
  "NULL",
  "IS",
  "IN",
  "LIKE",
  "BETWEEN",
  "RETURNING",
  "WINDOW",
  "OFFSET",
  "BY",
  "ASC",
  "DESC",
  "CASE",
  "EXISTS",
  "DISTINCT",
  "ALL",
  "ANY",
  "SOME",
  "PRAGMA",
  "WITH",
  "RECURSIVE",
  "LATERAL",
  "STRAIGHT_JOIN",
  "FORCE",
  "USE",
  "IGNORE",
  "CONFLICT",
  "DO",
  "NOTHING",
]);

type SnippetDef = { label: string; detail: string; template: string };

const SNIPPETS: SnippetDef[] = [
  {
    label: "sel",
    detail: "SELECT … FROM …",
    template:
      "SELECT ${1:columns}\nFROM ${2:table}\nWHERE ${3:condition}\nLIMIT ${4:100}${0}",
  },
  {
    label: "ins",
    detail: "INSERT INTO … VALUES",
    template: "INSERT INTO ${1:table} (${2:columns})\nVALUES (${3:values})${0}",
  },
  {
    label: "upd",
    detail: "UPDATE … SET … WHERE",
    template:
      "UPDATE ${1:table}\nSET ${2:column} = ${3:value}\nWHERE ${4:condition}${0}",
  },
  {
    label: "del",
    detail: "DELETE FROM … WHERE",
    template: "DELETE FROM ${1:table}\nWHERE ${2:condition}${0}",
  },
  {
    label: "join",
    detail: "SELECT … JOIN … ON",
    template:
      "SELECT ${1:columns}\nFROM ${2:left_table} AS l\nJOIN ${3:right_table} AS r\n  ON ${4:l.id = r.id}${0}",
  },
  {
    label: "cte",
    detail: "WITH … AS ( … )",
    template:
      "WITH ${1:name} AS (\n  SELECT ${2:columns}\n  FROM ${3:table}\n)\nSELECT *\nFROM ${1:name}${0}",
  },
  {
    label: "case",
    detail: "CASE WHEN … END",
    template:
      "CASE\n  WHEN ${1:condition} THEN ${2:result}\n  ELSE ${3:fallback}\nEND${0}",
  },
];

// ---------------------------------------------------------------------------
// Alias resolution
// ---------------------------------------------------------------------------

function lastIdentifierPart(ref: string): string {
  const clean = ref.replace(/[`"\[\]]/g, "");
  const parts = clean.split(".");
  return parts[parts.length - 1] ?? clean;
}

/** Map table aliases (and plain table names) referenced in `sql` to tables. */
function collectAliases(
  engine: Engine | null,
  sql: string,
  schema: InsightSchema,
): Map<string, InsightTable> {
  const aliases = new Map<string, InsightTable>();
  const code = scanSqlSegments(engine, sql)
    .filter((s) => s.kind === "code")
    .map((s) => s.text)
    .join("\n");
  if (!code) return aliases;
  const re =
    /\b(?:from|join|update|into)\s+([A-Za-z_`"\[][\w$`"\]\.]*)\s*(?:as\s+)?([A-Za-z_][\w$]*)?/gi;
  let match: RegExpExecArray | null;
  while ((match = re.exec(code)) !== null) {
    const name = lastIdentifierPart(match[1] ?? "");
    const table = schema.tables.get(name.toLowerCase());
    if (!table) continue;
    aliases.set(name.toLowerCase(), table);
    const alias = match[2];
    if (alias && !RESERVED_WORDS.has(alias.toUpperCase())) {
      aliases.set(alias.toLowerCase(), table);
    }
  }
  return aliases;
}

function findColumnOwner(
  engine: Engine | null,
  sql: string,
  schema: InsightSchema,
  column: string,
  qualifier: string | null,
): { table: InsightTable; column: ColumnDef } | null {
  const lower = column.toLowerCase();
  if (qualifier) {
    const aliasTables = collectAliases(engine, sql, schema);
    const table =
      aliasTables.get(qualifier.toLowerCase()) ??
      schema.tables.get(qualifier.toLowerCase()) ??
      null;
    const col = table?.columns.find((c) => c.name.toLowerCase() === lower);
    if (table && col) return { table, column: col };
  }
  // Prefer tables that appear in the query, then fall back to the full list.
  const referenced = new Set<InsightTable>(collectAliases(engine, sql, schema).values());
  for (const table of referenced) {
    const col = table.columns.find((c) => c.name.toLowerCase() === lower);
    if (col) return { table, column: col };
  }
  for (const table of schema.list) {
    const col = table.columns.find((c) => c.name.toLowerCase() === lower);
    if (col) return { table, column: col };
  }
  return null;
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

function tableCompletion(table: InsightTable): Completion {
  return {
    label: table.name,
    type: "class",
    detail: table.kind === "view" ? "view" : "table",
    boost: 30,
  };
}

function columnCompletion(
  column: ColumnDef,
  table: InsightTable,
  boost: number,
): Completion {
  return {
    label: column.name,
    type: "property",
    detail: column.is_primary_key ? `${column.native_type} · PK` : column.native_type,
    info: `${table.name}.${column.name}`,
    boost,
  };
}

/**
 * CodeMirror completion source: tables, columns, engine keywords and snippets.
 * `getScope` is called on demand so the source always sees the current scope.
 */
export function completionSource(
  engine: Engine | null,
  sessionId: string | null | undefined,
  getScope: () => Scope | null | undefined,
): CompletionSource {
  return (ctx) => {
    const text = ctx.state.doc.toString();
    if (!isCodePosition(engine, text, ctx.pos)) return null;
    const dotMatch = ctx.matchBefore(/([A-Za-z_][\w$]*)\.([\w$]*)$/);
    const word = ctx.matchBefore(/[A-Za-z_][\w$]*/);
    if (!ctx.explicit && !dotMatch && !word) return null;

    if (sessionId) {
      // Load/refresh in the background; completions use whatever is cached.
      void getSchema(sessionId, getScope());
    }
    const schema = sessionId ? peekSchema(sessionId, getScope()) : null;

    const options: Completion[] = [];
    let from = word?.from ?? ctx.pos;
    let qualifier: InsightTable | null = null;
    if (dotMatch) {
      const dot = dotMatch.text.lastIndexOf(".");
      const q = dotMatch.text.slice(0, dot).toLowerCase();
      from = dotMatch.from + dot + 1;
      qualifier =
        (schema ? collectAliases(engine, text, schema).get(q) : undefined) ??
        schema?.tables.get(q) ??
        null;
      // Unknown qualifier: stay quiet instead of suggesting unrelated names.
      if (!qualifier) return null;
    }

    if (qualifier) {
      for (const col of qualifier.columns) {
        options.push(columnCompletion(col, qualifier, 20));
      }
    } else {
      if (schema) {
        for (const table of schema.list) options.push(tableCompletion(table));
        const seen = new Set<string>();
        const pushColumns = (table: InsightTable, boost: number) => {
          for (const col of table.columns) {
            const key = col.name.toLowerCase();
            if (seen.has(key)) continue;
            seen.add(key);
            options.push(columnCompletion(col, table, boost));
          }
        };
        const referenced = new Set<InsightTable>(
          collectAliases(engine, text, schema).values(),
        );
        for (const table of referenced) pushColumns(table, 10);
        for (const table of schema.list) pushColumns(table, 0);
      }
      for (const keyword of keywordsFor(engine)) {
        options.push({ label: keyword, type: "keyword", boost: -10 });
      }
      for (const snippet of SNIPPETS) {
        options.push(
          snippetCompletion(snippet.template, {
            label: snippet.label,
            detail: snippet.detail,
            type: "text",
            boost: -20,
          }),
        );
      }
    }

    if (options.length === 0) return null;
    return { from, options, validFor: /^[\w$]*$/ };
  };
}

// ---------------------------------------------------------------------------
// Hover tooltips
// ---------------------------------------------------------------------------

function appendLine(dom: HTMLElement, className: string, text: string): void {
  const el = document.createElement("div");
  el.className = className;
  el.textContent = text;
  dom.appendChild(el);
}

function tooltipFor(word: SqlWord, build: (dom: HTMLElement) => void): Tooltip {
  const dom = document.createElement("div");
  dom.className =
    "mieo-insight-tooltip max-w-[380px] px-2 py-1.5 font-code text-[12px] leading-snug shadow-lg";
  build(dom);
  return {
    pos: word.from,
    end: word.to,
    arrow: false,
    create: () => ({ dom, offset: { x: 0, y: 4 } }),
  };
}

function tableTooltip(table: InsightTable, word: SqlWord): Tooltip {
  return tooltipFor(word, (dom) => {
    appendLine(
      dom,
      "font-semibold text-slate-800",
      `${table.name} · ${table.columns.length} column${table.columns.length === 1 ? "" : "s"}`,
    );
    if (table.comment) appendLine(dom, "text-slate-500", table.comment);
    const shown = table.columns.slice(0, 12);
    for (const col of shown) {
      appendLine(
        dom,
        "mt-0.5",
        `${col.name}  ${col.native_type}${col.is_primary_key ? " · PK" : ""}`,
      );
    }
    if (table.columns.length > shown.length) {
      appendLine(dom, "mt-0.5 text-slate-400", `… ${table.columns.length - shown.length} more`);
    }
  });
}

function columnTooltip(
  table: InsightTable,
  column: ColumnDef,
  word: SqlWord,
): Tooltip {
  const isFk = table.foreignKeys.some((fk) =>
    fk.columns.some((c) => c.toLowerCase() === column.name.toLowerCase()),
  );
  const keys = [column.is_primary_key ? "PK" : null, isFk ? "FK" : null]
    .filter(Boolean)
    .join(", ");
  return tooltipFor(word, (dom) => {
    appendLine(
      dom,
      "font-semibold text-slate-800",
      `${table.name}.${column.name} · ${column.native_type}${keys ? ` · ${keys}` : ""}`,
    );
    appendLine(dom, "text-slate-500", column.nullable ? "nullable" : "not null");
    if (column.default_value) {
      appendLine(dom, "text-slate-500", `default ${column.default_value}`);
    }
    if (column.comment) appendLine(dom, "mt-0.5 text-slate-500", column.comment);
  });
}

function keywordTooltip(word: SqlWord, engine: Engine | null): Tooltip {
  return tooltipFor(word, (dom) => {
    appendLine(dom, "font-semibold text-slate-800", word.text.toUpperCase());
    appendLine(dom, "text-slate-500", `SQL keyword${engine ? ` · ${engine}` : ""}`);
  });
}

/**
 * Hover tooltip extension: table summary, column details
 * (`table.column · type · PK/FK`) and a keyword fallback.
 */
export function schemaHover(
  engine: Engine | null,
  sessionId: string | null | undefined,
  getScope: () => Scope | null | undefined,
): Extension {
  return hoverTooltip(
    async (view, pos) => {
      const text = view.state.doc.toString();
      const word = identifierAt(engine, text, pos);
      if (!word) return null;
      const lower = word.text.toLowerCase();
      const keyword = keywordsFor(engine).some((k) => k.toLowerCase() === lower);
      if (!sessionId) {
        return keyword ? keywordTooltip(word, engine) : null;
      }
      const schema = await getSchema(sessionId, getScope());
      if (!schema) {
        return keyword ? keywordTooltip(word, engine) : null;
      }
      const table = schema.tables.get(lower);
      if (table) return tableTooltip(table, word);

      // `alias.column` → resolve through the alias map when possible.
      let qualifier: string | null = null;
      if (word.from > 0 && text[word.from - 1] === ".") {
        const before = identifierAt(engine, text, word.from - 2);
        qualifier = before?.text ?? null;
      }
      const owner = findColumnOwner(engine, text, schema, word.text, qualifier);
      if (owner) return columnTooltip(owner.table, owner.column, word);
      return keyword ? keywordTooltip(word, engine) : null;
    },
    { hoverTime: 250 },
  );
}

/** Completion + hover extensions for EditorPane (reconfigured per scope). */
export function insightExtensions(
  engine: Engine | null,
  sessionId: string | null | undefined,
  getScope: () => Scope | null | undefined,
): Extension {
  return [
    autocompletion({ override: [completionSource(engine, sessionId, getScope)] }),
    schemaHover(engine, sessionId, getScope),
  ];
}
