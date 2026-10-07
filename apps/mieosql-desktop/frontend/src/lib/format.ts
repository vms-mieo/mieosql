// Cell formatting, sorting, and export helpers for the data grid.

import type { ColumnMeta } from "@bindings/ColumnMeta";
import type { DataTypeKind } from "@bindings/DataTypeKind";
import type { Value } from "@bindings/Value";

export function isArrayKind(kind: DataTypeKind): kind is { array: DataTypeKind } {
  return typeof kind === "object" && kind !== null;
}

export function baseKind(kind: DataTypeKind): string {
  return isArrayKind(kind) ? "array" : kind;
}

const BADGES: Record<string, string> = {
  bool: "BOOL",
  int: "INT",
  uint: "UINT",
  float: "FLOAT",
  decimal: "DEC",
  text: "STR",
  bytes: "BIN",
  date: "DATE",
  time: "TIME",
  datetime: "DATETIME",
  timestamptz: "TIMESTAMPTZ",
  json: "JSON",
  uuid: "UUID",
  bit: "BIT",
  interval: "INTERVAL",
  enum: "ENUM",
  set: "SET",
  geometry: "GEO",
  array: "ARRAY",
  unknown: "?",
};

export function kindBadge(kind: DataTypeKind): string {
  return BADGES[baseKind(kind)] ?? "?";
}

const NUMERIC = new Set(["int", "uint", "float", "decimal"]);

export function isNumericKind(kind: DataTypeKind): boolean {
  return NUMERIC.has(baseKind(kind));
}

export type CellTone =
  | "null"
  | "number"
  | "temporal"
  | "json"
  | "binary"
  | "text"
  | "bool"
  | "unsupported";

export type CellView = {
  text: string;
  title: string;
  align: "left" | "right";
  tone: CellTone;
};

function base64DecodedLen(b64: string): number {
  const padding = (b64.match(/=+$/)?.[0] ?? "").length;
  return Math.max(0, Math.floor((b64.length * 3) / 4) - padding);
}

function truncateMiddle(s: string, max: number): string {
  if (s.length <= max) return s;
  const head = Math.ceil(max / 2) - 1;
  const tail = max - head - 1;
  return `${s.slice(0, head)}…${s.slice(-tail)}`;
}

/** Render a wire value for the grid. */
export function formatCell(value: Value): CellView {
  switch (value.t) {
    case "null":
      return { text: "[NULL]", title: "NULL", align: "left", tone: "null" };
    case "bool":
      return {
        text: value.v ? "true" : "false",
        title: String(value.v),
        align: "left",
        tone: "bool",
      };
    case "int":
    case "uint":
      return { text: value.v, title: value.v, align: "right", tone: "number" };
    case "float":
      return {
        text: String(value.v),
        title: String(value.v),
        align: "right",
        tone: "number",
      };
    case "decimal":
      return { text: value.v, title: value.v, align: "right", tone: "number" };
    case "text":
      return {
        text: value.v,
        title: value.v.length > 400 ? `${value.v.slice(0, 400)}…` : value.v,
        align: "left",
        tone: "text",
      };
    case "bytes":
      return {
        text: `<${base64DecodedLen(value.v)} B>`,
        title: "binary (base64)",
        align: "left",
        tone: "binary",
      };
    case "date":
    case "time":
    case "datetime":
      return { text: value.v, title: value.v, align: "left", tone: "temporal" };
    case "uuid":
      return { text: value.v, title: value.v, align: "left", tone: "text" };
    case "json": {
      let text: string;
      try {
        text = JSON.stringify(value.v) ?? "null";
      } catch {
        text = String(value.v);
      }
      return {
        text: truncateMiddle(text, 220),
        title: truncateMiddle(text, 4000),
        align: "left",
        tone: "json",
      };
    }
    case "array": {
      const parts = value.v.map((v) => plainText(v));
      const text = `[${parts.join(", ")}]`;
      return {
        text: truncateMiddle(text, 220),
        title: truncateMiddle(text, 4000),
        align: "left",
        tone: "json",
      };
    }
    case "unsupported":
      return {
        text: `‹${value.v.native_type}›`,
        title: value.v.reason,
        align: "left",
        tone: "unsupported",
      };
  }
}

