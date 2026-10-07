//! Generic streaming query executor shared by all sqlx-backed drivers.
//!
//! The driver-specific parts (parameter binding, row decoding, native type
//! mapping) are injected as function pointers, so this module stays completely
//! engine-agnostic while producing identical `QueryEvent` streams.

use std::time::{Duration, Instant};

use futures::StreamExt;
use sqlx::query::Query;
use sqlx::{
    Column, Database, Either, Executor, IntoArguments, Row, SqlSafeStr, Statement, TypeInfo,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    ColumnMeta, DataTypeKind, ErrorCode, MieoError, MieoResult, QueryEvent, QueryMode,
    QueryOptions, QueryStats, Value,
};

use crate::error_map::map_err;

/// `sqlx` 0.9 dropped the shared `QueryResult` trait; we bridge the concrete
/// result types to a uniform interface here.
pub(crate) trait ResultExt {
    fn result_rows_affected(&self) -> u64;
    fn result_last_insert_id(&self) -> Option<u64>;
}

impl ResultExt for sqlx::mysql::MySqlQueryResult {
    fn result_rows_affected(&self) -> u64 {
        self.rows_affected()
    }
    fn result_last_insert_id(&self) -> Option<u64> {
        let id = self.last_insert_id();
        (id != 0).then_some(id)
    }
}

impl ResultExt for sqlx::postgres::PgQueryResult {
    fn result_rows_affected(&self) -> u64 {
        self.rows_affected()
    }
    fn result_last_insert_id(&self) -> Option<u64> {
        None
    }
}

impl ResultExt for sqlx::sqlite::SqliteQueryResult {
    fn result_rows_affected(&self) -> u64 {
        self.rows_affected()
    }
    fn result_last_insert_id(&self) -> Option<u64> {
        let id = self.last_insert_rowid();
        (id != 0).then_some(id as u64)
    }
}

/// Binds a list of values onto a query, engine-specifically.
pub(crate) type Binder<DB> = for<'q> fn(
    Query<'q, DB, <DB as Database>::Arguments>,
    &[Value],
) -> MieoResult<Query<'q, DB, <DB as Database>::Arguments>>;

/// Decodes one wire row into wire-safe values.
pub(crate) type Decoder<DB> = fn(&<DB as Database>::Row) -> Vec<Value>;

/// Maps a native type name to a normalized `DataTypeKind`.
pub(crate) type KindFn = fn(&str) -> DataTypeKind;

async fn send_evt(sink: &mpsc::Sender<QueryEvent>, evt: QueryEvent) -> MieoResult<()> {
    sink.send(evt).await.map_err(|_| MieoError::cancelled())
}

