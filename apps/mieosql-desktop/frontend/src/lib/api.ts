// Typed wrappers around the Tauri IPC surface.
// Command argument keys are camelCase here; Tauri maps them to snake_case Rust params.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { ApiError } from "@bindings/ApiError";
import type { Capabilities } from "@bindings/Capabilities";
import type { ColumnDef } from "@bindings/ColumnDef";
import type { CollectedRows } from "@bindings/CollectedRows";
import type { ConnectionProfile } from "@bindings/ConnectionProfile";
import type { DatabaseRef } from "@bindings/DatabaseRef";
import type { ExportFormat } from "@bindings/ExportFormat";
import type { ExportStats } from "@bindings/ExportStats";
import type { ForeignKeyDef } from "@bindings/ForeignKeyDef";
import type { HistoryEntry } from "@bindings/HistoryEntry";
import type { IndexDef } from "@bindings/IndexDef";
import type { ObjectKind } from "@bindings/ObjectKind";
import type { ObjectRef } from "@bindings/ObjectRef";
import type { PreviewStart } from "@bindings/PreviewStart";
import type { ProfileInput } from "@bindings/ProfileInput";
import type { QueryEvent } from "@bindings/QueryEvent";
import type { QueryOptions } from "@bindings/QueryOptions";
import type { SchemaRef } from "@bindings/SchemaRef";
import type { Scope } from "@bindings/Scope";
import type { ServerInfo } from "@bindings/ServerInfo";
import type { SessionInfo } from "@bindings/SessionInfo";
import type { Value } from "@bindings/Value";
import type { DataSyncApplyResult } from "@bindings/DataSyncApplyResult";
import type { DataSyncPlan } from "@bindings/DataSyncPlan";
import type { DataSyncRequest } from "@bindings/DataSyncRequest";
import type { DocOptions } from "@bindings/DocOptions";
import type { DocSummary } from "@bindings/DocSummary";
import type { ExplainPlan } from "@bindings/ExplainPlan";
import type { GenRequest } from "@bindings/GenRequest";
import type { GenResult } from "@bindings/GenResult";
import type { ImportSummary } from "@bindings/ImportSummary";
import type { MaskExportRequest } from "@bindings/MaskExportRequest";
import type { MaskExportResult } from "@bindings/MaskExportResult";
import type { SchemaSnapshot } from "@bindings/SchemaSnapshot";
import type { SearchRequest } from "@bindings/SearchRequest";
import type { SearchResponse } from "@bindings/SearchResponse";
import type { StructureSyncPlan } from "@bindings/StructureSyncPlan";
import type { StructureSyncRequest } from "@bindings/StructureSyncRequest";
import type { SyncApplyResult } from "@bindings/SyncApplyResult";
import type { TransferRequest } from "@bindings/TransferRequest";
import type { TransferResult } from "@bindings/TransferResult";
import type { WorkspaceBundle } from "@bindings/WorkspaceBundle";

export type { ApiError, ExportFormat, ExportStats, PreviewStart, QueryEvent, Value };

