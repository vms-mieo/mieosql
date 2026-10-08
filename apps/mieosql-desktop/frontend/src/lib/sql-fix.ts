// Local SQL syntax fixer — pure heuristics for the most common mistakes.
// Deliberately conservative: every change is presented in a preview dialog
// before being applied, and risky situations are reported as `notes` instead
// of being rewritten. Strings / quoted identifiers / comments are never
// touched (segment-scanner based, same as sql-format).

import type { Engine } from "./bindings";
import { isCodePosition, scanSqlSegments } from "./sql-format";

export type SqlFix = { rule: string; message: string };

export type SqlFixResult = {
  /** Repaired text (equals the input when nothing applies). */
  fixed: string;
  /** Changes that were applied. */
  fixes: SqlFix[];
  /** Problems found but not auto-fixed (need manual review). */
  notes: string[];
};

/**
 * Known schema/table names for the active connection — lets the fixer repair
 * misspelled identifiers (FROM userz → FROM users) that keyword heuristics
 * cannot see.
 */
export type IdentifierHints = {
  tables: readonly string[];
  schemas: readonly string[];
};

// ---------------------------------------------------------------------------
// Keyword candidates
// ---------------------------------------------------------------------------

const CANDIDATES: readonly string[] = [
  // statements / clauses
  "SELECT", "FROM", "WHERE", "GROUP", "ORDER", "BY", "HAVING", "LIMIT", "OFFSET",
  "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "ALTER",
  "DROP", "TRUNCATE", "RENAME", "TABLE", "VIEW", "INDEX", "SEQUENCE", "DATABASE",
  "SCHEMA", "PRIMARY", "FOREIGN", "REFERENCES", "UNIQUE", "CHECK", "DEFAULT",
  "CONSTRAINT", "CASCADE", "RESTRICT", "AUTOINCREMENT",
  // expressions
  "AND", "OR", "NOT", "NULL", "TRUE", "FALSE", "IN", "LIKE", "ILIKE", "BETWEEN",
  "IS", "AS", "CASE", "WHEN", "THEN", "ELSE", "END", "EXISTS", "DISTINCT",
  "ASC", "DESC", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "OUTER", "CROSS",
  "NATURAL", "ON", "USING", "UNION", "ALL", "EXCEPT", "INTERSECT", "WITH",
  "RETURNING", "WINDOW", "OVER", "PARTITION", "ROWS", "RANGE", "UNBOUNDED",
  "PRECEDING", "FOLLOWING", "CURRENT", "ROW",
  // transactions / admin
  "BEGIN", "COMMIT", "ROLLBACK", "TRANSACTION", "SAVEPOINT", "GRANT", "REVOKE",
  "EXPLAIN", "ANALYZE", "PRAGMA", "VACUUM", "REPLACE", "MERGE", "CALL",
  "EXECUTE", "IF", "ELSEIF", "WHILE", "DECLARE", "FUNCTION", "PROCEDURE",
  "TRIGGER", "RETURNS", "RETURN", "LANGUAGE", "CONFLICT", "DO", "NOTHING",
  // types
  "INT", "INTEGER", "BIGINT", "SMALLINT", "TINYINT", "SERIAL", "BIGSERIAL",
  "DECIMAL", "NUMERIC", "FLOAT", "REAL", "DOUBLE", "PRECISION", "MONEY",
  "VARCHAR", "CHAR", "NVARCHAR", "TEXT", "CLOB", "BLOB", "BINARY", "VARBINARY",
  "BOOLEAN", "BOOL", "BIT", "DATE", "DATETIME", "DATETIME2", "TIMESTAMP",
  "TIMESTAMPTZ", "TIME", "INTERVAL", "UUID", "JSON", "JSONB", "XML",
  // functions
  "COUNT", "SUM", "AVG", "MIN", "MAX", "COALESCE", "NULLIF", "CAST", "CONVERT",
  "LENGTH", "CHAR_LENGTH", "SUBSTRING", "SUBSTR", "TRIM", "LTRIM", "RTRIM",
  "UPPER", "LOWER", "INITCAP", "REPLACE", "CONCAT", "CONCAT_WS", "FORMAT",
  "NOW", "CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME", "DATEADD",
  "DATEDIFF", "DATE_TRUNC", "EXTRACT", "ROUND", "ABS", "CEILING", "FLOOR",
  "POWER", "SQRT", "RANDOM", "RAND", "GREATEST", "LEAST", "IFNULL", "ISNULL",
  "STRING_AGG", "GROUP_CONCAT", "LISTAGG", "ARRAY_AGG", "ROW_NUMBER", "RANK",
  "DENSE_RANK", "LAG", "LEAD", "FIRST_VALUE", "LAST_VALUE", "NTILE",
  "JSON_EXTRACT", "JSON_VALUE", "STRFTIME", "DATE_FORMAT", "TO_CHAR",
  "TO_DATE", "TO_NUMBER", "GENERATE_SERIES", "SETVAL", "NEXTVAL", "CURRVAL",
];

