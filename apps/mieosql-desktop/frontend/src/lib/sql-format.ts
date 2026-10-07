// SQL text utilities for the editor: pretty formatting, minification and
// identifier-aware scans (rename / insight). Pure functions — callers apply
// the result through a CodeMirror transaction.
//
// The scanner understands engine-specific quoting so strings, quoted
// identifiers and comments are never mutated (beyond surrounding whitespace).

import type { Engine } from "./bindings";

// ---------------------------------------------------------------------------
// Segment scanner
// ---------------------------------------------------------------------------

export type SqlSegmentKind =
  | "code"
  | "single"
  | "double"
  | "backtick"
  | "bracket"
  | "line_comment"
  | "block_comment"
  | "dollar";

export type SqlSegment = {
  kind: SqlSegmentKind;
  /** Character offset of the segment start in the SQL text. */
  from: number;
  /** Character offset of the segment end (exclusive). */
  to: number;
  /** Verbatim slice of the source. */
  text: string;
};

function isEscapeStringPrefix(sql: string, quoteAt: number): boolean {
  const prev = quoteAt > 0 ? sql[quoteAt - 1] : undefined;
  if (prev !== "E" && prev !== "e") return false;
  const before = quoteAt > 1 ? sql[quoteAt - 2] : undefined;
  return before === undefined || !/[A-Za-z0-9_$]/.test(before);
}

function matchDollarTag(sql: string, i: number): string | null {
  if (sql[i + 1] === "$") return "$$";
  if (!/[A-Za-z_]/.test(sql[i + 1] ?? "")) return null;
  let j = i + 2;
  while (j < sql.length && /[A-Za-z0-9_]/.test(sql[j]!)) j++;
  if (sql[j] === "$") return sql.slice(i, j + 1);
  return null;
}

/**
 * Split `sql` into code and literal/comment segments. Every character of the
 * input belongs to exactly one segment; code segments may be empty/omitted.
 */
export function scanSqlSegments(
  engine: Engine | null,
  sql: string,
): SqlSegment[] {
  const mysql = engine === "mysql" || engine === "mariadb";
  const postgres = engine === "postgres";
  const sqlServer = engine === "sql_server";
  const out: SqlSegment[] = [];
  let i = 0;
  let codeFrom = 0;

  const flushCode = (to: number) => {
    if (to > codeFrom) {
      out.push({
        kind: "code",
        from: codeFrom,
        to,
        text: sql.slice(codeFrom, to),
      });
    }
  };
  const push = (kind: SqlSegmentKind, from: number, to: number) => {
    flushCode(from);
    out.push({ kind, from, to, text: sql.slice(from, to) });
    codeFrom = to;
  };

  while (i < sql.length) {
    const ch = sql[i]!;
    const next = sql[i + 1];

    // line comments -------------------------------------------------------
    if (ch === "-" && next === "-") {
      let j = i + 2;
      while (j < sql.length && sql[j] !== "\n") j++;
      push("line_comment", i, j);
      i = j;
      continue;
    }
    if (mysql && ch === "#") {
      let j = i + 1;
      while (j < sql.length && sql[j] !== "\n") j++;
      push("line_comment", i, j);
      i = j;
      continue;
    }

    // block comments -------------------------------------------------------
    if (ch === "/" && next === "*") {
      let depth = 1;
      let j = i + 2;
      while (j < sql.length && depth > 0) {
        if (postgres && sql[j] === "/" && sql[j + 1] === "*") {
          depth++;
          j += 2;
          continue;
        }
        if (sql[j] === "*" && sql[j + 1] === "/") {
          depth--;
          j += 2;
          continue;
        }
        j++;
      }
      push("block_comment", i, Math.min(j, sql.length));
      i = Math.min(j, sql.length);
      continue;
    }

    // single-quoted strings -------------------------------------------------
    if (ch === "'") {
      const backslash = mysql || (postgres && isEscapeStringPrefix(sql, i));
      let j = i + 1;
      while (j < sql.length) {
        if (backslash && sql[j] === "\\") {
          j += 2;
          continue;
        }
        if (sql[j] === "'" && sql[j + 1] === "'") {
          j += 2;
          continue;
        }
        if (sql[j] === "'") {
          j++;
          break;
        }
        j++;
      }
      push("single", i, Math.min(j, sql.length));
      i = Math.min(j, sql.length);
      continue;
    }

    // double-quoted identifiers / strings -----------------------------------
    if (ch === '"') {
      let j = i + 1;
      while (j < sql.length) {
        if (mysql && sql[j] === "\\") {
          j += 2;
          continue;
        }
        if (sql[j] === '"' && sql[j + 1] === '"') {
          j += 2;
          continue;
        }
        if (sql[j] === '"') {
          j++;
          break;
        }
        j++;
      }
      push("double", i, Math.min(j, sql.length));
      i = Math.min(j, sql.length);
      continue;
    }

    // backtick identifiers ---------------------------------------------------
    if (ch === "`") {
      let j = i + 1;
      while (j < sql.length) {
        if (sql[j] === "`" && sql[j + 1] === "`") {
          j += 2;
          continue;
        }
        if (sql[j] === "`") {
          j++;
          break;
        }
        j++;
      }
      push("backtick", i, Math.min(j, sql.length));
      i = Math.min(j, sql.length);
      continue;
    }

    // [bracketed] identifiers (SQL Server) -----------------------------------
    if (sqlServer && ch === "[") {
      let j = i + 1;
      while (j < sql.length) {
        if (sql[j] === "]" && sql[j + 1] === "]") {
          j += 2;
          continue;
        }
        if (sql[j] === "]") {
          j++;
          break;
        }
        j++;
      }
      push("bracket", i, Math.min(j, sql.length));
      i = Math.min(j, sql.length);
      continue;
    }

    // dollar-quoted strings (Postgres) ---------------------------------------
    if (postgres && ch === "$") {
      const tag = matchDollarTag(sql, i);
      if (tag) {
        const end = sql.indexOf(tag, i + tag.length);
        const to = end === -1 ? sql.length : end + tag.length;
        push("dollar", i, to);
        i = to;
        continue;
      }
    }

    i++;
  }
  flushCode(sql.length);
  return out;
}

