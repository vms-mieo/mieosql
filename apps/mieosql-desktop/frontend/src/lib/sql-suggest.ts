// SQL suggestion helpers: pure functions behind the editor's Code Insight.
//
// Everything here is dependency-free (types only) so the parsing logic can be
// exercised without a database connection or a CodeMirror instance:
//   - completion context detection (tables vs columns vs join vs star)
//   - table-reference + alias parsing (`FROM` / `JOIN` / `UPDATE` / `INTO`)
//   - `JOIN … ON` context parsing and FK-based join-condition building
//     (the DBeaver-style suggestion, always boosted to the top)
//   - CTE name collection (`WITH x AS (…)`, `, y AS (…)`)
//   - a per-engine SQL function catalog (name + signature + docs)
//
// The CodeMirror wiring lives in `sql-insight.ts`, which imports from here.

import type { Engine } from "./bindings";
import { scanSqlSegments } from "./sql-format.ts";

// ---------------------------------------------------------------------------
// Structural schema view (satisfied by `InsightTable` in sql-insight.ts)
// ---------------------------------------------------------------------------

export interface SuggestColumn {
  name: string;
  native_type: string;
  is_primary_key: boolean;
}

export interface SuggestFk {
  columns: string[];
  ref_table: string;
  ref_columns: string[];
}

export interface SuggestTable {
  name: string;
  columns: SuggestColumn[];
  foreignKeys: SuggestFk[];
}

/**
 * Text safe for keyword parsing: code plus quoted identifiers (backtick /
 * bracket / double-quoted), with strings, dollar bodies and comments dropped.
 * Quoted identifiers are kept verbatim — the table-reference patterns
 * understand quoting and `baseName` strips it.
 */
function codeOnly(sql: string, engine: Engine | null = null): string {
  void engine;
  // Concatenate verbatim (original spacing inside code segments survives, so
  // `"shop"."order"` stays whole); dropped strings/comments become one space
  // so neighbouring keywords never glue together.
  return scanSqlSegments(null, sql)
    .map((s) =>
      s.kind === "code" || s.kind === "backtick" || s.kind === "bracket" || s.kind === "double"
        ? s.text
        : " ",
    )
    .join("");
}

// ---------------------------------------------------------------------------
// Completion context
// ---------------------------------------------------------------------------

export type CompletionContextKind = "tables" | "columns" | "general";

/**
 * What the cursor most likely wants, from the last meaningful keyword before
 * it. Positional like DBeaver's legacy engine: the *last* keyword wins, so
 * `SELECT … FROM (SELECT … WHERE` still resolves to columns.
 */
export function detectContext(sqlBefore: string, engine: Engine | null = null): CompletionContextKind {
  sqlBefore = codeOnly(sqlBefore, engine);
  const re =
    /\b(from|join|update|into|delete|table|create|drop|alter|on|using|select|where|set|values|having|group\s+by|order\s+by|and|or|by|when|then|else|limit|offset)\b/gi;
  let kind: CompletionContextKind = "general";
  let match: RegExpExecArray | null;
  while ((match = re.exec(sqlBefore)) !== null) {
    const kw = match[1]!.toLowerCase().replace(/\s+/g, " ");
    if (
      kw === "from" ||
      kw === "join" ||
      kw === "update" ||
      kw === "into" ||
      kw === "delete" ||
      kw === "table" ||
      kw === "create" ||
      kw === "drop" ||
      kw === "alter"
    ) {
      kind = "tables";
    } else {
      kind = "columns";
    }
  }
  return kind;
}

// ---------------------------------------------------------------------------
// Table references + aliases
// ---------------------------------------------------------------------------

export interface TableRef {
  /** Raw referenced name (may be qualified); use `baseName` for lookups. */
  name: string;
  /** Explicit alias, or the base name when no alias is given. */
  alias: string;
}

const RESERVED_ALIAS = new Set([
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
  "OFFSET",
  "BY",
  "CASE",
]);