const VALID = new Set<string>(CANDIDATES);

/** Keyword pairs commonly written glued together (no space). */
export const GLUED: Record<string, string> = {
  GROUPBY: "GROUP BY",
  ORDERBY: "ORDER BY",
  UNIONALL: "UNION ALL",
  INSERTINTO: "INSERT INTO",
  DELETEFROM: "DELETE FROM",
  SELECTDISTINCT: "SELECT DISTINCT",
  LEFTOUTERJOIN: "LEFT OUTER JOIN",
  LEFTJOIN: "LEFT JOIN",
  RIGHTJOIN: "RIGHT JOIN",
  INNERJOIN: "INNER JOIN",
  FULLJOIN: "FULL JOIN",
  CROSSJOIN: "CROSS JOIN",
  ISNOT: "IS NOT",
  NOTIN: "NOT IN",
  NOTLIKE: "NOT LIKE",
  NOTBETWEEN: "NOT BETWEEN",
  ORDERBYDESC: "ORDER BY DESC",
  PARTITIONBY: "PARTITION BY",
};

const GLUED_RE = new RegExp(`\\b(${Object.keys(GLUED).join("|")})\\b`, "gi");

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function mapCode(
  engine: Engine | null,
  sql: string,
  fn: (code: string) => string,
): string {
  return scanSqlSegments(engine, sql)
    .map((seg) => (seg.kind === "code" ? fn(seg.text) : seg.text))
    .join("");
}

/** Optimal string-alignment distance with an early cap (adjacent swaps = 1). */
function editDistance(a: string, b: string, cap: number): number {
  const m = a.length;
  const n = b.length;
  if (Math.abs(m - n) > cap) return cap + 1;
  let prev2: number[] | null = null;
  let prev = Array.from({ length: n + 1 }, (_, j) => j);
  for (let i = 1; i <= m; i++) {
    const cur = new Array<number>(n + 1);
    cur[0] = i;
    let rowMin = cur[0]!;
    for (let j = 1; j <= n; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      let v = Math.min(prev[j]! + 1, cur[j - 1]! + 1, prev[j - 1]! + cost);
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) {
        v = Math.min(v, prev2![j - 2]! + 1);
      }
      cur[j] = v;
      if (v < rowMin) rowMin = v;
    }
    if (rowMin > cap) return cap + 1;
    prev2 = prev;
    prev = cur;
  }
  return prev[n]!;
}

function sameCase(source: string, target: string): string {
  return source === source.toLowerCase() ? target.toLowerCase() : target;
}

/** Same letters, different order (WEHER ↔ WHERE, UDPATE ↔ UPDATE). */
function isAnagram(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  return a.split("").sort().join("") === b.split("").sort().join("");
}

