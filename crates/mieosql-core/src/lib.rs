//! MieoSQL core domain.
//!
//! This crate is intentionally driver-agnostic and free of async code.
//! Everything here can be serialized to JSON and consumed by the TypeScript
//! frontend (bindings are generated from these types by `ts-rs`).

pub mod error;
pub mod events;
pub mod model;
pub mod profile;
pub mod value;

pub use error::{ErrorCode, ErrorInfo, MieoError, MieoResult};
pub use events::{
    DEFAULT_FETCH_SIZE, MessageLevel, QueryEvent, QueryMode, QueryOptions, QueryStats,
};
pub use model::{
    Capabilities, ColumnDef, ColumnMeta, DatabaseRef, ForeignKeyDef, IndexDef, ObjectKind,
    ObjectRef, PlaceholderStyle, SchemaRef, Scope, ServerInfo,
};
pub use profile::{
    ConnectionProfile, ConnectionSpec, Engine, ProfileId, SshAuthKind, SshSpec, SslMode,
};
pub use value::{DataTypeKind, Value};

/// Version of the DTO contract exposed to the frontend.
///
/// The frontend can compare this value to detect incompatible backends.
pub const API_VERSION: &str = "0.1.0";
