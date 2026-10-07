//! Apache Kafka driver (`rskafka`, pure Rust client).
//!
//! Status: scaffold — the implementation is filled in by the connector pack
//! workstream. Keep the method signatures in sync with [`crate::AnyDriver`].
//!
//! Query language (v1): JSON commands.
//! - Consume: `{ "topic": "orders", "partition": 0, "from": "latest", "limit": 100 }`
//!   (`from` = `"earliest"` | `"latest"` | integer offset)
//! - Produce: `{ "produce": { "topic": "orders", "key": "k", "value": "hello" } }`

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities, ColumnDef, ConnectionSpec, DatabaseRef, Engine, ErrorCode, ForeignKeyDef,
    IndexDef, MieoError, MieoResult, ObjectKind, ObjectRef, PlaceholderStyle, QueryEvent,
    QueryOptions, QueryStats, SchemaRef, Scope, ServerInfo, Value,
};

pub struct KafkaDriver {
    spec: ConnectionSpec,
}

pub(crate) fn kafka_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::MessageBroker,
        placeholder: PlaceholderStyle::Question,
        list_databases: false,
        list_schemas: false,
        schema_is_database: false,
        transactions: false,
        simple_mode: true,
        ddl: false,
        multiple_databases: false,
    }
}

fn pending(what: &str) -> MieoError {
    MieoError::new(
        ErrorCode::Unsupported,
        format!("Kafka {what} is not implemented yet"),
    )
}

impl KafkaDriver {
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
        Ok(Vec::new())
    }

    pub async fn list_schemas(&mut self, database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        let _ = database;
        Ok(Vec::new())
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
        Ok(Vec::new())
    }

    pub async fn table_indexes(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let _ = (scope, table);
        Ok(Vec::new())
    }

    pub async fn foreign_keys(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        let _ = (scope, table);
        Ok(Vec::new())
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
        Ok(())
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

    /// JSON command to fetch the latest messages of a topic (partition 0).
    pub fn preview_statement(&self, scope: &Scope, table: &str, limit: u64) -> MieoResult<String> {
        let _ = scope;
        Ok(format!(
            "{{ \"topic\": \"{table}\", \"partition\": 0, \"from\": \"latest\", \"limit\": {limit} }}"
        ))
    }

    pub fn engine(&self) -> Engine {
        Engine::Kafka
    }
}