/** True when `pos` falls inside a code (non string/comment) segment. */
export function isCodePosition(
  engine: Engine | null,
  sql: string,
  pos: number,
): boolean {
  const clamped = Math.max(0, Math.min(pos, sql.length));
  for (const seg of scanSqlSegments(engine, sql)) {
    if (clamped < seg.from || clamped > seg.to) continue;
    return seg.kind === "code";
  }
  return true;
}

// ---------------------------------------------------------------------------
// Identifier scans (rename / hover)
// ---------------------------------------------------------------------------

export type SqlWord = { text: string; from: number; to: number };

function isWordChar(ch: string | undefined): boolean {
  return ch !== undefined && /[A-Za-z0-9_$]/.test(ch);
}

/** Identifier (word) under `pos`, restricted to code positions. */
export function identifierAt(
  engine: Engine | null,
  sql: string,
  pos: number,
): SqlWord | null {
  const clamped = Math.max(0, Math.min(pos, sql.length));
  const segments = scanSqlSegments(engine, sql);
  for (const seg of segments) {
    if (clamped < seg.from || clamped > seg.to) continue;
    if (seg.kind !== "code") return null;
    let from = clamped;
    let to = clamped;
    while (from > seg.from && isWordChar(sql[from - 1])) from--;
    while (to < seg.to && isWordChar(sql[to])) to++;
    if (from === to) return null;
    const text = sql.slice(from, to);
    if (!/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(text)) return null;
    return { text, from, to };
  }
  return null;
}

/**
 * Case-sensitive, word-boundary occurrences of `word` in code positions only
 * (strings, quoted identifiers and comments are skipped).
 */
