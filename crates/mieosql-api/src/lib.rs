//! MieoSQL API layer.
//!
//! Everything the frontend (Tauri commands, CLI, or any future transport)
//! talks to. DTO types derive `ts-rs` so TypeScript bindings are generated
//! from the exact Rust definitions — the contract can never drift.

pub mod datagen;
pub mod docs;
pub mod dto;
pub mod explain;
pub mod export;
pub mod fetch;
pub mod files;
pub mod masking;
pub mod search;
pub mod service;
pub mod snapshot;
pub mod sync;
pub mod transfer;
pub mod util;
pub mod workspace;

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

        // v1 feature pack: search / explain / docs / datagen / masking / sync /
        // transfer / snapshot / workspace
        exp!(
            crate::search::SearchRequest,
            crate::search::SearchResponse,
            crate::search::SearchHit,
            crate::search::SearchMatchMode,
            crate::search::SearchHitField,
            crate::explain::ExplainPlan,
            crate::explain::ExplainNode,
            crate::explain::ExplainFormat,
            crate::docs::DocOptions,
            crate::docs::DocFormat,
            crate::docs::DocSummary,
            crate::datagen::GenRequest,
            crate::datagen::GenResult,
            crate::datagen::GenStrategy,
            crate::datagen::GenMode,
            crate::datagen::GenColumnRule,
            crate::masking::MaskExportRequest,
            crate::masking::MaskExportResult,
            crate::masking::MaskStrategy,
            crate::masking::MaskColumnRule,
            crate::sync::SyncEndpoint,
            crate::sync::StructureSyncRequest,
            crate::sync::StructureSyncPlan,
            crate::sync::TableDiff,
            crate::sync::ColumnDiff,
            crate::sync::DiffChange,
            crate::sync::DiffStatus,
            crate::sync::SyncStatement,
            crate::sync::SyncStatementKind,
            crate::sync::SyncSummary,
            crate::sync::SyncApplyResult,
            crate::sync::DataSyncRequest,
            crate::sync::DataSyncPlan,
            crate::sync::DataDiffSummary,
            crate::sync::DataSyncApplyResult,
            crate::transfer::TransferRequest,
            crate::transfer::TransferMode,
            crate::transfer::TransferResult,
            crate::snapshot::SchemaSnapshot,
            crate::snapshot::TableSnapshot,
            crate::workspace::WorkspaceBundle,
            crate::workspace::ImportSummary,
            crate::fetch::CollectedRows,
        );

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }
}
