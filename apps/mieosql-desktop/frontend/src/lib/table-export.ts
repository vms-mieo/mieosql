// Direct table export from the connection tree: builds `SELECT * FROM …`
// with engine-correct quoting, asks for a destination file and streams the
// result through the existing `export_result` backend command.

import { save } from "@tauri-apps/plugin-dialog";

import type { Engine, Scope } from "./bindings";
import { api, errorText } from "./api";
import { formatBytes } from "./format";
import { qualified } from "./quoting";
import { useStore } from "./store";
import { runWithTask } from "./tasks";

/** Formats offered by the table context menu (a subset of all export formats). */
export type TableExportFormat = "csv" | "json" | "sql" | "xlsx";

export const TABLE_EXPORT_MENU: { format: TableExportFormat; label: string }[] = [
  { format: "csv", label: "Export CSV…" },
  { format: "json", label: "Export JSON…" },
  { format: "sql", label: "Export SQL INSERTs…" },
  { format: "xlsx", label: "Export Excel (XLSX)…" },
];

const EXPORT_META: Record<TableExportFormat, { ext: string; label: string }> = {
  csv: { ext: "csv", label: "CSV" },
  json: { ext: "json", label: "JSON" },
  sql: { ext: "sql", label: "SQL INSERTs" },
  xlsx: { ext: "xlsx", label: "Excel (XLSX)" },
};

/** Export one table (or view) straight from the tree, with progress toasts. */
export async function exportTableData(
  sessionId: string,
  engine: Engine,
  scope: Scope,
  table: string,
  format: TableExportFormat,
): Promise<void> {
  const toast = useStore.getState().toast;
  const meta = EXPORT_META[format];
  const sql = `SELECT * FROM ${qualified(engine, scope, table)};`;
  const base = table.replace(/[^\w.-]+/g, "_").slice(0, 60);
  const stamp = new Date()
    .toISOString()
    .slice(0, 19)
    .replace(/[:T]/g, "-");

  let path: string | null = null;
  try {
    path = await save({
      title: `Export ${meta.label}`,
      defaultPath: `mieosql-${base}-${stamp}.${meta.ext}`,
      filters: [{ name: meta.label, extensions: [meta.ext] }],
    });
  } catch (err) {
    toast("error", errorText(err));
    return;
  }
  if (!path) return; // cancelled

  try {
    toast("info", `Exporting ${meta.label}…`);
    const stats = await runWithTask(
      `Export ${meta.label} ${table}`,
      (report) => {
        report(path);
        return api.exportResult(sessionId, sql, format, path, table);
      },
      "export",
    );
    toast(
      "success",
      `Exported ${stats.rows.toLocaleString()} rows (${formatBytes(stats.bytes)}) in ${stats.elapsed_ms} ms${
        stats.truncated ? " — truncated at format limit" : ""
      }\n${stats.path}`,
    );
  } catch (err) {
    toast("error", errorText(err));
  }
}
