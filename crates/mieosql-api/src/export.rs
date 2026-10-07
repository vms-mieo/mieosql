//! Result export: stream query results straight to disk.
//!
//! Supported formats: CSV, TSV (plain text), JSON, JSON Lines, SQL INSERTs and
//! Excel (XLSX). Rows are written batch-by-batch as the query streams — the
//! whole result never has to fit in memory.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use ts_rs::TS;

use mieosql_core::{ColumnMeta, DataTypeKind, Engine, ErrorCode, MieoError, MieoResult, Value};

use crate::service::quote_ident;

/// Excel hard limits.
const XLSX_MAX_ROWS: u64 = 1_048_575; // data rows + 1 header row
const XLSX_MAX_COLS: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExportFormat {
    Csv,
    /// Plain text, tab-separated.
    Tsv,
    /// JSON array of row objects.
    Json,
    /// One JSON object per line.
    JsonLines,
    /// `INSERT INTO ... VALUES ...;` statements.
    Sql,
    /// Excel workbook (requires the "xlsx" feature of this build).
    Xlsx,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json => "json",
            ExportFormat::JsonLines => "jsonl",
            ExportFormat::Sql => "sql",
            ExportFormat::Xlsx => "xlsx",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Tsv => "Text (TSV)",
            ExportFormat::Json => "JSON",
            ExportFormat::JsonLines => "JSON Lines",
            ExportFormat::Sql => "SQL INSERTs",
            ExportFormat::Xlsx => "Excel (XLSX)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExportStats {
    /// Rows written to the file.
    #[ts(type = "number")]
    pub rows: u64,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    /// True when a format limit (e.g. Excel row cap) cut the export short.
    pub truncated: bool,
    pub path: String,
}

// ---------------------------------------------------------------------------
// Exporter
// ---------------------------------------------------------------------------

enum Sink {
    Text(BufWriter<File>),
    Xlsx {
        worksheet: Box<rust_xlsxwriter::Worksheet>,
        row: u32,
    },
}

pub struct ResultExporter {
    format: ExportFormat,
    engine: Engine,
    table: String,
    path: PathBuf,
    columns: Vec<ColumnMeta>,
    header_written: bool,
    json_first: bool,
    rows: u64,
    truncated: bool,
    sink: Option<Sink>,
}

impl ResultExporter {
    pub fn create(
        format: ExportFormat,
        engine: Engine,
        table: String,
        path: PathBuf,
    ) -> MieoResult<Self> {
        // Fail fast if the destination is not writable.
        let file = File::create(&path)
            .map_err(|e| MieoError::new(ErrorCode::Data, format!("cannot create file: {e}")))?;
        let sink = match format {
            ExportFormat::Xlsx => Sink::Xlsx {
                worksheet: Box::new(rust_xlsxwriter::Worksheet::new()),
                row: 0,
            },
            _ => Sink::Text(BufWriter::new(file)),
        };
        Ok(Self {
            format,
            engine,
            table,
            path,
            columns: Vec::new(),
            header_written: false,
            json_first: true,
            rows: 0,
            truncated: false,
            sink: Some(sink),
        })
    }

    pub fn set_columns(&mut self, columns: Vec<ColumnMeta>) -> MieoResult<()> {
        self.columns = columns;
        self.write_header()
    }

    fn write_header(&mut self) -> MieoResult<()> {
        if self.header_written || self.columns.is_empty() {
            return Ok(());
        }
        self.header_written = true;
        let names: Vec<String> = self.columns.iter().map(|c| c.name.clone()).collect();
        match self.sink.as_mut().expect("sink present") {
            Sink::Text(w) => match self.format {
                ExportFormat::Csv => {
                    // UTF-8 BOM so Excel opens the file with the right encoding.
                    w.write_all("\u{FEFF}".as_bytes())?;
                    writeln!(w, "{}", csv_line(&names, ','))?;
                }
                ExportFormat::Tsv => writeln!(w, "{}", csv_line(&names, '\t'))?,
                ExportFormat::Sql => {
                    writeln!(
                        w,
                        "-- MieoSQL export · {} · {} column(s)",
                        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                        names.len()
                    )?;
                }
                ExportFormat::Json => {}
                ExportFormat::JsonLines => {}
                ExportFormat::Xlsx => unreachable!(),
            },
            Sink::Xlsx { worksheet, row } => {
                let bold = rust_xlsxwriter::Format::new().set_bold();
                for (col, name) in names.iter().enumerate() {
                    if col >= XLSX_MAX_COLS {
                        break;
                    }
                    worksheet
                        .write_string_with_format(*row, col as u16, name, &bold)
                        .map_err(xlsx_err)?;
                    let width = (name.chars().count() as f64 + 6.0).clamp(12.0, 40.0);
                    let _ = worksheet.set_column_width(col as u16, width);
                }
                *row += 1;
            }
        }
        Ok(())
    }

