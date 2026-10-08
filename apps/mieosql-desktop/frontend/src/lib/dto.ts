// DTOs for the object-management dialogs (new database / new table / import)
// and backup & restore. Declared locally — the Rust commands return snake_case
// fields, matching the ts-rs codegen style of the generated bindings.
//
// Note: `ImportSummary` here is the CSV/JSON file-import result and is
// unrelated to the workspace-bundle `ImportSummary` in `@bindings`.

import type { Scope } from "@bindings/Scope";
import type { Value } from "@bindings/Value";

export type BatchResult = {
  applied: number;
  failed: number;
  errors: string[];
  elapsed_ms: number;
};

export type ImportOptions = {
  /** Field delimiter; `null`/empty lets the backend auto-detect (CSV only). */
  delimiter?: string | null;
  has_header: boolean;
  /** Text that maps to NULL; the empty string means "no marker". */
  null_marker: string;
  mode: "append" | "truncate";
  batch_size?: number | null;
};

export type ImportSummary = {
  rows_read: number;
  rows_inserted: number;
  rows_failed: number;
  errors: string[];
  elapsed_ms: number;
};

export type ScriptStatements = {
  statements: string[];
  warnings: string[];
};

export type BackupSummary = {
  tables: number;
  rows: number;
  bytes: number;
  errors: string[];
  elapsed_ms: number;
};

// ------------------------------------------------------------------ grid edits

/** One cell in a grid-edit statement: column name + the typed value to bind. */
export type GridCell = { column: string; value: Value };

/** An update: PK cells with their ORIGINAL values + the changed cells. */
export type RowChanges = { keys: GridCell[]; changes: GridCell[] };

export type GridEditRequest = {
  session_id: string;
  scope: Scope;
  table: string;
  inserts: GridCell[][];
  updates: RowChanges[];
  deletes: GridCell[][];
};

export type EditApplyResult = {
  inserted: number;
  updated: number;
  deleted: number;
  failed: number;
  errors: string[];
  elapsed_ms: number;
};
