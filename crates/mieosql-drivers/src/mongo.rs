//! MongoDB / AWS DocumentDB driver (official `mongodb` crate).
//!
//! Status: scaffold — the implementation is filled in by the connector pack
//! workstream. Keep the method signatures in sync with [`crate::AnyDriver`].
//!
//! Query language (v1, parsed by this driver):
//! - `db.<collection>.find(<json filter>).limit(<n>)`
//! - `db.<collection>.aggregate(<json array>)`
//! - `db.<collection>.countDocuments(<json filter>)`
//! - `{ "collection": "<name>", "find": { ... }, "limit": 100 }` (JSON form)

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities, ColumnDef, ConnectionSpec, DatabaseRef, Engine, ErrorCode, ForeignKeyDef,
    IndexDef, MieoError, MieoResult, ObjectKind, ObjectRef, PlaceholderStyle, QueryEvent,
    QueryOptions, QueryStats, SchemaRef, Scope, ServerInfo, Value,
};

pub struct MongoDriver {
    spec: ConnectionSpec,
}

pub(crate) fn mongo_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::Document,
        placeholder: PlaceholderStyle::Question,
        list_databases: true,
        list_schemas: false,
        schema_is_database: false,
        transactions: false,
        simple_mode: true,
        ddl: false,
        multiple_databases: true,
    }
}

fn pending(what: &str) -> MieoError {
    MieoError::new(
        ErrorCode::Unsupported,
        format!("MongoDB {what} is not implemented yet"),
    )
}

impl MongoDriver {
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
        Ok(None)
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        let _ = database;
        Err(pending("use_database"))
    }

    pub async fn begin(&mut self) -> MieoResult<()> {
        Err(pending("transactions"))
    }

    pub async fn commit(&mut self) -> MieoResult<()> {
        Err(pending("transactions"))
    }

    pub async fn rollback(&mut self) -> MieoResult<()> {
        Err(pending("transactions"))
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

    /// `db.<collection>.find({}).limit(n)` preview.
    pub fn preview_statement(&self, scope: &Scope, table: &str, limit: u64) -> MieoResult<String> {
        let _ = scope;
        Ok(format!("db.{table}.find({{}}).limit({limit})"))
    }

    pub fn engine(&self) -> Engine {
        Engine::MongoDb
    }
}
