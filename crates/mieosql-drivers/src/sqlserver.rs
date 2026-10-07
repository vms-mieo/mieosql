//! SQL Server driver (tiberius, pure Rust TDS).
//!
//! Status: scaffold — the implementation is filled in by the connector pack
//! workstream. Keep the method signatures in sync with [`crate::AnyDriver`].

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities, ColumnDef, ConnectionSpec, DatabaseRef, Engine, ErrorCode, ForeignKeyDef,
    IndexDef, MieoError, MieoResult, ObjectKind, ObjectRef, PlaceholderStyle, QueryEvent,
    QueryOptions, QueryStats, SchemaRef, Scope, ServerInfo, Value,
};

pub struct SqlServerDriver {
    spec: ConnectionSpec,
}

pub(crate) fn sqlserver_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::Sql,
        // `?` positional placeholders are rewritten to `@P1..` internally.
        placeholder: PlaceholderStyle::Question,
        list_databases: true,
        list_schemas: true,
        schema_is_database: false,
        transactions: true,
        simple_mode: true,
        ddl: true,
        multiple_databases: true,
    }
}

fn pending(what: &str) -> MieoError {
    MieoError::new(
        ErrorCode::Unsupported,
        format!("SQL Server {what} is not implemented yet"),
    )
}

impl SqlServerDriver {
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        let _ = spec;
        Err(pending("connect"))
    }

    pub fn spec(&self) -> &ConnectionSpec {
        &self.spec
    }

    pub fn in_transaction(&self) -> bool {
        false
    }

    pub async fn ping(&mut self) -> MieoResult<()> {
        Err(pending("ping"))
    }

    pub async fn server_info(&mut self) -> MieoResult<ServerInfo> {
        Err(pending("server_info"))
    }

    pub async fn list_databases(&mut self) -> MieoResult<Vec<DatabaseRef>> {
        Err(pending("list_databases"))
    }

    pub async fn list_schemas(&mut self, database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        let _ = database;
        Err(pending("list_schemas"))
    }

    pub async fn list_objects(
        &mut self,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        let _ = (scope, kind, pattern);
        Err(pending("list_objects"))
    }

    pub async fn table_columns(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ColumnDef>> {
        let _ = (scope, table);
        Err(pending("table_columns"))
    }

    pub async fn table_indexes(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let _ = (scope, table);
        Err(pending("table_indexes"))
    }

    pub async fn foreign_keys(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        let _ = (scope, table);
        Err(pending("foreign_keys"))
    }

    pub async fn object_ddl(
        &mut self,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let _ = (scope, kind, name);
        Err(pending("object_ddl"))
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        let _ = database;
        Err(pending("use_database"))
    }

    pub async fn begin(&mut self) -> MieoResult<()> {
        Err(pending("begin"))
    }

    pub async fn commit(&mut self) -> MieoResult<()> {
        Err(pending("commit"))
    }

    pub async fn rollback(&mut self) -> MieoResult<()> {
        Err(pending("rollback"))
    }

    pub async fn run_query(
        &mut self,
        query_id: &str,
        sql: String,
        params: Vec<Value>,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        let _ = (query_id, sql, params, options, sink, cancel);
        Err(pending("query"))
    }

    /// `SELECT * FROM [schema].[table]` preview used by "Open data grid".
    pub fn preview_statement(&self, scope: &Scope, table: &str, limit: u64) -> MieoResult<String> {
        let _ = (scope, table, limit);
        Err(pending("preview"))
    }

    pub fn engine(&self) -> Engine {
        Engine::SqlServer
    }
}
