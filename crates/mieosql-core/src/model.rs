//! Schema / structural model shared by all engines.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::value::DataTypeKind;

/// Identifies where an object lives.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Scope {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub schema: Option<String>,
}

impl Scope {
    pub fn db(database: impl Into<String>) -> Self {
        Self {
            database: Some(database.into()),
            schema: None,
        }
    }

    pub fn db_schema(database: impl Into<String>, schema: impl Into<String>) -> Self {
        Self {
            database: Some(database.into()),
            schema: Some(schema.into()),
        }
    }
}

/// A database (MySQL) or a database inside a server (Postgres/SQL Server).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DatabaseRef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub charset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub collation: Option<String>,
    /// System database (information_schema, mysql, postgres, ...) — UI may hide these.
    #[serde(default)]
    pub is_system: bool,
}

/// A schema inside a database (Postgres). For MySQL "schema" == "database".
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SchemaRef {
    pub name: String,
    #[serde(default)]
    pub is_system: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
    Function,
    Procedure,
    Trigger,
    Sequence,
}

/// A table / view / function / ... in the explorer tree.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ObjectRef {
    pub name: String,
    pub kind: ObjectKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
    /// Approximate row count (from catalog statistics) for tables.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub rows_estimate: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_at: Option<String>,
}

/// Column definition from the catalog (`information_schema` / `PRAGMA`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ColumnDef {
    pub name: String,
    pub ordinal: u32,
    /// Full native type text, e.g. `varchar(255)`, `numeric(12,4)`, `INTEGER`.
    pub native_type: String,
    pub data_type: DataTypeKind,
    pub nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub is_primary_key: bool,
    #[serde(default)]
    pub is_auto_increment: bool,
    #[serde(default)]
    pub is_generated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub char_max_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub numeric_precision: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub numeric_scale: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub collation: Option<String>,
}

/// Lightweight column metadata for **query results** (no catalog round-trip).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ColumnMeta {
    pub name: String,
    pub ordinal: u32,
    /// Engine-native type name as reported by the wire protocol.
    pub native_type: String,
    pub kind: DataTypeKind,
    /// Usually unknown for ad-hoc query results; filled when the engine tells us.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub nullable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    #[serde(default)]
    pub is_unique: bool,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub index_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForeignKeyDef {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_table: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ref_schema: Option<String>,
    pub ref_columns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub on_update: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub on_delete: Option<String>,
}

/// Server identity + runtime info shown in the UI status bar.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServerInfo {
    pub engine: crate::profile::Engine,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version_comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub current_database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub current_user: Option<String>,
}

/// Placeholder style for parameterized statements, engine-specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlaceholderStyle {
    /// MySQL / MariaDB / SQLite: `?`
    Question,
    /// PostgreSQL: `$1`, `$2`, ...
    DollarNumbered,
}

/// What a driver can do. The UI uses this to hide unsupported affordances.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Capabilities {
    /// Broad category of the engine (SQL / document / key-value / broker).
    pub engine_kind: crate::profile::EngineKind,
    pub placeholder: PlaceholderStyle,
    /// Can enumerate databases on the server (MySQL `SHOW DATABASES`).
    pub list_databases: bool,
    /// Has schemas within a database (Postgres).
    pub list_schemas: bool,
    /// For MySQL-like engines schema == database.
    pub schema_is_database: bool,
    pub transactions: bool,
    /// Supports unprepared "simple" execution mode (DDL, SET, USE, ...).
    pub simple_mode: bool,
    /// Can produce object DDL.
    pub ddl: bool,
    /// Engine supports multiple databases per server connection.
    pub multiple_databases: bool,
    /// Engine exposes users/roles/grants management.
    pub user_management: bool,
}
