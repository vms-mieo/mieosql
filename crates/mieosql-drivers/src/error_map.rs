//! Map `sqlx::Error` into the structured `MieoError` used across MieoSQL.

use sqlx::Error as SqlxError;

use mieosql_core::{ErrorCode, MieoError};

pub(crate) fn map_err(e: SqlxError) -> MieoError {
    // Matched by reference so `e` stays available as the error source below.
    match &e {
        SqlxError::Database(db) => {
            let code = db.code().map(|c| c.to_string());
            let message = db.message().to_string();
            let constraint = db.constraint().map(|c| c.to_string());
            let kind = classify(code.as_deref(), &message);

            let mut err = MieoError::new(kind, message);
            if let Some(c) = &code {
                err = err.with_db_code(c.clone());
            }
            if let Some(c) = constraint {
                err = err.with_hint(format!("constraint: {c}"));
            }
            if let Some(pg) = db.try_downcast_ref::<sqlx::postgres::PgDatabaseError>() {
                if let Some(detail) = pg.detail() {
                    err = err.with_detail(detail.to_string());
                }
                if let Some(hint) = pg.hint() {
                    err = err.with_hint(hint.to_string());
                }
                if let Some(sqlx::postgres::PgErrorPosition::Original(pos)) = pg.position() {
                    err = err.with_position(pos as u32);
                }
            }
            err.with_source(e)
        }
        SqlxError::PoolTimedOut => MieoError::new(
            ErrorCode::Timeout,
            "timed out acquiring a connection from the pool (server unreachable or pool exhausted)",
        )
        .with_source(e),
        SqlxError::PoolClosed => {
            MieoError::new(ErrorCode::Connection, "connection pool is closed").with_source(e)
        }
        SqlxError::Io(io) => {
            MieoError::new(ErrorCode::Network, format!("I/O error: {io}")).with_source(e)
        }
        SqlxError::Tls(tls) => {
            MieoError::new(ErrorCode::Network, format!("TLS error: {tls}")).with_source(e)
        }
        SqlxError::RowNotFound => MieoError::new(
            ErrorCode::NotFound,
            "no rows returned where one was expected",
        ),
        SqlxError::Configuration(c) => MieoError::new(
            ErrorCode::Connection,
            format!("invalid connection configuration: {c}"),
        )
        .with_source(e),
        SqlxError::Protocol(msg) => {
            MieoError::new(ErrorCode::Connection, format!("protocol error: {msg}")).with_source(e)
        }
        SqlxError::TypeNotFound { type_name } => MieoError::new(
            ErrorCode::Unsupported,
            format!("type not supported by driver: {type_name}"),
        ),
        SqlxError::ColumnDecode { index, source } => MieoError::new(
            ErrorCode::Data,
            format!("column {index} decode error: {source}"),
        )
        .with_source(e),
        SqlxError::ColumnNotFound(name) => {
            MieoError::new(ErrorCode::NotFound, format!("column not found: {name}"))
        }
        SqlxError::Decode(d) => {
            MieoError::new(ErrorCode::Data, format!("decode error: {d}")).with_source(e)
        }
        SqlxError::AnyDriverError(inner) => MieoError::internal(format!("driver error: {inner}")),
        _ => MieoError::internal(format!("database error: {e}")).with_source(e),
    }
}

/// Classify an engine error from its SQLSTATE-ish code and message text.
fn classify(code: Option<&str>, message: &str) -> ErrorCode {
    let code = code.unwrap_or_default();
    let msg = message.to_ascii_lowercase();

    // SQLSTATE class prefixes (SQL standard, used by both MySQL and Postgres).
    match &code[..code.len().min(2)] {
        "08" => return ErrorCode::Connection,
        "28" => return ErrorCode::Auth,
        "22" => return ErrorCode::Data,
        "23" => return ErrorCode::Constraint,
        "25" => return ErrorCode::Conflict,
        "40" => return ErrorCode::Conflict, // transaction rollback (deadlock/serialization)
        "42" => return ErrorCode::Syntax,
        "53" => return ErrorCode::Connection, // insufficient resources
        "57" => {
            if code == "57014" {
                return ErrorCode::Cancelled;
            }
            return ErrorCode::Connection;
        }
        _ => {}
    }

    // Message heuristics (MySQL often reports with legacy codes).
    if msg.contains("access denied") || msg.contains("authentication failed") {
        return ErrorCode::Auth;
    }
    if msg.contains("can't connect")
        || msg.contains("connection refused")
        || msg.contains("unknown host")
    {
        return ErrorCode::Connection;
    }
    if msg.contains("syntax") || msg.contains("sql error") {
        return ErrorCode::Syntax;
    }
    if msg.contains("duplicate") || msg.contains("foreign key") || msg.contains("constraint") {
        return ErrorCode::Constraint;
    }
    if msg.contains("doesn't exist") || msg.contains("does not exist") || msg.contains("no such") {
        return ErrorCode::NotFound;
    }
    if msg.contains("cancell") || msg.contains("interrupted") {
        return ErrorCode::Cancelled;
    }
    ErrorCode::Internal
}
