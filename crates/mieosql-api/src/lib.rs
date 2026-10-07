//! MieoSQL API layer.
//!
//! Everything the frontend (Tauri commands, CLI, or any future transport)
//! talks to. DTO types derive `ts-rs` so TypeScript bindings are generated
//! from the exact Rust definitions — the contract can never drift.

pub mod dto;
pub mod export;
pub mod service;

pub use dto::{ApiError, ApiResult, PreviewStart, ProfileInput};
pub use export::{ExportFormat, ExportStats};
pub use service::Api;

pub use mieosql_core::API_VERSION;

/// Types exported to TypeScript by `bin/export-ts.rs`.
pub mod export_manifest {
    /// Calls `export_all` for every root type of the public contract.
    pub fn export_all(cfg: &ts_rs::Config) -> Result<(), String> {
        let mut failures: Vec<String> = Vec::new();
        macro_rules! exp {
            ($($t:ty),+ $(,)?) => {
                $(
                    if let Err(e) = <$t as ts_rs::TS>::export_all(cfg) {
                        failures.push(format!("{}: {e:?}", stringify!($t)));
                    }
                )+
            };
        }

        // core domain
        exp!(
            mieosql_core::Value,
            mieosql_core::DataTypeKind,
            mieosql_core::ErrorCode,
            mieosql_core::ErrorInfo,
            mieosql_core::Scope,
            mieosql_core::DatabaseRef,
            mieosql_core::SchemaRef,
            mieosql_core::ObjectKind,
            mieosql_core::ObjectRef,
            mieosql_core::ColumnDef,
            mieosql_core::ColumnMeta,
            mieosql_core::IndexDef,
            mieosql_core::ForeignKeyDef,
            mieosql_core::ServerInfo,
            mieosql_core::PlaceholderStyle,
            mieosql_core::Capabilities,
            mieosql_core::ProfileId,
            mieosql_core::Engine,
            mieosql_core::SslMode,
            mieosql_core::SshAuthKind,
            mieosql_core::SshSpec,
            mieosql_core::ConnectionProfile,
            mieosql_core::QueryMode,
            mieosql_core::QueryOptions,
            mieosql_core::MessageLevel,
            mieosql_core::QueryEvent,
            mieosql_core::QueryStats,
        );

        // engine
        exp!(
            mieosql_engine::SessionInfo,
            mieosql_engine::HistoryEntry,
            mieosql_engine::HistoryStatus,
        );

        // api
        exp!(
            crate::dto::ApiError,
            crate::dto::ProfileInput,
            crate::dto::PreviewStart,
            crate::export::ExportFormat,
            crate::export::ExportStats,
        );

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }
}
