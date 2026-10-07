//! Ad-hoc row fetch for frontend features that need rows without a query tab
//! (BI dashboards, data previews).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{ColumnMeta, QueryMode, QueryStats, Value};

use crate::dto::ApiResult;
use crate::service::Api;

/// A bounded, fully materialized result set.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CollectedRows {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
    pub stats: QueryStats,
}

impl Api {
    /// Run `sql` and collect up to `max_rows` (default 10 000) into memory.
    /// Intended for small analytical queries (dashboards, previews).
    pub async fn fetch_rows(
        &self,
        session_id: String,
        sql: String,
        max_rows: Option<u32>,
    ) -> ApiResult<CollectedRows> {
        let out = self
            .collect_query(
                &session_id,
                sql,
                Vec::new(),
                QueryMode::Prepared,
                Some(u64::from(max_rows.unwrap_or(10_000).clamp(1, 200_000))),
            )
            .await?;
        Ok(CollectedRows {
            columns: out.columns,
            rows: out.rows,
            stats: out.stats,
        })
    }
}
