// SQL parameter detection & binding for editor runs.
//
// Frozen API consumed by the store (`requestRun` / `runWithParams`) and the
// Code Insight workstream. Improve the internals freely, but keep the
// signatures stable — the store depends on them.

import type { Engine, Value } from "./bindings";

export type PlaceholderInfo = {
  /** 1-based parameter index (statement order for `?`, positional for `$n`). */
  index: number;
  /** Raw token, e.g. `?` or `$2`. */
  token: string;
  /** Character offset of the token in the SQL text. */
  position: number;
};

type ScanState =
  | "normal"
  | "single"
  | "double"
  | "backtick"
  | "line_comment"
  | "block_comment"
  | "dollar";

/**
 * Find bind placeholders in `sql` for the given engine dialect.
 *
 * - MySQL / MariaDB / SQLite: `?`
 * - PostgreSQL: `$1`, `$2`, …
 *
 * Placeholders inside strings, identifiers and comments are ignored; dollar
 * quoting (`$$…$$`) is handled for Postgres.
 */
export function findPlaceholders(
  engine: Engine | null,
  sql: string,
): PlaceholderInfo[] {
  const out: PlaceholderInfo[] = [];
  const postgres = engine === "postgres";
  let i = 0;
  let state: ScanState = "normal";
  let dollarTag = "";

  while (i < sql.length) {
    const ch = sql[i]!;
    const next = sql[i + 1];

    if (state === "normal") {
      if (ch === "'") {
        state = "single";
        i++;
        continue;
      }
      if (ch === '"') {
        state = "double";
        i++;
        continue;
      }
      if (ch === "`") {
        state = "backtick";
        i++;
        continue;
      }
      if (ch === "-" && next === "-") {
        state = "line_comment";
        i += 2;
        continue;
      }
      if (ch === "/" && next === "*") {
        state = "block_comment";
        i += 2;
        continue;
      }
      if (ch === "?") {
        out.push({ index: out.length + 1, token: "?", position: i });
        i++;
        continue;
      }
      if (postgres && ch === "$") {
        // `$1` … `$n` → placeholder
        let j = i + 1;
        while (j < sql.length && sql[j]! >= "0" && sql[j]! <= "9") j++;
        if (j > i + 1) {
          const token = sql.slice(i, j);
          out.push({ index: Number(sql.slice(i + 1, j)), token, position: i });
          i = j;
          continue;
        }
        // `$tag$` / `$$` → dollar-quoted string start
        let k = i + 1;
        while (
          k < sql.length &&
          /[A-Za-z0-9_]/.test(sql[k]!)
        ) {
          k++;
        }
        if (k < sql.length && sql[k] === "$") {
          state = "dollar";
          dollarTag = sql.slice(i, k + 1);
          i = k + 1;
          continue;
        }
      }
      i++;
      continue;
    }

    if (state === "single") {
      if (ch === "'" && next === "'") {
        i += 2;
        continue;
      }
      if (ch === "\\" && next !== undefined) {
        i += 2;
        continue;
      }
      if (ch === "'") state = "normal";
      i++;
      continue;
    }
    if (state === "double") {
      if (ch === '"') state = "normal";
      i++;
      continue;
    }
    if (state === "backtick") {
      if (ch === "`") state = "normal";
      i++;
      continue;
    }
    if (state === "line_comment") {
      if (ch === "\n") state = "normal";
      i++;
      continue;
    }
    if (state === "block_comment") {
      if (ch === "*" && next === "/") {
        state = "normal";
        i += 2;
        continue;
      }
      i++;
      continue;
    }
    // dollar-quoted string
    if (sql.startsWith(dollarTag, i)) {
      i += dollarTag.length;
      state = "normal";
      continue;
    }
    i++;
  }
  return out;
}

/** Bind raw input strings to the placeholders found in `sql`. */
export function bindPlaceholders(
  engine: Engine | null,
  sql: string,
  values: Array<string | null>,
): { sql: string; params: Value[] } {
  const infos = findPlaceholders(engine, sql);
  const params: Value[] = infos.map((_, i) => valueFromInput(values[i] ?? null));
  return { sql, params };
}

/** Heuristic conversion of a raw input string into a typed `Value`. */
export function valueFromInput(raw: string | null): Value {
  if (raw === null || raw === "") return { t: "null" };
  if (raw === "true" || raw === "false") return { t: "bool", v: raw === "true" };
  if (/^-?\d+$/.test(raw) && Number.isSafeInteger(Number(raw))) {
    return { t: "int", v: raw };
  }
  if (/^-?\d*\.\d+$/.test(raw)) return { t: "decimal", v: raw };
  return { t: "text", v: raw };
}