/** Suggest a keyword/function for a mistyped word, or null when unsure. */
export function suggestKeyword(word: string): string | null {
  const up = word.toUpperCase();
  if (up.length < 3 || up.length > 24) return null;
  if (VALID.has(up)) return null;
  // Skip placeholders and identifier-ish tokens.
  if (/[$@#:]/.test(word)) return null;

  let best: string | null = null;
  let bestD = 99;
  for (const kw of CANDIDATES) {
    if (kw[0] !== up[0]) continue;
    const raw = editDistance(up, kw, 2);
    const d = raw === 1 || (up.length >= 5 && isAnagram(up, kw)) ? 1 : raw;
    if (d < bestD) {
      bestD = d;
      best = kw;
    }
    if (bestD === 1) break;
  }
  if (!best) return null;
  if (up.startsWith(best)) {
    // "orders" must not become ORDER — but a doubled final letter is a typo
    // ("FROMM" → FROM, "WHEREE" → WHERE).
    const extra = up.slice(best.length);
    const doubledLast = extra.length === 1 && extra === best[best.length - 1];
    return doubledLast ? sameCase(word, best) : null;
  }
  if (bestD === 1) return sameCase(word, best);
  if (bestD === 2 && up.length >= 7) {
    return sameCase(word, best);
  }
  return null;
}

// ---------------------------------------------------------------------------
// Identifier hints (table / schema typos)
// ---------------------------------------------------------------------------

/** Closest known identifier, or null when unsure / when it already matches. */
function nearestIdentifier(
  word: string,
  candidates: readonly string[],
): string | null {
  const lower = word.toLowerCase();
  if (lower.length < 3 || lower.length > 64) return null;
  let best: string | null = null;
  let bestD = 99;
  for (const candidate of candidates) {
    const cl = candidate.toLowerCase();
    if (cl === lower) return null; // already correct
    if (cl[0] !== lower[0]) continue;
    if (Math.abs(cl.length - lower.length) > 2) continue;
    const d = editDistance(lower, cl, 2);
    if (d < bestD) {
      bestD = d;
      best = candidate;
      if (d === 1) break;
    }
  }
  const threshold = lower.length >= 5 ? 2 : 1;
  return best && bestD <= threshold ? sameCase(word, best) : null;
}

/** `FROM/JOIN/UPDATE/INTO/USING/TABLE [IF EXISTS]` followed by a name. */
const TABLE_POSITION_RE =
  /\b(FROM|JOIN|UPDATE|INTO|USING|TABLE)(\s+)(IF\s+EXISTS\s+)?([A-Za-z_][A-Za-z0-9_$]*)(?:(\s*\.\s*)([A-Za-z_][A-Za-z0-9_$]*))?/gi;

/**
 * Words that introduce an identifier — the keyword fixer must leave the next
 * word alone there (`FROM dbo` must not become `FROM DO`).
 */
const IDENTIFIER_INTRO = new Set([
  "FROM",
  "JOIN",
  "INTO",
  "UPDATE",
  "USING",
  "TABLE",
  "EXISTS",
  "ON",
  "AS",
  "BY",
]);

export function isIdentifierPosition(code: string, offset: number): boolean {
  const before = code.slice(0, offset);
  // Right-hand side of `schema.table`.
  if (/\.\s*$/.test(before)) return true;
  const prev = /([A-Za-z_][A-Za-z0-9_$]*)\s*$/.exec(before)?.[1]?.toUpperCase();
  return prev !== undefined && IDENTIFIER_INTRO.has(prev);
}

function fixIdentifiers(
  code: string,
  hints: IdentifierHints,
  messages: string[],
): string {
  const tablesKnown = new Set(hints.tables.map((n) => n.toLowerCase()));
  const schemasKnown = new Set(hints.schemas.map((n) => n.toLowerCase()));
  return code.replace(
    TABLE_POSITION_RE,
    (
      match: string,
      keyword: string,
      spacing: string,
      ifExists: string | undefined,
      first: string,
      dotPart: string | undefined,
      second: string | undefined,
    ) => {
      // A bare keyword can never be a table name (defensive).
      if (VALID.has(first.toUpperCase())) return match;

      if (second !== undefined) {
        let schema = first;
        if (!schemasKnown.has(first.toLowerCase()) && hints.schemas.length > 0) {
          const fixed = nearestIdentifier(first, hints.schemas);
          if (fixed) {
            messages.push(`Schema "${first}" → "${fixed}"`);
            schema = fixed;
          }
        }
        let table = second;
        if (
          !VALID.has(second.toUpperCase()) &&
          !tablesKnown.has(second.toLowerCase()) &&
          hints.tables.length > 0
        ) {
          const fixed = nearestIdentifier(second, hints.tables);
          if (fixed) {
            messages.push(`Table "${second}" → "${fixed}"`);
            table = fixed;
          }
        }
        if (schema === first && table === second) return match;
        return `${keyword}${spacing}${ifExists ?? ""}${schema}${dotPart}${table}`;
      }

      if (!tablesKnown.has(first.toLowerCase()) && hints.tables.length > 0) {
        const fixed = nearestIdentifier(first, hints.tables);
        if (fixed) {
          messages.push(`Table "${first}" → "${fixed}"`);
          return `${keyword}${spacing}${ifExists ?? ""}${fixed}`;
        }
      }
      return match;
    },
  );
}

// ---------------------------------------------------------------------------
// Main entry
// ---------------------------------------------------------------------------

export function autofixSql(
  engine: Engine | null,
  sql: string,
  hints?: IdentifierHints,
): SqlFixResult {
  const fixes: SqlFix[] = [];
  const notes: string[] = [];
  let text = sql;

  const step = (
    rule: string,
    message: string,
    fn: (t: string) => string,
  ): void => {
    const next = fn(text);
    if (next !== text) {
      text = next;
      fixes.push({ rule, message });
    }
  };

  // 1. keyword pairs glued together -------------------------------------
  step("spacing", "Split glued keywords (GROUPBY → GROUP BY, …)", (t) =>
    mapCode(engine, t, (code) =>
      code.replace(GLUED_RE, (m) =>
        m === m.toLowerCase() ? GLUED[m.toUpperCase()]!.toLowerCase() : GLUED[m.toUpperCase()]!,
      ),
    ),
  );

  // 2. common operator typos --------------------------------------------
  step("operator", "== → = (SQL compares with a single =)", (t) =>
    mapCode(engine, t, (code) => code.replace(/===/g, "=").replace(/==/g, "=")),
  );
  step("operator", "=< → <=  ·  => → >=", (t) =>
    mapCode(engine, t, (code) => code.replace(/=</g, "<=").replace(/=>/g, ">=")),
  );

  // 3. commas ------------------------------------------------------------
  step("comma", "Collapse duplicate commas (,, → ,)", (t) =>
    mapCode(engine, t, (code) => code.replace(/(,\s*){2,}/g, ",")),
  );
  step("comma", "Remove a dangling comma before ) or a clause keyword", (t) =>
    mapCode(engine, t, (code) =>
      code
        .replace(/,(\s*\))/g, "$1")
        .replace(
          /,(\s+(?:FROM|WHERE|GROUP|ORDER|HAVING|LIMIT|VALUES|SET|UNION|EXCEPT|INTERSECT|JOIN|LEFT|RIGHT|INNER|FULL|CROSS)\b)/gi,
          "$1",
        ),
    ),
  );
  step("comma", "Remove a trailing comma at the end of the statement", (t) => {
    const m = /,\s*$/.exec(t);
    if (!m) return t;
    if (!isCodePosition(engine, t, m.index)) return t;
    return t.replace(/,\s*$/, "");
  });

  // 4. keyword / function typos ------------------------------------------
  step("typo", "Fix mistyped keywords and functions (SELET → SELECT, …)", (t) =>
    mapCode(engine, t, (code) =>
      code.replace(/[A-Za-z_][A-Za-z0-9_$]*/g, (w, offset: number) =>
        isIdentifierPosition(code, offset) ? w : (suggestKeyword(w) ?? w),
      ),
    ),
  );

  // 4b. table / schema identifier typos (needs schema hints) --------------
  if (hints && (hints.tables.length > 0 || hints.schemas.length > 0)) {
    const messages: string[] = [];
    const next = mapCode(engine, text, (code) =>
      fixIdentifiers(code, hints, messages),
    );
    if (next !== text) {
      text = next;
      for (const message of messages) {
        fixes.push({ rule: "identifier", message });
      }
    }
  }

  // 5. dangling binary keyword at the very end ---------------------------
  step("dangling", "Remove a dangling AND/OR at the end of the statement", (t) => {
    const m = /\b(AND|OR)\s*$/i.exec(t);
    if (!m) return t;
    if (!isCodePosition(engine, t, m.index)) return t;
    return t.replace(/\b(AND|OR)\s*$/i, "").replace(/[ \t]+$/, "");
  });

  // 6. unbalanced parentheses (code only) ---------------------------------
  {
    let opens = 0;
    let closes = 0;
    for (const seg of scanSqlSegments(engine, text)) {
      if (seg.kind !== "code") continue;
      for (const ch of seg.text) {
        if (ch === "(") opens++;
        else if (ch === ")") closes++;
      }
    }
    if (opens > closes) {
      const missing = opens - closes;
      const insert = ")".repeat(missing);
      const semi = /\s*;\s*$/.test(text);
      text = semi
        ? text.replace(/\s*;\s*$/, `${insert};`)
        : text.replace(/\s*$/, "") + insert;
      fixes.push({
        rule: "paren",
        message: `Close ${missing} unclosed parenthesis${missing > 1 ? "es" : ""} at the end`,
      });
    } else if (closes > opens) {
      notes.push(
        `${closes - opens} extra ")" — review the parentheses manually (not auto-fixed).`,
      );
    }
  }

  // 7. unterminated quotes ------------------------------------------------
  {
    const segs = scanSqlSegments(engine, text);
    const last = segs[segs.length - 1];
    if (last && last.to === text.length) {
      const close: Partial<Record<string, string>> = {
        single: "'",
        double: '"',
        backtick: "`",
        bracket: "]",
      };
      const q = close[last.kind];
      if (q) {
        const body = last.text;
        const closed =
          body.length > 1 &&
          body.endsWith(q) &&
          body[body.length - 2] !== "\\";
        if (!closed) {
          text = `${text}${q}`;
          fixes.push({
            rule: "quote",
            message: `Add the missing closing ${q === "'" ? "quote" : `“${q}”`} at the end of the string`,
          });
          notes.push(
            "The unterminated string may swallow following text — verify the result carefully.",
          );
        }
      }
    }
  }

  return { fixed: text, fixes, notes };
}
