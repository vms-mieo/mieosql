//! Shared helpers for the driver implementations.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, SecondsFormat, Utc};
use mieosql_core::{ErrorCode, MieoError, MieoResult};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

pub(crate) fn b64_encode(bytes: &[u8]) -> String {
    base64::Engine::encode(&B64, bytes)
}

pub(crate) fn b64_decode(s: &str) -> MieoResult<Vec<u8>> {
    base64::Engine::decode(&B64, s)
        .map_err(|e| MieoError::new(ErrorCode::Data, format!("invalid base64 blob: {e}")))
}

/// MySQL identifier quoting: `` `name` `` with backtick doubling.
pub(crate) fn quote_mysql_ident(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// ANSI (Postgres/SQLite) identifier quoting: `"name"` with quote doubling.
pub(crate) fn quote_ansi_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// SQLite string literal quoting: `'text'` with quote doubling.
pub(crate) fn quote_sqlite_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// SQL string literal quoting: `'text'` with quote doubling. Valid for
/// MySQL / MariaDB / PostgreSQL / SQLite / SQL Server.
pub(crate) fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Expand a leading `~/` to the user's home directory.
pub(crate) fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    path.to_string()
}

pub(crate) fn fmt_naive_date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub(crate) fn fmt_naive_time(t: NaiveTime) -> String {
    t.format("%H:%M:%S%.f").to_string()
}

pub(crate) fn fmt_naive_dt(dt: NaiveDateTime) -> String {
    dt.format("%Y-%m-%d %H:%M:%S%.f").to_string()
}

pub(crate) fn fmt_dt_utc(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Render a Postgres interval the way psql does (`1 year 2 mons 3 days 04:05:06.789`).
pub(crate) fn fmt_interval(months: i32, days: i32, microseconds: i64) -> String {
    let years = months / 12;
    let mons = months % 12;
    let total_secs = microseconds / 1_000_000;
    let micros = microseconds % 1_000_000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    let mut parts: Vec<String> = Vec::new();
    if years != 0 {
        parts.push(format!("{years} year{}", plural(years)));
    }
    if mons != 0 {
        parts.push(format!("{mons} mon{}", plural(mons)));
    }
    if days != 0 {
        parts.push(format!("{days} day{}", plural(days)));
    }
    let mut time = format!("{hours:02}:{minutes:02}:{secs:02}");
    if micros != 0 {
        time.push_str(format!(".{micros:06}").trim_end_matches('0'));
    }
    if microseconds != 0 || parts.is_empty() {
        parts.push(time);
    }
    parts.join(" ")
}

fn plural(n: i32) -> &'static str {
    if n == 1 { "" } else { "s" }
}