    pub fn write_rows(&mut self, rows: &[Vec<Value>]) -> MieoResult<()> {
        if !self.header_written {
            self.write_header()?;
        }
        for row in rows {
            self.rows += 1;
            match self.sink.as_mut().expect("sink present") {
                Sink::Text(w) => {
                    let line_flags = self.format;
                    let engine = self.engine;
                    match line_flags {
                        ExportFormat::Csv | ExportFormat::Tsv => {
                            let delim = if line_flags == ExportFormat::Csv {
                                ','
                            } else {
                                '\t'
                            };
                            let cells: Vec<String> = row.iter().map(cell_text).collect();
                            writeln!(w, "{}", csv_line(&cells, delim))?;
                        }
                        ExportFormat::JsonLines => {
                            let obj = row_object(&self.columns, row);
                            writeln!(w, "{obj}")?;
                        }
                        ExportFormat::Json => {
                            let obj = row_object(&self.columns, row);
                            if self.json_first {
                                self.json_first = false;
                                writeln!(w, "[")?;
                            } else {
                                writeln!(w, ",")?;
                            }
                            write!(w, "{obj}")?;
                        }
                        ExportFormat::Sql => {
                            let columns: Vec<String> = self
                                .columns
                                .iter()
                                .map(|c| quote_ident(engine, &c.name))
                                .collect();
                            let values: Vec<String> =
                                row.iter().map(|v| sql_literal(engine, v)).collect();
                            writeln!(
                                w,
                                "INSERT INTO {} ({}) VALUES ({});",
                                quote_ident(engine, &self.table),
                                columns.join(", "),
                                values.join(", ")
                            )?;
                        }
                        ExportFormat::Xlsx => unreachable!(),
                    }
                }
                Sink::Xlsx {
                    worksheet,
                    row: xrow,
                } => {
                    if u64::from(*xrow) > XLSX_MAX_ROWS {
                        self.truncated = true;
                        continue;
                    }
                    for (col, value) in row.iter().enumerate() {
                        if col >= XLSX_MAX_COLS {
                            break;
                        }
                        write_xlsx_cell(worksheet, *xrow, col as u16, value)?;
                    }
                    *xrow += 1;
                }
            }
        }
        Ok(())
    }

    /// Delete the file after a failed export so users never see partial output.
    pub fn abort(self) {
        drop(self.sink);
        let _ = std::fs::remove_file(&self.path);
    }

    pub fn finish(mut self) -> MieoResult<ExportStats> {
        if !self.header_written && !self.columns.is_empty() {
            self.write_header()?;
        }
        let sink = self.sink.take().expect("sink present");
        match sink {
            Sink::Text(mut writer) => {
                if self.format == ExportFormat::Json {
                    // Close the array (open it even for an empty result).
                    if self.json_first {
                        writeln!(writer, "[")?;
                    }
                    writeln!(writer, "\n]")?;
                }
                writer.flush()?;
                drop(writer);
                let bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
                Ok(ExportStats {
                    rows: self.rows,
                    bytes,
                    elapsed_ms: 0,
                    truncated: self.truncated,
                    path: self.path.to_string_lossy().into_owned(),
                })
            }
            Sink::Xlsx { worksheet, .. } => {
                let mut workbook = rust_xlsxwriter::Workbook::new();
                workbook.push_worksheet(*worksheet);
                workbook.save(&self.path).map_err(xlsx_err)?;
                let bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
                Ok(ExportStats {
                    rows: self.rows,
                    bytes,
                    elapsed_ms: 0,
                    truncated: self.truncated,
                    path: self.path.to_string_lossy().into_owned(),
                })
            }
        }
    }
}

fn xlsx_err(e: rust_xlsxwriter::XlsxError) -> MieoError {
    MieoError::new(ErrorCode::Data, format!("xlsx write error: {e}"))
}

// ---------------------------------------------------------------------------
// Cell encodings
// ---------------------------------------------------------------------------

/// Plain text rendering for CSV/TSV (no placeholder strings, NULL = empty).
fn cell_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Decimal(s) => s.clone(),
        Value::Text(s) | Value::Date(s) | Value::Time(s) | Value::DateTime(s) | Value::Uuid(s) => {
            s.clone()
        }
        Value::Bytes(b) => b.clone(),
        Value::Json(j) => j.to_string(),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(cell_text).collect::<Vec<_>>().join(", ")
        ),
        Value::Unsupported { native_type, .. } => format!("<{native_type}>"),
    }
}

/// Natural JSON mapping (numbers stay numbers when lossless).
fn value_to_json(value: &Value) -> JsonValue {
    match value {
        Value::Null => JsonValue::Null,
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::UInt(u) => json!(u),
        Value::Float(f) => {
            if f.is_finite() {
                json!(f)
            } else {
                json!(f.to_string())
            }
        }
        Value::Decimal(s) => match s.parse::<f64>() {
            Ok(f) if f.is_finite() && s.len() <= 15 => json!(f),
            _ => json!(s),
        },
        Value::Text(s) | Value::Date(s) | Value::Time(s) | Value::DateTime(s) | Value::Uuid(s) => {
            json!(s)
        }
        Value::Bytes(b) => json!(b),
        Value::Json(j) => j.clone(),
        Value::Array(items) => JsonValue::Array(items.iter().map(value_to_json).collect()),
        Value::Unsupported { native_type, .. } => json!(format!("<{native_type}>")),
    }
}