/// Execute `sql` on `conn`, streaming `Columns`/`Batch`/`Affected` events.
///
/// When `max_rows` is reached the remaining rows are drained (but neither
/// decoded nor sent) so the connection stays protocol-clean.
pub(crate) async fn stream_query<DB>(
    conn: &mut DB::Connection,
    query_id: &str,
    sql: String,
    params: Vec<Value>,
    options: &QueryOptions,
    sink: &mpsc::Sender<QueryEvent>,
    cancel: &CancellationToken,
    bind: Binder<DB>,
    decode: Decoder<DB>,
    kind_of: KindFn,
) -> MieoResult<QueryStats>
where
    DB: Database,
    DB::QueryResult: ResultExt,
    <DB as Database>::Arguments: IntoArguments<DB> + Send,
    for<'a> &'a mut DB::Connection: Executor<'a, Database = DB>,
    DB::Row: Send,
{
    let start = Instant::now();
    let fetch_size = options.fetch_size.max(1) as usize;
    let deadline = options
        .timeout_ms
        .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));

    // Audit note: `sql` here is user-authored SQL from the query editor — the
    // whole point of this application — hence the explicit `AssertSqlSafe`.
    let sql: sqlx::SqlStr = sqlx::AssertSqlSafe(sql).into_sql_str();

    let mut stats = QueryStats::default();
    let mut batch: Vec<Vec<Value>> = Vec::with_capacity(fetch_size.min(4096));
    let mut batch_index: u64 = 0;
    let mut current_sig: Vec<String> = Vec::new();
    let mut truncated = false;

    // In prepared mode, describe the statement first so the UI receives column
    // metadata even when the result set is empty.
    if options.mode == QueryMode::Prepared
        && let Ok(stmt) = (&mut *conn).prepare(sql.clone()).await
    {
        let columns: Vec<ColumnMeta> = stmt
            .columns()
            .iter()
            .enumerate()
            .map(|(i, c)| ColumnMeta {
                name: c.name().to_string(),
                ordinal: i as u32,
                native_type: c.type_info().name().to_string(),
                kind: kind_of(c.type_info().name()),
                nullable: None,
            })
            .collect();
        if !columns.is_empty() {
            current_sig = columns.iter().map(|c| c.name.clone()).collect();
            send_evt(
                sink,
                QueryEvent::Columns {
                    query_id: query_id.to_string(),
                    columns,
                },
            )
            .await?;
        }
    }

    let mut rows = if options.mode == QueryMode::Simple {
        if !params.is_empty() {
            return Err(MieoError::new(
                ErrorCode::Data,
                "bound parameters are not supported in simple (unprepared) mode",
            ));
        }
        sqlx::raw_sql(sql.clone()).fetch_many(&mut *conn)
    } else {
        let query = sqlx::query::<DB>(sql.clone());
        let query = bind(query, &params)?;
        // `fetch_many` is the only API that yields both `QueryResult` and rows
        // for a single statement — exactly what a SQL client needs. The
        // deprecation targets multi-statement prepared queries, which we never
        // issue in prepared mode.
        #[allow(deprecated)]
        query.fetch_many(&mut *conn)
    };

    loop {
        let next = if let Some(deadline) = deadline {
            let delay = tokio::time::sleep_until(deadline);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(MieoError::cancelled()),
                _ = delay => {
                    return Err(MieoError::new(ErrorCode::Timeout, "query timed out"));
                }
                next = rows.next() => next,
            }
        } else {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(MieoError::cancelled()),
                next = rows.next() => next,
            }
        };

        let Some(item) = next else { break };
        let item = item.map_err(map_err)?;

        match item {
            Either::Left(result) => {
                // Deferred: emitted after the stream ends, and only when a
                // statement actually affected rows. Some drivers (SQLite) emit
                // a zero-row result for plain SELECTs, which must not surface
                // as an "affected rows" event in the UI.
                stats.rows_affected = result.result_rows_affected();
                stats.last_insert_id = result.result_last_insert_id();
            }
            Either::Right(row) => {
                let within_limit = options.max_rows.is_none_or(|m| stats.rows < m);
                if within_limit {
                    // Simple mode: the result set can change between batches.
                    let sig: Vec<String> =
                        row.columns().iter().map(|c| c.name().to_string()).collect();
                    if sig != current_sig {
                        current_sig = sig;
                        let columns: Vec<ColumnMeta> = row
                            .columns()
                            .iter()
                            .enumerate()
                            .map(|(i, c)| ColumnMeta {
                                name: c.name().to_string(),
                                ordinal: i as u32,
                                native_type: c.type_info().name().to_string(),
                                kind: kind_of(c.type_info().name()),
                                nullable: None,
                            })
                            .collect();
                        send_evt(
                            sink,
                            QueryEvent::Columns {
                                query_id: query_id.to_string(),
                                columns,
                            },
                        )
                        .await?;
                    }

                    stats.rows += 1;
                    batch.push(decode(&row));
                    if batch.len() >= fetch_size {
                        batch_index += 1;
                        send_evt(
                            sink,
                            QueryEvent::Batch {
                                query_id: query_id.to_string(),
                                rows: std::mem::take(&mut batch),
                                batch_index,
                            },
                        )
                        .await?;
                        batch.reserve(fetch_size.min(4096));
                    }
                } else {
                    // Drain silently to keep the connection protocol-clean.
                    truncated = true;
                }
            }
        }
    }

    if stats.rows_affected > 0 {
        send_evt(
            sink,
            QueryEvent::Affected {
                query_id: query_id.to_string(),
                rows_affected: stats.rows_affected,
                last_insert_id: stats.last_insert_id,
            },
        )
        .await?;
    }

    if !batch.is_empty() {
        batch_index += 1;
        send_evt(
            sink,
            QueryEvent::Batch {
                query_id: query_id.to_string(),
                rows: batch,
                batch_index,
            },
        )
        .await?;
    }

    stats.truncated = truncated;
    stats.elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(stats)
}
