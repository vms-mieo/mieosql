//! MieoSQL driver layer.
//!
//! `AnyDriver` is a static-dispatch enum over the concrete engine drivers.
//! It is the only type the engine layer talks to, keeping all sqlx usage
//! contained in this crate.

mod error_map;
mod kafka;
mod mongo;
mod mysql;
mod postgres;
mod redis;
mod sqlite;
mod sqlserver;
mod stream;
mod util;

pub use kafka::KafkaDriver;
pub use mongo::MongoDriver;
pub use mysql::MySqlDriver;
pub use postgres::PostgresDriver;
pub use redis::RedisDriver;
pub use sqlite::SqliteDriver;
pub use sqlserver::SqlServerDriver;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities, ColumnDef, ConnectionSpec, DatabaseRef, Engine, ForeignKeyDef, IndexDef,
    MieoError, MieoResult, ObjectKind, ObjectRef, QueryEvent, QueryOptions, QueryStats, SchemaRef,
    Scope, ServerInfo, Value,
};

/// Dispatches a call to the active engine driver.
macro_rules! dispatch {
    ($self:expr, $d:ident => $body:expr) => {
        match $self {
            AnyDriver::MySql($d) => $body,
            AnyDriver::Postgres($d) => $body,
            AnyDriver::Sqlite($d) => $body,
            AnyDriver::SqlServer($d) => $body,
            AnyDriver::Mongo($d) => $body,
            AnyDriver::Redis($d) => $body,
            AnyDriver::Kafka($d) => $body,
        }
    };
}

/// An open, engine-agnostic database connection.
pub enum AnyDriver {
    MySql(MySqlDriver),
    Postgres(PostgresDriver),
    Sqlite(SqliteDriver),
    SqlServer(SqlServerDriver),
    Mongo(MongoDriver),
    Redis(RedisDriver),
    Kafka(KafkaDriver),
}

impl AnyDriver {
    /// Open a connection according to `spec`.
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        match spec.engine {
            Engine::MySql | Engine::MariaDb => {
                Ok(AnyDriver::MySql(MySqlDriver::connect(spec).await?))
            }
            Engine::Postgres => Ok(AnyDriver::Postgres(PostgresDriver::connect(spec).await?)),
            Engine::Sqlite => Ok(AnyDriver::Sqlite(SqliteDriver::connect(spec).await?)),
            Engine::SqlServer => {
                Ok(AnyDriver::SqlServer(SqlServerDriver::connect(spec).await?))
            }
            Engine::MongoDb => Ok(AnyDriver::Mongo(MongoDriver::connect(spec).await?)),
            Engine::Redis => Ok(AnyDriver::Redis(RedisDriver::connect(spec).await?)),
            Engine::Kafka => Ok(AnyDriver::Kafka(KafkaDriver::connect(spec).await?)),
            other => Err(MieoError::unsupported(format!(
                "engine `{}` is not supported by this build",
                other.label()
            ))),
        }
    }

    pub fn engine(&self) -> Engine {
        dispatch!(self, d => d.spec().engine)
    }

    pub fn capabilities(&self) -> Capabilities {
        match self {
            AnyDriver::MySql(_) => mysql::mysql_capabilities(),
            AnyDriver::Postgres(_) => postgres::postgres_capabilities(),
            AnyDriver::Sqlite(_) => sqlite::sqlite_capabilities(),
            AnyDriver::SqlServer(_) => sqlserver::sqlserver_capabilities(),
            AnyDriver::Mongo(_) => mongo::mongo_capabilities(),
            AnyDriver::Redis(_) => redis::redis_capabilities(),
            AnyDriver::Kafka(_) => kafka::kafka_capabilities(),
        }
    }

    pub fn in_transaction(&self) -> bool {
        dispatch!(self, d => d.in_transaction())
    }

    pub async fn ping(&mut self) -> MieoResult<()> {
        dispatch!(self, d => d.ping().await)
    }

    pub async fn server_info(&mut self) -> MieoResult<ServerInfo> {
        dispatch!(self, d => d.server_info().await)
    }

    pub async fn list_databases(&mut self) -> MieoResult<Vec<DatabaseRef>> {
        dispatch!(self, d => d.list_databases().await)
    }

    pub async fn list_schemas(&mut self, database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        dispatch!(self, d => d.list_schemas(database).await)
    }

    pub async fn list_objects(
        &mut self,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        dispatch!(self, d => d.list_objects(scope, kind, pattern).await)
    }

    pub async fn table_columns(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ColumnDef>> {
        dispatch!(self, d => d.table_columns(scope, table).await)
    }

    pub async fn table_indexes(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        dispatch!(self, d => d.table_indexes(scope, table).await)
    }

    pub async fn foreign_keys(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        dispatch!(self, d => d.foreign_keys(scope, table).await)
    }

    pub async fn object_ddl(
        &mut self,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        dispatch!(self, d => d.object_ddl(scope, kind, name).await)
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        dispatch!(self, d => d.use_database(database).await)
    }

    pub async fn begin(&mut self) -> MieoResult<()> {
        dispatch!(self, d => d.begin().await)
    }

    pub async fn commit(&mut self) -> MieoResult<()> {
        dispatch!(self, d => d.commit().await)
    }

    pub async fn rollback(&mut self) -> MieoResult<()> {
        dispatch!(self, d => d.rollback().await)
    }

    /// Execute a statement, streaming `Columns` / `Batch` / `Affected` events
    /// into `sink` until completion or cancellation.
    pub async fn run_query(
        &mut self,
        query_id: &str,
        sql: String,
        params: Vec<Value>,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        dispatch!(self, d => d.run_query(query_id, sql, params, options, sink, cancel).await)
    }

    /// Engine-appropriate "open data grid" statement: a SQL `SELECT` for SQL
    /// engines (generated by the API layer) or the native preview syntax for
    /// NoSQL engines.
    pub fn preview_statement(&self, scope: &Scope, table: &str, limit: u64) -> MieoResult<String> {
        match self {
            AnyDriver::Mongo(d) => d.preview_statement(scope, table, limit),
            AnyDriver::Redis(d) => d.preview_statement(scope, table, limit),
            AnyDriver::Kafka(d) => d.preview_statement(scope, table, limit),
            _ => Err(MieoError::unsupported(
                "SQL preview statements are generated by the API layer",
            )),
        }
    }
}