export const api = {
  // ---------------------------------------------------------------- general
  apiVersion: () => invoke<string>("api_version"),

  // --------------------------------------------------------------- profiles
  listProfiles: () => invoke<ConnectionProfile[]>("list_profiles"),
  getProfile: (id: string) => invoke<ConnectionProfile>("get_profile", { id }),
  saveProfile: (input: ProfileInput) =>
    invoke<ConnectionProfile>("save_profile", { input }),
  deleteProfile: (id: string) => invoke<void>("delete_profile", { id }),
  testProfile: (id: string, password?: string | null) =>
    invoke<ServerInfo>("test_profile", { id, password: password ?? null }),
  testConnection: (input: ProfileInput) =>
    invoke<ServerInfo>("test_connection", { input }),

  // --------------------------------------------------------------- sessions
  openSession: (profileId: string) =>
    invoke<SessionInfo>("open_session", { profileId }),
  closeSession: (sessionId: string) =>
    invoke<void>("close_session", { sessionId }),
  listSessions: () => invoke<SessionInfo[]>("list_sessions"),
  sessionInfo: (sessionId: string) =>
    invoke<SessionInfo>("session_info", { sessionId }),
  capabilities: (sessionId: string) =>
    invoke<Capabilities>("capabilities", { sessionId }),
  serverInfo: (sessionId: string) =>
    invoke<ServerInfo>("server_info", { sessionId }),

  // ---------------------------------------------------------- introspection
  listDatabases: (sessionId: string) =>
    invoke<DatabaseRef[]>("list_databases", { sessionId }),
  listSchemas: (sessionId: string, database?: string | null) =>
    invoke<SchemaRef[]>("list_schemas", { sessionId, database: database ?? null }),
  listObjects: (
    sessionId: string,
    scope: Scope,
    kind?: ObjectKind | null,
    pattern?: string | null,
  ) =>
    invoke<ObjectRef[]>("list_objects", {
      sessionId,
      scope,
      kind: kind ?? null,
      pattern: pattern ?? null,
    }),
  tableColumns: (sessionId: string, scope: Scope, table: string) =>
    invoke<ColumnDef[]>("table_columns", { sessionId, scope, table }),
  tableIndexes: (sessionId: string, scope: Scope, table: string) =>
    invoke<IndexDef[]>("table_indexes", { sessionId, scope, table }),
  foreignKeys: (sessionId: string, scope: Scope, table: string) =>
    invoke<ForeignKeyDef[]>("foreign_keys", { sessionId, scope, table }),
  objectDdl: (sessionId: string, scope: Scope, kind: ObjectKind, name: string) =>
    invoke<string | null>("object_ddl", { sessionId, scope, kind, name }),
  useDatabase: (sessionId: string, database: string) =>
    invoke<void>("use_database", { sessionId, database }),

  // ------------------------------------------------------------ transactions
  beginTransaction: (sessionId: string) =>
    invoke<void>("begin_transaction", { sessionId }),
  commitTransaction: (sessionId: string) =>
    invoke<void>("commit_transaction", { sessionId }),
  rollbackTransaction: (sessionId: string) =>
    invoke<void>("rollback_transaction", { sessionId }),

  // ---------------------------------------------------------------- queries
  startQuery: (
    sessionId: string,
    sql: string,
    params: Value[] = [],
    options?: QueryOptions | null,
  ) =>
    invoke<string>("start_query", {
      sessionId,
      sql,
      params,
      options: options ?? null,
    }),
  previewTable: (sessionId: string, scope: Scope, table: string, limit?: number) =>
    invoke<PreviewStart>("preview_table", {
      sessionId,
      scope,
      table,
      limit: limit ?? null,
    }),
  exportResult: (
    sessionId: string,
    sql: string,
    format: ExportFormat,
    path: string,
    table?: string | null,
  ) =>
    invoke<ExportStats>("export_result", {
      sessionId,
      sql,
      format,
      path,
      table: table ?? null,
    }),
  cancelQuery: (queryId: string) =>
    invoke<boolean>("cancel_query", { queryId }),

  // ---------------------------------------------------------------- history
  listHistory: (limit?: number) =>
    invoke<HistoryEntry[]>("list_history", { limit: limit ?? null }),
  clearHistory: () => invoke<void>("clear_history"),

  // ------------------------------------------------- feature pack v1
  searchDatabase: (sessionId: string, request: SearchRequest) =>
    invoke<SearchResponse>("search_database", { sessionId, request }),
  explainQuery: (sessionId: string, sql: string, analyze: boolean) =>
    invoke<ExplainPlan>("explain_query", { sessionId, sql, analyze }),
  exportDocumentation: (sessionId: string, options: DocOptions, path: string) =>
    invoke<DocSummary>("export_documentation", { sessionId, options, path }),
  generateData: (sessionId: string, request: GenRequest) =>
    invoke<GenResult>("generate_data", { sessionId, request }),
  exportResultMasked: (sessionId: string, request: MaskExportRequest) =>
    invoke<MaskExportResult>("export_result_masked", { sessionId, request }),
  structureSyncPlan: (request: StructureSyncRequest) =>
    invoke<StructureSyncPlan>("structure_sync_plan", { request }),
  applySyncStatements: (sessionId: string, statements: string[]) =>
    invoke<SyncApplyResult>("apply_sync_statements", { sessionId, statements }),
  dataSyncPlan: (request: DataSyncRequest) =>
    invoke<DataSyncPlan>("data_sync_plan", { request }),
  applyDataSync: (request: DataSyncRequest) =>
    invoke<DataSyncApplyResult>("apply_data_sync", { request }),
  transferTable: (request: TransferRequest) =>
    invoke<TransferResult>("transfer_table", { request }),
  schemaSnapshot: (
    sessionId: string,
    scope: Scope,
    includeViews = false,
    maxTables?: number | null,
  ) =>
    invoke<SchemaSnapshot>("schema_snapshot", {
      sessionId,
      scope,
      includeViews,
      maxTables: maxTables ?? null,
    }),
  exportWorkspace: (path: string) =>
    invoke<WorkspaceBundle>("export_workspace", { path }),
  importWorkspace: (path: string, overwrite: boolean) =>
    invoke<ImportSummary>("import_workspace", { path, overwrite }),
  fetchRows: (sessionId: string, sql: string, maxRows?: number | null) =>
    invoke<CollectedRows>("fetch_rows", {
      sessionId,
      sql,
      maxRows: maxRows ?? null,
    }),
  writeTextFile: (path: string, contents: string) =>
    invoke<void>("write_text_file", { path, contents }),
  writeBinaryFile: (path: string, base64Data: string) =>
    invoke<void>("write_binary_file", { path, base64Data }),

  // ----------------------------------------------------------------- events
  onQueryEvent: (
    handler: (queryId: string, event: QueryEvent) => void,
  ): Promise<UnlistenFn> =>
    listen<{ queryId: string; event: QueryEvent }>("mieosql://query", (e) =>
      handler(e.payload.queryId, e.payload.event),
    ),
};

/** Human-friendly error text from an unknown thrown value (usually `ApiError`). */
export function errorText(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) {
    const e = err as Partial<ApiError>;
    const code = e.code ? `[${e.code}] ` : "";
    const hint = e.hint ? ` — ${e.hint}` : "";
    const detail = e.detail ? `\n${e.detail}` : "";
    return `${code}${e.message ?? "Unknown error"}${hint}${detail}`;
  }
  return String(err);
}