fn row_object(columns: &[ColumnMeta], row: &[Value]) -> String {
    let mut map = serde_json::Map::with_capacity(row.len());
    for (i, value) in row.iter().enumerate() {
        let key = columns
            .get(i)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("column_{}", i + 1));
        map.insert(key, value_to_json(value));
    }
    serde_json::Value::Object(map).to_string()
}

fn csv_line(cells: &[String], delimiter: char) -> String {
    let mut out = String::new();
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            out.push(delimiter);
        }
        let needs_quote = cell.contains(delimiter)
            || cell.contains('"')
            || cell.contains('\n')
            || cell.contains('\r');
        if needs_quote {
            out.push('"');
            out.push_str(&cell.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(cell);
        }
    }
    out
}

/// SQL literal, engine-aware (bytes → hex literal).
fn sql_literal(engine: Engine, value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => {
            if f.is_finite() {
                f.to_string()
            } else {
                "NULL".to_string()
            }
        }
        Value::Decimal(s) => {
            if s.parse::<f64>().is_ok() {
                s.clone()
            } else {
                quote(s)
            }
        }
        Value::Text(s) | Value::Date(s) | Value::Time(s) | Value::DateTime(s) | Value::Uuid(s) => {
            quote(s)
        }
        Value::Bytes(b64) => {
            let hex = decode_hex(b64);
            match engine {
                Engine::Postgres => format!("'\\x{hex}'"),
                _ => format!("X'{hex}'"),
            }
        }
        Value::Json(j) => quote(&j.to_string()),
        Value::Array(_) => quote(&cell_text(value)),
        Value::Unsupported { .. } => "NULL".to_string(),
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn decode_hex(b64: &str) -> String {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap_or_default();
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// Write one cell into the Excel sheet with type preservation where safe.
fn write_xlsx_cell(
    worksheet: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    value: &Value,
) -> MieoResult<()> {
    match value {
        Value::Null => {} // leave blank
        Value::Bool(b) => {
            worksheet.write_boolean(row, col, *b).map_err(xlsx_err)?;
        }
        Value::Int(i) => {
            if i.unsigned_abs() <= (1u64 << 53) {
                worksheet
                    .write_number(row, col, *i as f64)
                    .map_err(xlsx_err)?;
            } else {
                worksheet
                    .write_string(row, col, i.to_string())
                    .map_err(xlsx_err)?;
            }
        }
        Value::UInt(u) => {
            if *u <= (1u64 << 53) {
                worksheet
                    .write_number(row, col, *u as f64)
                    .map_err(xlsx_err)?;
            } else {
                worksheet
                    .write_string(row, col, u.to_string())
                    .map_err(xlsx_err)?;
            }
        }
        Value::Float(f) => {
            if f.is_finite() {
                worksheet.write_number(row, col, *f).map_err(xlsx_err)?;
            } else {
                worksheet
                    .write_string(row, col, f.to_string())
                    .map_err(xlsx_err)?;
            }
        }
        Value::Decimal(s) => match s.parse::<f64>() {
            Ok(f) if f.is_finite() && s.len() <= 15 => {
                worksheet.write_number(row, col, f).map_err(xlsx_err)?;
            }
            _ => {
                worksheet
                    .write_string(row, col, s.clone())
                    .map_err(xlsx_err)?;
            }
        },
        _ => {
            worksheet
                .write_string(row, col, cell_text(value))
                .map_err(xlsx_err)?;
        }
    }
    Ok(())
}

/// Column types are only used to pick a display hint for the CLI; kept to make
/// future typed exports (dates as real Excel dates) easy to add.
#[allow(dead_code)]
fn is_temporal(kind: &DataTypeKind) -> bool {
    matches!(
        kind,
        DataTypeKind::Date
            | DataTypeKind::Time
            | DataTypeKind::DateTime
            | DataTypeKind::TimestampTz
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escaping() {
        assert_eq!(csv_line(&["a,b".into(), "x".into()], ','), "\"a,b\",x");
        assert_eq!(csv_line(&["he\"llo".into()], ','), "\"he\"\"llo\"");
        assert_eq!(csv_line(&["t\ts".into()], '\t'), "\"t\ts\"");
    }

    #[test]
    fn sql_literals_are_engine_aware() {
        assert_eq!(
            sql_literal(Engine::Postgres, &Value::Text("O'Brien".into())),
            "'O''Brien'"
        );
        assert_eq!(
            sql_literal(Engine::MySql, &Value::Bytes("AQID".into())),
            "X'010203'"
        );
        assert_eq!(
            sql_literal(Engine::Postgres, &Value::Bytes("AQID".into())),
            "'\\x010203'"
        );
        assert_eq!(sql_literal(Engine::Sqlite, &Value::Null), "NULL");
    }

    #[test]
    fn text_encoding_handles_all_values() {
        assert_eq!(cell_text(&Value::Null), "");
        assert_eq!(cell_text(&Value::Decimal("12.50".into())), "12.50");
        assert_eq!(cell_text(&Value::Json(json!({"a": 1}))), "{\"a\":1}");
    }
}
