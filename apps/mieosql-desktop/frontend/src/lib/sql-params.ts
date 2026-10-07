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

/** Explicit parameter type chosen in the parameter dialog. */
export type ParamType = "auto" | "text" | "int" | "decimal" | "bool" | "null";

/**
 * Marker used to smuggle an explicit type through `store.runWithParams`
 * (whose signature only carries raw strings). NUL cannot be typed into an
 * input, so it is safe as a sentinel.
 */
const TYPE_MARK = "\u0000";

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
        while (k < sql.length && /[A-Za-z0-9_]/.test(sql[k]!)) {
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

/**
 * Bind raw input strings to the placeholders found in `sql`.
 *
 * For Postgres the parameter array is positional (`$n` → slot `n-1`), so
 * repeated/out-of-order placeholders still bind correctly. The `values` array
 * is aligned with the order of `findPlaceholders(...)` (unique indices).
 */
export function bindPlaceholders(
  engine: Engine | null,
  sql: string,
  values: Array<string | null>,
): { sql: string; params: Value[] } {
  const infos = findPlaceholders(engine, sql);
  const params: Value[] = [];
  if (engine === "postgres") {
    const seen = new Set<number>();
    let slot = 0;
    for (const info of infos) {
      if (seen.has(info.index)) continue;
      seen.add(info.index);
      params[info.index - 1] = valueFromInput(values[slot] ?? null);
      slot++;
    }
    for (let i = 0; i < params.length; i++) {
      if (params[i] === undefined) params[i] = { t: "null" };
    }
  } else {
    for (let i = 0; i < infos.length; i++) {
      params.push(valueFromInput(values[i] ?? null));
    }
  }
  return { sql, params };
}

/** Encode a dialog input together with its explicit type (auto is a passthrough). */
export function encodeTypedInput(raw: string, type: ParamType): string {
  if (type === "auto") return raw;
  if (type === "null") return `${TYPE_MARK}null:`;
  return `${TYPE_MARK}${type}:${raw}`;
}

/** Heuristic conversion of a raw input string into a typed `Value`. */
export function valueFromInput(raw: string | null, type?: ParamType): Value {
  if (raw !== null && raw.startsWith(TYPE_MARK)) {
    const sep = raw.indexOf(":", 1);
    const encoded = raw.slice(1, sep) as ParamType;
    return valueFromTyped(raw.slice(sep + 1), encoded);
  }
  return valueFromTyped(raw, type ?? "auto");
}

function valueFromTyped(raw: string | null, type: ParamType): Value {
  if (type === "null") return { t: "null" };
  if (raw === null || (raw === "" && type === "auto")) return { t: "null" };
  switch (type) {
    case "text":
      return { t: "text", v: raw };
    case "int":
      return /^-?\d+$/.test(raw) && Number.isSafeInteger(Number(raw))
        ? { t: "int", v: raw }
        : { t: "text", v: raw };
    case "decimal":
      return /^-?\d+(\.\d+)?$/.test(raw)
        ? { t: "decimal", v: raw }
        : { t: "text", v: raw };
    case "bool": {
      const lower = raw.toLowerCase();
      if (lower === "true" || lower === "1") return { t: "bool", v: true };
      if (lower === "false" || lower === "0") return { t: "bool", v: false };
      return { t: "text", v: raw };
    }
    case "auto":
    default:
      break;
  }
  // auto
  if (raw === "true" || raw === "false") return { t: "bool", v: raw === "true" };
  if (/^-?\d+$/.test(raw) && Number.isSafeInteger(Number(raw))) {
    return { t: "int", v: raw };
  }
  if (/^-?\d*\.\d+$/.test(raw)) return { t: "decimal", v: raw };
  return { t: "text", v: raw };
}