/** Plain text of a value, used for copy operations (no placeholder text). */
export function plainText(value: Value): string {
  switch (value.t) {
    case "null":
      return "";
    case "bool":
      return value.v ? "true" : "false";
    case "int":
    case "uint":
    case "decimal":
      return value.v;
    case "float":
      return String(value.v);
    case "text":
    case "date":
    case "time":
    case "datetime":
    case "uuid":
      return value.v;
    case "bytes":
      return value.v; // base64
    case "json":
      return JSON.stringify(value.v) ?? "null";
    case "array":
      return `[${value.v.map(plainText).join(", ")}]`;
    case "unsupported":
      return `<${value.v.native_type}>`;
  }
}

/** Natural JSON representation for export (numbers stay numbers when safe). */
export function valueToJson(value: Value): unknown {
  switch (value.t) {
    case "null":
      return null;
    case "bool":
      return value.v;
    case "int":
    case "uint": {
      const n = Number(value.v);
      return Number.isSafeInteger(n) ? n : value.v;
    }
    case "float":
      return value.v;
    case "decimal":
    case "text":
    case "date":
    case "time":
    case "datetime":
    case "uuid":
      return value.v;
    case "bytes":
      return value.v; // base64 string
    case "json":
      return value.v;
    case "array":
      return value.v.map(valueToJson);
    case "unsupported":
      return `<${value.v.native_type}>`;
  }
}

function numericOf(value: Value): number | null {
  switch (value.t) {
    case "int":
    case "uint":
      return Number(value.v);
    case "float":
      return value.v;
    case "decimal":
      return Number(value.v);
    default:
      return null;
  }
}

/** Sort comparator; NULLs always last. */
export function compareValues(a: Value, b: Value, kind: DataTypeKind): number {
  if (a.t === "null" && b.t === "null") return 0;
  if (a.t === "null") return 1;
  if (b.t === "null") return -1;
  if (isNumericKind(kind)) {
    const na = numericOf(a);
    const nb = numericOf(b);
    if (na !== null && nb !== null && !Number.isNaN(na) && !Number.isNaN(nb)) {
      return na - nb;
    }
  }
  return plainText(a).localeCompare(plainText(b), undefined, {
    numeric: true,
    sensitivity: "base",
  });
}

export function toCsv(columns: ColumnMeta[], rows: Value[][]): string {
  const esc = (s: string) =>
    /[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
  const header = columns.map((c) => esc(c.name)).join(",");
  const body = rows
    .map((row) => row.map((v) => esc(plainText(v))).join(","))
    .join("\n");
  return `${header}\n${body}`;
}

export function toJsonLines(columns: ColumnMeta[], rows: Value[][]): string {
  return rows
    .map((row) => {
      const obj: Record<string, unknown> = {};
      columns.forEach((c, i) => {
        obj[c.name] = valueToJson(row[i] ?? { t: "null" });
      });
      return JSON.stringify(obj);
    })
    .join("\n");
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      const ok = document.execCommand("copy");
      ta.remove();
      return ok;
    } catch {
      return false;
    }
  }
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** Rough engine version badge, e.g. "PostgreSQL 16.2" -> "PG 16.2". */
export function engineVersionBadge(engineLabel: string, version: string): string {
  const m = version.match(/(\d+)\.(\d+)/);
  const num = m ? `${m[1]}.${m[2]}` : "";
  const prefix =
    engineLabel === "PostgreSQL"
      ? "PG"
      : engineLabel === "SQLite"
        ? "SQLite"
        : engineLabel === "MariaDB"
          ? "MariaDB"
          : engineLabel === "MySQL"
            ? "MySQL"
            : engineLabel;
  return num ? `${prefix} ${num}` : prefix;
}