export function identifierOccurrences(
  engine: Engine | null,
  sql: string,
  word: string,
): Array<{ from: number; to: number }> {
  if (!word) return [];
  const out: Array<{ from: number; to: number }> = [];
  for (const seg of scanSqlSegments(engine, sql)) {
    if (seg.kind !== "code") continue;
    const text = seg.text;
    let at = 0;
    while ((at = text.indexOf(word, at)) !== -1) {
      const before = at > 0 ? text[at - 1] : undefined;
      const after = at + word.length < text.length ? text[at + word.length] : undefined;
      if (!isWordChar(before) && !isWordChar(after)) {
        out.push({ from: seg.from + at, to: seg.from + at + word.length });
      }
      at += word.length;
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Minify
// ---------------------------------------------------------------------------

/**
 * Collapse whitespace outside strings/comments into single spaces and trim
 * the code edges. Content of literals/comments is preserved verbatim; line
 * comments keep their terminating newline so nothing gets swallowed.
 */
export function minifySql(engine: Engine | null, sql: string): string {
  const segments = scanSqlSegments(engine, sql);
  let out = "";
  let afterLineComment = false;
  for (let i = 0; i < segments.length; i++) {
    const seg = segments[i]!;
    if (seg.kind === "line_comment") {
      out += seg.text;
      afterLineComment = true;
      continue;
    }
    let text: string;
    if (seg.kind === "code") {
      text = seg.text.replace(/\s+/g, " ");
      if (i === 0) text = text.replace(/^\s+/, "");
      if (i === segments.length - 1) text = text.replace(/\s+$/, "");
    } else {
      text = seg.text;
    }
    if (afterLineComment) {
      out += "\n";
      text = text.replace(/^[ \t]+/, "");
      afterLineComment = false;
    }
    out += text;
  }
  return out;
}

// ---------------------------------------------------------------------------
// Format (pretty print)
// ---------------------------------------------------------------------------

type Token = {
  kind: "word" | "number" | "string" | "comment" | "punct";
  text: string;
  /** comment only: `--`/`#` style, must keep its own line */
  line?: boolean;
  /** true when whitespace (or a comment) preceded this token in the source */
  spaceBefore?: boolean;
};

const OPERATORS = ["->>", "->", "::", ":=", "<=", ">=", "<>", "!=", "||", "&&", "**"];

const SIGN_LEADERS = new Set([
  "BETWEEN",
  "AND",
  "OR",
  "NOT",
  "WHERE",
  "HAVING",
  "ON",
  "SET",
  "VALUES",
  "SELECT",
  "LIMIT",
  "OFFSET",
  "THEN",
  "ELSE",
  "WHEN",
  "IN",
  "LIKE",
  "BY",
]);

function readNumber(s: string, i: number): number {
  let j = i;
  if (s[j] === "0" && (s[j + 1] === "x" || s[j + 1] === "X")) {
    j += 2;
    while (j < s.length && /[0-9A-Fa-f]/.test(s[j]!)) j++;
    return j;
  }
  while (j < s.length && /[0-9]/.test(s[j]!)) j++;
  if (s[j] === "." && /[0-9]/.test(s[j + 1] ?? "")) {
    j++;
    while (j < s.length && /[0-9]/.test(s[j]!)) j++;
  }
  if ((s[j] === "e" || s[j] === "E") && /[0-9+-]/.test(s[j + 1] ?? "")) {
    let k = j + 1;
    if (s[k] === "+" || s[k] === "-") k++;
    if (/[0-9]/.test(s[k] ?? "")) {
      k++;
      while (k < s.length && /[0-9]/.test(s[k]!)) k++;
      j = k;
    }
  }
  return j;
}

function tokenize(engine: Engine | null, sql: string): Token[] {
  const toks: Token[] = [];
  let prevEnd: number | null = null;
  const emit = (tok: Token, start: number) => {
    tok.spaceBefore = prevEnd !== null && start > prevEnd;
    prevEnd = start + tok.text.length;
    toks.push(tok);
  };
  for (const seg of scanSqlSegments(engine, sql)) {
    if (seg.kind === "line_comment" || seg.kind === "block_comment") {
      emit(
        { kind: "comment", text: seg.text, line: seg.kind === "line_comment" },
        seg.from,
      );
      continue;
    }
    if (seg.kind !== "code") {
      emit({ kind: "string", text: seg.text }, seg.from);
      continue;
    }
    const s = seg.text;
    let i = 0;
    while (i < s.length) {
      const ch = s[i]!;
      if (/\s/.test(ch)) {
        i++;
        continue;
      }
      if (ch === ":" && (s[i + 1] === ":" || s[i + 1] === "=")) {
        emit({ kind: "punct", text: s.slice(i, i + 2) }, seg.from + i);
        i += 2;
        continue;
      }
      if (/[A-Za-z_@:$]/.test(ch)) {
        let j = i + 1;
        while (j < s.length && /[A-Za-z0-9_$]/.test(s[j]!)) j++;
        if ((ch === "@" || ch === ":" || ch === "$") && j === i + 1) {
          emit({ kind: "punct", text: ch }, seg.from + i);
          i++;
          continue;
        }
        emit({ kind: "word", text: s.slice(i, j) }, seg.from + i);
        i = j;
        continue;
      }
      // signed numeric literal: `-1`, `+2.5`, `WHERE x = -3`
      if (ch === "-" || ch === "+") {
        const last = toks.length > 0 ? toks[toks.length - 1] : undefined;
        const signable =
          last === undefined ||
          (last.kind === "punct" &&
            last.text !== ")" &&
            last.text !== "]" &&
            last.text !== "*") ||
          (last.kind === "word" && SIGN_LEADERS.has(last.text.toUpperCase()));
        if (signable && /[0-9]/.test(s[i + 1] ?? "")) {
          const j = readNumber(s, i + 1);
          emit({ kind: "number", text: s.slice(i, j) }, seg.from + i);
          i = j;
          continue;
        }
      }
      if (/[0-9]/.test(ch)) {
        const j = readNumber(s, i);
        emit({ kind: "number", text: s.slice(i, j) }, seg.from + i);
        i = j;
        continue;
      }
      const op = OPERATORS.find((o) => s.startsWith(o, i));
      if (op) {
        emit({ kind: "punct", text: op }, seg.from + i);
        i += op.length;
        continue;
      }
      emit({ kind: "punct", text: ch }, seg.from + i);
      i++;
    }
  }
  return toks;
}

type Clause = { kind: "major" | "join" | "on" | "andor"; len: number };

const JOIN_LEADERS = new Set(["INNER", "LEFT", "RIGHT", "FULL", "CROSS", "NATURAL"]);

const MAJOR_KEYWORDS = new Set([
  "SELECT",
  "FROM",
  "WHERE",
  "HAVING",
  "LIMIT",
  "OFFSET",
  "VALUES",
  "RETURNING",
  "WINDOW",
  "SET",
  "INSERT",
  "UPDATE",
  "DELETE",
  "MERGE",
  "CREATE",
  "ALTER",
  "DROP",
  "TRUNCATE",
  "WITH",
  "EXPLAIN",
  "PRAGMA",
  "BEGIN",
  "COMMIT",
  "ROLLBACK",
  "REPLACE",
  "CALL",
  "UNION",
  "EXCEPT",
  "INTERSECT",
]);

const SUBQUERY_STARTERS = new Set(["SELECT", "WITH", "VALUES"]);

const NO_SPACE_BEFORE = new Set([",", ";", ")", ".", "::", "->", "->>", "]", "["]);
const NO_SPACE_AFTER = new Set(["(", "[", ".", "::", "->", "->>"]);

const KEYWORDS_BEFORE_PAREN = new Set([
  "IN",
  "NOT",
  "EXISTS",
  "VALUES",
  "SELECT",
  "WHERE",
  "HAVING",
  "ON",
  "AND",
  "OR",
  "USING",
  "BETWEEN",
  "LIKE",
  "RETURNING",
  "SET",
  "AS",
  "FROM",
  "JOIN",
  "CASE",
  "WHEN",
  "THEN",
  "ELSE",
  "OVER",
  "PARTITION",
  "ALL",
  "ANY",
  "SOME",
  "DISTINCT",
  "UNION",
  "EXCEPT",
  "INTERSECT",
  "DELETE",
  "INSERT",
  "UPDATE",
  "WITH",
  "BY",
  "ORDER",
  "GROUP",
  "LIMIT",
  "OFFSET",
  "CHECK",
  "DEFAULT",
]);

function wordUpperAt(toks: Token[], i: number): string | null {
  const t = toks[i];
  return t && t.kind === "word" ? t.text.toUpperCase() : null;
}

function clauseAt(toks: Token[], i: number): Clause | null {
  const w = wordUpperAt(toks, i);
  if (!w) return null;
  if (w === "AND" || w === "OR") return { kind: "andor", len: 1 };
  if (w === "ON") return { kind: "on", len: 1 };
  if (w === "JOIN") return { kind: "join", len: 1 };
  if (JOIN_LEADERS.has(w)) {
    let j = i + 1;
    let len = 1;
    if (wordUpperAt(toks, j) === "OUTER") {
      len++;
      j++;
    }
    if (wordUpperAt(toks, j) === "JOIN") return { kind: "join", len: len + 1 };
    return null;
  }
  if (w === "GROUP" && wordUpperAt(toks, i + 1) === "BY") {
    return { kind: "major", len: 2 };
  }
  if (w === "ORDER" && wordUpperAt(toks, i + 1) === "BY") {
    return { kind: "major", len: 2 };
  }
  if (w === "UNION" && wordUpperAt(toks, i + 1) === "ALL") {
    return { kind: "major", len: 2 };
  }
  if (MAJOR_KEYWORDS.has(w)) return { kind: "major", len: 1 };
  return null;
}

function nextNonComment(toks: Token[], start: number): Token | null {
  for (let i = start; i < toks.length; i++) {
    const t = toks[i]!;
    if (t.kind !== "comment") return t;
  }
  return null;
}

function needsSpace(prev: Token, next: Token): boolean {
  if (next.kind === "punct" && NO_SPACE_BEFORE.has(next.text)) return false;
  if (prev.kind === "punct" && NO_SPACE_AFTER.has(prev.text)) return false;
  if (
    next.kind === "punct" &&
    next.text === "*" &&
    prev.kind === "punct" &&
    prev.text === "("
  ) {
    return false;
  }
  if (
    prev.kind === "punct" &&
    prev.text === "*" &&
    next.kind === "punct" &&
    next.text === ")"
  ) {
    return false;
  }
  if (next.kind === "punct" && next.text === "(") {
    if (prev.kind === "word") {
      if (KEYWORDS_BEFORE_PAREN.has(prev.text.toUpperCase())) return true;
      // Preserve the source's call-vs-list spacing: `count(*)` stays tight,
      // `table (a, b)` keeps its space.
      return next.spaceBefore !== false;
    }
    if (prev.kind === "punct") return !NO_SPACE_AFTER.has(prev.text);
    return true;
  }
  return true;
}

/**
 * Pretty-print `sql`: a newline before major clauses, indented subqueries and
 * hanging `AND`/`OR`/`ON` continuations. String/comment content is untouched.
 */
export function formatSql(engine: Engine | null, sql: string): string {
  const toks = tokenize(engine, sql);
  if (toks.length === 0) return "";

  let out = "";
  let indent = 0;
  let prev: Token | null = null;
  const parenStack: boolean[] = [];
  let betweenPending = false;
  let clauseSkip = 0;

  const startsLine = (): boolean => {
    const trimmed = out.replace(/[ \t]+$/, "");
    return trimmed.length === 0 || trimmed.endsWith("\n");
  };
  const newline = (level: number): void => {
    out = out.replace(/[ \t]+$/, "");
    if (out.length === 0) return;
    if (!out.endsWith("\n")) out += "\n";
    out += "  ".repeat(level);
  };

  for (let i = 0; i < toks.length; i++) {
    const tok = toks[i]!;

    // Comments always keep their own line.
    if (tok.kind === "comment") {
      if (!startsLine()) newline(indent);
      out += tok.text;
      if (tok.line || tok.text.includes("\n")) {
        newline(indent);
        prev = null;
      } else {
        prev = tok;
      }
      continue;
    }

    let clause: Clause | null = i >= clauseSkip ? clauseAt(toks, i) : null;
    if (tok.kind === "word") {
      const upper = tok.text.toUpperCase();
      if (upper === "BETWEEN") betweenPending = true;
      if (clause?.kind === "andor" && betweenPending) {
        betweenPending = false;
        clause = null;
      }
    }
    if (clause) {
      const level = clause.kind === "major" || clause.kind === "join" ? indent : indent + 1;
      newline(level);
      prev = null;
      if (clause.len > 1) clauseSkip = i + clause.len;
    } else if (prev?.kind === "comment" && prev.line) {
      newline(indent);
      prev = null;
    }

    // Parentheses: subqueries open a new indented block.
    if (tok.kind === "punct" && (tok.text === "(" || tok.text === ")")) {
      if (tok.text === "(") {
        const nextTok = nextNonComment(toks, i + 1);
        const multiline =
          nextTok?.kind === "word" && SUBQUERY_STARTERS.has(nextTok.text.toUpperCase());
        parenStack.push(multiline);
        if (prev && needsSpace(prev, tok)) out += " ";
        out += "(";
        prev = tok;
        if (multiline) {
          indent++;
          newline(indent);
          prev = null;
        }
        continue;
      }
      const multiline = parenStack.pop() ?? false;
      if (multiline) {
        indent = Math.max(0, indent - 1);
        newline(indent);
        prev = null;
      }
      if (prev && needsSpace(prev, tok)) out += " ";
      out += ")";
      prev = tok;
      continue;
    }

    if (prev && needsSpace(prev, tok)) out += " ";
    out += tok.text;
    prev = tok;

    if (tok.kind === "punct" && tok.text === ";") {
      if (i < toks.length - 1) {
        newline(indent);
        prev = null;
      }
    }
  }

  return out
    .split("\n")
    .map((line) => line.replace(/[ \t]+$/, ""))
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}