/** Last dotted part of a possibly quoted/qualified reference. */
export function baseName(ref: string): string {
  const clean = ref.replace(/[`"\[\]]/g, "");
  const parts = clean.split(".");
  return parts[parts.length - 1] ?? clean;
}

/**
 * Every `FROM / JOIN / UPDATE / INTO <table> [[AS] alias]` in `sql`, in
 * order. Quoted and schema-qualified names are supported.
 */
export function parseTableRefs(sql: string, engine: Engine | null = null): TableRef[] {
  sql = codeOnly(sql, engine);
  const refs: TableRef[] = [];
  const re =
    /\b(?:from|join|update|into)\s+([A-Za-z_`"\[][^\s,;()]*)\s*(?:as\s+)?([A-Za-z_][\w$]*)?/gi;
  let match: RegExpExecArray | null;
  while ((match = re.exec(sql)) !== null) {
    const name = baseName(match[1] ?? "");
    if (!name) continue;
    const alias = match[2];
    refs.push({
      name,
      alias:
        alias && !RESERVED_ALIAS.has(alias.toUpperCase()) ? alias : name,
    });
  }
  return refs;
}

// ---------------------------------------------------------------------------
// JOIN … ON / USING context
// ---------------------------------------------------------------------------

export interface JoinContext {
  /** The joined (right-side) table reference. */
  right: TableRef;
  /** Table references left of the join (FROM + earlier JOINs). */
  left: TableRef[];
  /** True when the cursor is inside `USING (…)`. */
  using: boolean;
  /** True once `ON`/`USING` was typed (condition context vs table context). */
  hasCondition: boolean;
}

const JOIN_TAIL_ENDERS =
  /\b(where|group\s+by|order\s+by|having|limit|offset|union|except|intersect|join|returning|window)\b/i;

/**
 * Parse the innermost `JOIN <table> [AS alias] [ON|USING] <partial>` when the
 * cursor sits right inside its condition. Returns null when the ON clause
 * already ended (e.g. a WHERE/GROUP BY follows) or a `;` intervenes.
 */
export function parseJoinOn(sqlBefore: string, engine: Engine | null = null): JoinContext | null {
  sqlBefore = codeOnly(sqlBefore, engine);
  const re =
    /\bjoin\s+([A-Za-z_`"\[][^\s,;()]*)(?:\s+as\s+|\s+)?([A-Za-z_][\w$]*)?/gi;
  let last: { ref: string; alias: string | undefined; end: number } | null = null;
  let match: RegExpExecArray | null;
  while ((match = re.exec(sqlBefore)) !== null) {
    const ref = (match[1] ?? "").trim();
    if (!ref || ref === "(") continue;
    let end = match.index + match[0].length;
    let alias: string | undefined = match[2];
    if (alias && RESERVED_ALIAS.has(alias.toUpperCase())) {
      // The "alias" is really ON/USING/a clause keyword: rewind so the tail
      // starts at it (`JOIN b USING (` must see the USING).
      end -= alias.length;
      alias = undefined;
    }
    last = { ref, alias, end };
  }
  if (!last) return null;
  const tail = sqlBefore.slice(last.end);
  if (tail.includes(";")) return null;
  if (JOIN_TAIL_ENDERS.test(tail)) return null;
  const kw = /^\s*(on|using)\b/i.exec(tail);
  // Bare `JOIN t ` with nothing after is a table-name context, not a join
  // condition — but `JOIN t ON ` (empty condition) *is* one.
  if (!kw && !/^\s*$/.test(tail)) return null;
  const base = baseName(last.ref);
  if (!base) return null;
  const right: TableRef = { name: base, alias: last.alias ?? base };
  const leftRefs = parseTableRefs(sqlBefore.slice(0, last.end)).filter(
    (r) => r.name.toLowerCase() !== right.name.toLowerCase() || r.alias.toLowerCase() !== right.alias.toLowerCase(),
  );
  return {
    right,
    left: leftRefs,
    using: (kw?.[1] ?? "").toLowerCase() === "using",
    hasCondition: kw != null,
  };
}

/** `a.x = b.y` conditions between two tables, following FKs both ways. */
export function fkConditions(
  left: SuggestTable,
  leftAlias: string,
  right: SuggestTable,
  rightAlias: string,
): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  const push = (text: string) => {
    const key = text.toLowerCase();
    if (!seen.has(key)) {
      seen.add(key);
      out.push(text);
    }
  };
  const zip = (
    cols: string[],
    refCols: string[],
    localAlias: string,
    refAlias: string,
  ) => {
    const n = Math.min(cols.length, refCols.length);
    if (n === 0) return null;
    const parts: string[] = [];
    for (let i = 0; i < n; i++) {
      parts.push(`${localAlias}.${cols[i]} = ${refAlias}.${refCols[i]}`);
    }
    return parts.join(" AND ");
  };
  // FKs declared on the right table pointing at the left one.
  for (const fk of right.foreignKeys) {
    if (fk.ref_table.toLowerCase() !== left.name.toLowerCase()) continue;
    const cond = zip(fk.columns, fk.ref_columns, rightAlias, leftAlias);
    if (cond) push(cond);
  }
  // FKs declared on the left table pointing at the right one.
  for (const fk of left.foreignKeys) {
    if (fk.ref_table.toLowerCase() !== right.name.toLowerCase()) continue;
    const cond = zip(fk.columns, fk.ref_columns, leftAlias, rightAlias);
    if (cond) push(cond);
  }
  return out;
}

/** Column names present in both tables (for `USING (…)`). */
export function sharedColumns(a: SuggestTable, b: SuggestTable): string[] {
  const names = new Set(b.columns.map((c) => c.name.toLowerCase()));
  return a.columns.map((c) => c.name).filter((n) => names.has(n.toLowerCase()));
}

// ---------------------------------------------------------------------------
// CTE names
// ---------------------------------------------------------------------------

/** Names from `WITH x AS (…)` / `, y AS (…)` (non-recursive scan). */
export function collectCteNames(sql: string): string[] {
  sql = codeOnly(sql);
  const names: string[] = [];
  const seen = new Set<string>();
  const re = /\b([A-Za-z_][\w$]*)\s+as\s*\(/gi;
  let match: RegExpExecArray | null;
  while ((match = re.exec(sql)) !== null) {
    const name = match[1]!;
    const key = name.toLowerCase();
    if (!seen.has(key) && !RESERVED_ALIAS.has(key.toUpperCase())) {
      seen.add(key);
      names.push(name);
    }
  }
  return names;
}

// ---------------------------------------------------------------------------
// SELECT * expansion
// ---------------------------------------------------------------------------

/**
 * Position of a top-level `SELECT *` star directly before the cursor
 * (`SELECT *⎵` / `SELECT DISTINCT *`). Used for explicit column expansion.
 */
export function matchStarExpand(sqlBefore: string): number | null {
  if (!/select\s+(?:distinct\s+)?\*\s*$/i.test(sqlBefore)) return null;
  return sqlBefore.lastIndexOf("*");
}

// ---------------------------------------------------------------------------
// SQL function catalog
// ---------------------------------------------------------------------------

export interface SqlFunction {
  name: string;
  /** Inserted argument names; a trailing `?` marks an optional argument. */
  args: string[];
  signature: string;
  doc: string;
  /** Engines offering it; omitted = all SQL engines. */
  engines?: Engine[];
}

function fn(
  name: string,
  args: string[],
  doc: string,
  engines?: Engine[],
): SqlFunction {
  const rendered = args
    .map((a) => (a.endsWith("?") ? `[${a.slice(0, -1)}]` : a))
    .join(", ");
  return { name, args, signature: `${name}(${rendered})`, doc, engines };
}

const COMMON_FUNCTIONS: SqlFunction[] = [
  fn("COUNT", ["expr"], "Row count; COUNT(*) counts all rows."),
  fn("SUM", ["expr"], "Sum of non-NULL values."),
  fn("AVG", ["expr"], "Average of non-NULL values."),
  fn("MIN", ["expr"], "Minimum value."),
  fn("MAX", ["expr"], "Maximum value."),
  fn("COALESCE", ["v1", "v2?"], "First non-NULL argument."),
  fn("NULLIF", ["a", "b"], "NULL when a = b, else a."),
  fn("CONCAT", ["s1", "s2?"], "Concatenate strings."),
  fn("SUBSTRING", ["str", "pos", "len?"], "Part of a string (1-based)."),
  fn("TRIM", ["str"], "Strip leading/trailing spaces."),
  fn("LTRIM", ["str"], "Strip leading spaces."),
  fn("RTRIM", ["str"], "Strip trailing spaces."),
  fn("UPPER", ["str"], "Uppercase."),
  fn("LOWER", ["str"], "Lowercase."),
  fn("LENGTH", ["str"], "Length in characters."),
  fn("REPLACE", ["str", "from", "to"], "Replace occurrences."),
  fn("LEFT", ["str", "n"], "First n characters."),
  fn("RIGHT", ["str", "n"], "Last n characters."),
  fn("ROUND", ["x", "d?"], "Round to d decimals."),
  fn("FLOOR", ["x"], "Round down."),
  fn("CEIL", ["x"], "Round up."),
  fn("ABS", ["x"], "Absolute value."),
  fn("MOD", ["a", "b"], "Remainder."),
  fn("POWER", ["x", "y"], "x to the power y."),
  fn("SQRT", ["x"], "Square root."),
  fn("CAST", ["expr", "type"], "Convert value to a type."),
  fn("ROW_NUMBER", [], "Row number in a window: ROW_NUMBER() OVER (…)."),
  fn("RANK", [], "Rank with gaps: RANK() OVER (…)."),
  fn("DENSE_RANK", [], "Rank without gaps: DENSE_RANK() OVER (…)."),
  fn("LAG", ["expr", "offset?", "default?"], "Value from an earlier row in a window."),
  fn("LEAD", ["expr", "offset?", "default?"], "Value from a later row in a window."),
  fn("NOW", [], "Current date and time."),
  fn("CURRENT_DATE", [], "Current date."),
  fn("CURRENT_TIMESTAMP", [], "Current date and time."),
];

const ENGINE_FUNCTIONS: SqlFunction[] = [
  fn("GROUP_CONCAT", ["expr", "sep?"], "Group values into one string.", ["mysql", "mariadb", "sqlite"]),
  fn("IFNULL", ["a", "b"], "b when a is NULL.", ["mysql", "mariadb", "sqlite"]),
  fn("IF", ["cond", "a", "b"], "a when cond is true, else b.", ["mysql", "mariadb"]),
  fn("DATE_FORMAT", ["d", "fmt"], "Format a date (MySQL format string).", ["mysql", "mariadb"]),
  fn("DATEDIFF", ["d1", "d2"], "Days d1 − d2.", ["mysql", "mariadb"]),
  fn("JSON_EXTRACT", ["doc", "path"], "Extract from JSON.", ["mysql", "mariadb"]),
  fn("STRING_AGG", ["expr", "sep?"], "Group values into one string.", ["postgres", "sql_server"]),
  fn("TO_CHAR", ["v", "fmt"], "Format a value as text.", ["postgres"]),
  fn("GENERATE_SERIES", ["start", "stop"], "Set-returning number series.", ["postgres"]),
  fn("STRING_TO_ARRAY", ["str", "sep"], "Split a string into an array.", ["postgres"]),
  fn("GREATEST", ["a", "b?"], "Greatest argument.", ["mysql", "mariadb", "postgres"]),
  fn("LEAST", ["a", "b?"], "Smallest argument.", ["mysql", "mariadb", "postgres"]),
  fn("DATE_TRUNC", ["part", "d"], "Truncate a timestamp.", ["postgres"]),
  fn("GETDATE", [], "Current date and time.", ["sql_server"]),
  fn("SYSDATETIME", [], "Current date and time (datetime2).", ["sql_server"]),
  fn("ISNULL", ["a", "b"], "b when a is NULL.", ["sql_server"]),
  fn("IIF", ["cond", "a", "b"], "a when cond is true, else b.", ["sql_server"]),
  fn("LEN", ["str"], "Length in characters.", ["sql_server"]),
  fn("CHARINDEX", ["sub", "str"], "Position of sub in str.", ["sql_server"]),
  fn("STUFF", ["str", "pos", "len", "new"], "Delete + insert in a string.", ["sql_server"]),
  fn("FORMAT", ["v", "fmt"], "Format a value (.NET format string).", ["sql_server"]),
  fn("DATEDIFF", ["part", "start", "end"], "Difference between dates.", ["sql_server"]),
  fn("DATEADD", ["part", "n", "d"], "Add an interval to a date.", ["sql_server"]),
  fn("JSON_VALUE", ["doc", "path"], "Scalar from JSON.", ["sql_server"]),
  fn("GLOB", ["pattern", "str"], "Pattern match (Unix-style).", ["sqlite"]),
  fn("DATETIME", ["timestring", "mod?"], "Date/time arithmetic.", ["sqlite"]),
  fn("STRFTIME", ["fmt", "timestring"], "Format a date.", ["sqlite"]),
  fn("TYPEOF", ["x"], "Storage class of a value.", ["sqlite"]),
  fn("QUOTE", ["x"], "SQL-quoted literal of a value.", ["sqlite", "postgres"]),
];

/** Function catalog for an engine (common + engine-specific). */
export function functionsFor(engine: Engine | null): SqlFunction[] {
  if (!engine) return COMMON_FUNCTIONS;
  return [
    ...COMMON_FUNCTIONS,
    ...ENGINE_FUNCTIONS.filter((f) => f.engines?.includes(engine)),
  ];
}

/** Lookup for hover tooltips (case-insensitive). */
export function functionHelp(
  engine: Engine | null,
  name: string,
): SqlFunction | null {
  const lower = name.toLowerCase();
  for (const f of functionsFor(engine)) {
    if (f.name.toLowerCase() === lower) return f;
  }
  return null;
}

/** Snippet template inserting a call with tabstops, e.g. `SUB(a, b)`. */
export function functionTemplate(f: SqlFunction): string {
  if (f.args.length === 0) return `${f.name}()`;
  const stops = f.args.map((a, i) => `\${${i + 1}:${a.replace(/\?$/, "")}}`).join(", ");
  return `${f.name}(${stops})`;
}

// ---------------------------------------------------------------------------
// Unsafe-write detection (UPDATE/DELETE without WHERE)
// ---------------------------------------------------------------------------

export interface UnsafeWrite {
  kind: "UPDATE" | "DELETE";
  /** First line of the offending statement, for confirm dialogs. */
  preview: string;
}

const ROUTINE_HEAD = /^\s*create\s+(or\s+replace\s+)?(procedure|function|trigger|event)\b/i;
const BLOCK_ENDERS = ["if", "loop", "while", "case", "repeat"];

interface ScannedStatement {
  /** Original text (trimmed) for previews. */
  raw: string;
  /** Code only: strings/comments replaced by spaces. */
  code: string;
}

/** Split into top-level statements; `;` inside parens or routine bodies stays. */
function splitTopLevel(sql: string): ScannedStatement[] {
  const out: ScannedStatement[] = [];
  let raw = "";
  let code = "";
  let parenDepth = 0;
  let routine = false;
  let beginDepth = 0;
  const flush = () => {
    if (raw.trim().length > 0) out.push({ raw: raw.trim(), code });
    raw = "";
    code = "";
    routine = false;
    beginDepth = 0;
  };
  const isWordChar = (c: string) => /[A-Za-z0-9_$]/.test(c);
  const text = sql;
  let i = 0;
  let inSingle = false;
  let inLineComment = false;
  let inBlockComment = false;
  let word = "";
  const endWord = () => {
    if (!word) return;
    if (parenDepth === 0 && beginDepth === 0 && !routine && ROUTINE_HEAD.test(code + word)) {
      routine = true;
    }
    if (routine && parenDepth === 0) {
      const lower = word.toLowerCase();
      if (lower === "begin") {
        beginDepth++;
      } else if (lower === "end" && beginDepth > 0) {
        // Peek the next word: `END IF/LOOP/...` is not a body end.
        let j = i;
        while (j < text.length && /\s/.test(text[j]!)) j++;
        let nw = "";
        while (j < text.length && isWordChar(text[j]!)) {
          nw += text[j];
          j++;
        }
        if (!BLOCK_ENDERS.includes(nw.toLowerCase())) beginDepth--;
      }
    }
    code += word;
    word = "";
  };
  while (i < text.length) {
    const ch = text[i]!;
    const next = text[i + 1];
    if (inLineComment) {
      raw += ch;
      code += ch === "\n" ? "\n" : " ";
      if (ch === "\n") inLineComment = false;
      i++;
      continue;
    }
    if (inBlockComment) {
      raw += ch;
      code += " ";
      if (ch === "*" && next === "/") {
        raw += next!;
        i += 2;
      } else {
        i++;
      }
      if (text[i - 2] === "*" && text[i - 1] === "/") inBlockComment = false;
      continue;
    }
    if (inSingle) {
      raw += ch;
      code += " ";
      if (ch === "'" && next === "'") {
        raw += next!;
        i += 2;
        continue;
      }
      if (ch === "'") inSingle = false;
      i++;
      continue;
    }
    if (ch === "-" && next === "-") {
      endWord();
      inLineComment = true;
      raw += "--";
      code += "  ";
      i += 2;
      continue;
    }
    if (ch === "/" && next === "*") {
      endWord();
      inBlockComment = true;
      raw += "/*";
      code += "  ";
      i += 2;
      continue;
    }
    if (ch === "'") {
      endWord();
      inSingle = true;
      raw += ch;
      code += " ";
      i++;
      continue;
    }
    if (ch === '"' || ch === "`") {
      // Quoted identifiers stay verbatim (table names matter for previews).
      endWord();
      const quote = ch;
      raw += ch;
      code += ch;
      i++;
      while (i < text.length) {
        const c = text[i]!;
        raw += c;
        code += c;
        i++;
        if (c === quote) {
          if (text[i] === quote) {
            raw += text[i]!;
            code += text[i]!;
            i++;
          } else {
            break;
          }
        }
      }
      continue;
    }
    if (ch === "(" || ch === ")") {
      endWord();
      if (ch === "(") parenDepth++;
      else parenDepth = Math.max(0, parenDepth - 1);
      raw += ch;
      code += ch;
      i++;
      continue;
    }
    if (ch === ";" && parenDepth === 0 && beginDepth === 0) {
      endWord();
      raw += ch;
      flush();
      i++;
      continue;
    }
    if (isWordChar(ch)) {
      word += ch;
      raw += ch;
      i++;
      continue;
    }
    endWord();
    raw += ch;
    code += ch;
    i++;
  }
  endWord();
  flush();
  return out;
}

/** First keyword of a statement (`SELECT`, `UPDATE`, ...) or null. */
function firstKeyword(code: string): string | null {
  const m = /^\s*([A-Za-z_][\w$]*)/.exec(code);
  return m ? m[1]!.toUpperCase() : null;
}

/** Top-level WHERE (paren depth 0) in a single UPDATE/DELETE statement. */
function hasTopLevelWhere(code: string): boolean {
  let depth = 0;
  let word = "";
  const check = () => {
    const hit = depth === 0 && word.toLowerCase() === "where";
    word = "";
    return hit;
  };
  for (let idx = 0; idx < code.length; idx++) {
    const ch = code[idx]!;
    if (ch === "(") {
      if (check()) return true;
      depth++;
    } else if (ch === ")") {
      if (check()) return true;
      depth = Math.max(0, depth - 1);
    } else if (/[A-Za-z0-9_$]/.test(ch)) {
      word += ch;
    } else if (check()) {
      return true;
    }
  }
  return check();
}

function previewOf(raw: string): string {
  const line = raw.split("\n").find((l) => l.trim().length > 0) ?? raw;
  const clean = line.trim().replace(/\s+/g, " ");
  return clean.length > 120 ? `${clean.slice(0, 120)}…` : clean;
}

/**
 * UPDATE / DELETE statements without a top-level WHERE. Subquery WHEREs
 * don't count (`UPDATE t SET x = (SELECT ... WHERE ...)` still hits every
 * row); strings/comments can't smuggle a WHERE in; routine bodies
 * (`CREATE PROCEDURE ... BEGIN ... END`) are never flagged.
 */
export function findUnsafeWrites(sql: string): UnsafeWrite[] {
  const out: UnsafeWrite[] = [];
  for (const stmt of splitTopLevel(sql)) {
    const first = firstKeyword(stmt.code);
    if (first !== "UPDATE" && first !== "DELETE") continue;
    if (hasTopLevelWhere(stmt.code)) continue;
    out.push({
      kind: first,
      preview: previewOf(stmt.raw),
    });
  }
  return out;
}
