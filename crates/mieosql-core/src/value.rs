//! The value model: a lossless-enough, JSON-safe representation of any cell
//! coming out of (or going into) any supported database engine.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A single database value, tagged for the wire.
///
/// Design rules:
/// - `Decimal` is a string — never lose precision for money/numeric types.
/// - `Bytes` is base64 — BLOBs survive JSON transport.
/// - `DateTime` is RFC 3339 when a timezone is known, naive otherwise.
/// - Unknown / undecodable values never fail a query: they become
///   `Unsupported` so the UI can render them gracefully.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
#[ts(export)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    /// Signed 64-bit integer. Serialized as a string so values beyond
    /// `Number.MAX_SAFE_INTEGER` survive the JSON bridge to the UI.
    Int(
        #[serde(with = "i64_str")]
        #[ts(type = "string")]
        i64,
    ),
    /// Unsigned 64-bit integer (MySQL `BIGINT UNSIGNED`, Postgres `oid`, ...).
    /// Serialized as a string for the same reason as `Int`.
    #[serde(rename = "uint")]
    UInt(
        #[serde(with = "u64_str")]
        #[ts(type = "string")]
        u64,
    ),
    Float(f64),
    /// Exact decimal, stored as string to preserve scale and precision.
    Decimal(String),
    Text(String),
    /// Binary data, base64-encoded.
    Bytes(String),
    /// `YYYY-MM-DD`.
    Date(String),
    /// `HH:MM:SS[.fff]`.
    Time(String),
    /// RFC 3339 with offset when known, otherwise `YYYY-MM-DD HH:MM:SS[.fff]`.
    #[serde(rename = "datetime")]
    DateTime(String),
    Json(serde_json::Value),
    Uuid(String),
    /// A database array (e.g. Postgres `int4[]`).
    Array(Vec<Value>),
    /// Value could not be decoded; `native_type` keeps the raw engine type name.
    Unsupported {
        native_type: String,
        reason: String,
    },
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::UInt(_) => "uint",
            Value::Float(_) => "float",
            Value::Decimal(_) => "decimal",
            Value::Text(_) => "text",
            Value::Bytes(_) => "bytes",
            Value::Date(_) => "date",
            Value::Time(_) => "time",
            Value::DateTime(_) => "datetime",
            Value::Json(_) => "json",
            Value::Uuid(_) => "uuid",
            Value::Array(_) => "array",
            Value::Unsupported { .. } => "unsupported",
        }
    }

    pub fn text(s: impl Into<String>) -> Value {
        Value::Text(s.into())
    }

    /// Best-effort textual rendering, used by export and CLI display.
    pub fn to_display_string(&self) -> String {
        match self {
            Value::Null => "NULL".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::UInt(u) => u.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Decimal(d) | Value::Text(d) => d.clone(),
            Value::Bytes(b) => format!("<{} bytes>", base64_decoded_len(b)),
            Value::Date(d) => d.clone(),
            Value::Time(t) => t.clone(),
            Value::DateTime(t) => t.clone(),
            Value::Json(j) => j.to_string(),
            Value::Uuid(u) => u.clone(),
            Value::Array(items) => {
                let inner: Vec<String> = items.iter().map(|v| v.to_display_string()).collect();
                format!("[{}]", inner.join(", "))
            }
            Value::Unsupported { native_type, .. } => format!("<{native_type}>"),
        }
    }
}

fn base64_decoded_len(s: &str) -> usize {
    // len * 3 / 4 minus padding; good enough for a display hint.
    let n = s.len();
    let padding = s.bytes().rev().take_while(|b| *b == b'=').count();
    n.saturating_mul(3) / 4 - padding
}

/// Serde helpers: 64-bit integers travel as strings, and accept both string
/// and number forms when deserializing (hand-written JSON, tests, CLI input).
mod i64_str {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &i64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Str(String),
            Num(i64),
        }
        match Repr::deserialize(deserializer)? {
            Repr::Str(s) => s.parse().map_err(serde::de::Error::custom),
            Repr::Num(n) => Ok(n),
        }
    }
}

mod u64_str {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Str(String),
            Num(u64),
        }
        match Repr::deserialize(deserializer)? {
            Repr::Str(s) => s.parse().map_err(serde::de::Error::custom),
            Repr::Num(n) => Ok(n),
        }
    }
}

/// Normalized data type across engines, used for column metadata and to drive
/// decode strategies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DataTypeKind {
    Bool,
    Int,
    #[serde(rename = "uint")]
    UInt,
    Float,
    Decimal,
    Text,
    Bytes,
    Date,
    Time,
    #[serde(rename = "datetime")]
    DateTime,
    /// `TIMESTAMP WITH TIME ZONE` (Postgres) / `TIMESTAMP` (MySQL aliases differ).
    #[serde(rename = "timestamptz")]
    TimestampTz,
    Json,
    Uuid,
    Bit,
    Interval,
    Enum,
    Set,
    Geometry,
    /// Element kind of an array type, e.g. Postgres `int4[]` -> `Array(Int)`.
    Array(Box<DataTypeKind>),
    Unknown,
}

impl DataTypeKind {
    pub fn is_array(&self) -> bool {
        matches!(self, DataTypeKind::Array(_))
    }
}
