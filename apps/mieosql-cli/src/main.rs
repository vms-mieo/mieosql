//! MieoSQL headless CLI.
//!
//! A first-class consumer of the same API the desktop frontend uses — handy
//! for scripting, smoke-testing the backend and CI.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::json;

use mieosql_api::{Api, ProfileInput};
use mieosql_core::{
    ColumnMeta, Engine as DbEngine, QueryEvent, QueryMode, QueryOptions, SslMode, Value,
};
use mieosql_engine::{Engine, EngineConfig, HistoryStatus};

#[derive(Parser)]
#[command(
    name = "mieosql",
    version,
    about = "MieoSQL CLI — multi-engine database client backend"
)]
struct Cli {
    /// Override the app data directory (profiles, vault, history).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage connection profiles
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// Test a saved connection and print server information
    Test {
        profile: String,
        #[arg(long)]
        password: Option<String>,
    },
    /// Run a SQL statement and stream results
    Query(QueryArgs),
    /// List tables/views in a database
    Schema(SchemaArgs),
    /// Describe the columns of a table
    Columns(TableArgs),
    /// Print the DDL of a table/view
    Ddl(TableArgs),
    /// Show query history
    History {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// List saved profiles
    List,
    /// Create a profile
    Add(AddArgs),
    /// Delete a profile
    Remove { id: String },
    /// Show one profile as JSON
    Show { id: String },
}

#[derive(Args)]
struct AddArgs {
    #[arg(long)]
    name: String,
    #[arg(long, value_parser = parse_engine)]
    engine: DbEngine,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    user: Option<String>,
    #[arg(long)]
    password: Option<String>,
    #[arg(long)]
    database: Option<String>,
    /// SQLite database file path
    #[arg(long)]
    file: Option<String>,
    #[arg(long, value_parser = parse_ssl_mode, default_value = "prefer")]
    ssl_mode: SslMode,
    /// Extra engine parameter, repeatable: --param charset=utf8mb4
    #[arg(long = "param")]
    params: Vec<String>,
}

#[derive(Args)]
struct QueryArgs {
    profile: String,
    sql: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
    /// Execute unprepared (required for USE/SET/some DDL)
    #[arg(long)]
    simple: bool,
    #[arg(long, default_value_t = 256)]
    fetch_size: u32,
    /// Stop collecting after N rows
    #[arg(long)]
    max_rows: Option<u64>,
    #[arg(long)]
    database: Option<String>,
    #[arg(long)]
    schema: Option<String>,
}

#[derive(Args)]
struct SchemaArgs {
    profile: String,
    #[arg(long)]
    database: Option<String>,
    #[arg(long)]
    schema: Option<String>,
    #[arg(long, value_parser = parse_kind)]
    kind: Option<mieosql_core::ObjectKind>,
    #[arg(long)]
    pattern: Option<String>,
}

#[derive(Args)]
struct TableArgs {
    profile: String,
    table: String,
    #[arg(long)]
    database: Option<String>,
    #[arg(long)]
    schema: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
    Csv,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = cli.data_dir.unwrap_or_else(EngineConfig::default_dir);
    let engine = Engine::open(EngineConfig { data_dir })?;
    let api = Api::new(engine);

    match cli.command {
        Command::Profile { cmd } => match cmd {
            ProfileCmd::List => {
                let profiles = api.list_profiles();
                if profiles.is_empty() {
                    println!("no profiles yet — create one with `mieosql profile add`");
                }
                for p in profiles {
                    println!(
                        "{:16} {:10} {:24} {}",
                        p.id.0,
                        p.engine.label(),
                        p.endpoint_summary(),
                        p.name
                    );
                }
            }
            ProfileCmd::Add(args) => {
                let mut params = std::collections::BTreeMap::new();
                for kv in &args.params {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| anyhow!("--param expects key=value, got `{kv}`"))?;
                    params.insert(k.to_string(), v.to_string());
                }
                let input = ProfileInput {
                    id: None,
                    name: args.name,
                    engine: args.engine,
                    host: args.host,
                    port: args.port,
                    username: args.user,
                    database: args.database,
                    file: args.file,
                    params,
                    ssl_mode: Some(args.ssl_mode),
                    ssh: None,
                    color: None,
                    group: None,
                    variant: None,
                    password: args.password,
                    ssh_password: None,
                };
                let profile = api.save_profile(input)?;
                println!("saved profile {} ({})", profile.id.0, profile.name);
            }
            ProfileCmd::Remove { id } => {
                let profile_id = resolve_profile(&api, &id)?;
                api.delete_profile(profile_id)?;
                println!("deleted profile {id}");
            }
            ProfileCmd::Show { id } => {
                let profile_id = resolve_profile(&api, &id)?;
                let profile = api.get_profile(profile_id)?;
                println!("{}", serde_json::to_string_pretty(&profile)?);
            }
        },

        Command::Test { profile, password } => {
            let profile_id = resolve_profile(&api, &profile)?;
            let info = api.test_profile(profile_id, password).await?;
            println!(
                "✓ connected: {} {}{}",
                info.engine.label(),
                info.version,
                info.current_database
                    .as_deref()
                    .map(|d| format!(" · database {d}"))
                    .unwrap_or_default()
            );
            if let Some(user) = info.current_user {
                println!("  user: {user}");
            }
        }

        Command::Query(args) => {
            let profile_id = resolve_profile(&api, &args.profile)?;
            let session = api.open_session(profile_id).await?;
            if let Some(db) = &args.database {
                api.use_database(session.id.clone(), db.clone()).await?;
            }

            let options = QueryOptions {
                mode: if args.simple {
                    QueryMode::Simple
                } else {
                    QueryMode::Prepared
                },
                fetch_size: args.fetch_size,
                max_rows: args.max_rows,
                timeout_ms: None,
            };
            let mut stream = api
                .start_query(session.id.clone(), args.sql, vec![], Some(options))
                .await?;

            let mut printer = Printer::new(args.format);
            let mut failed = false;
            while let Some(event) = stream.events.recv().await {
                match event {
                    QueryEvent::Started { .. } => {}
                    QueryEvent::Columns { columns, .. } => printer.on_columns(&columns),
                    QueryEvent::Batch { rows, .. } => printer.on_batch(&rows)?,
                    QueryEvent::Affected {
                        rows_affected,
                        last_insert_id,
                        ..
                    } => {
                        printer.finish()?;
                        print!("{rows_affected} row(s) affected");
                        match last_insert_id {
                            Some(id) => println!(", last insert id {id}"),
                            None => println!(),
                        }
                    }
                    QueryEvent::Message { text, .. } => eprintln!("-- {text}"),
                    QueryEvent::Error { error, .. } => {
                        printer.finish()?;
                        failed = true;
                        eprintln!("error [{}]: {}", error_code(&error.code), error.message);
                        if let Some(detail) = error.detail {
                            eprintln!("detail: {detail}");
                        }
                        if let Some(hint) = error.hint {
                            eprintln!("hint: {hint}");
                        }
                        if let Some(db_code) = error.db_code {
                            eprintln!("code: {db_code}");
                        }
                        break;
                    }
                    QueryEvent::Finished { stats, .. } => {
                        printer.finish()?;
                        eprintln!(
                            "{} row(s) in {} ms{}",
                            stats.rows,
                            stats.elapsed_ms,
                            if stats.truncated { " (truncated)" } else { "" }
                        );
                    }
                }
            }
            let _ = api.close_session(session.id).await;
            if failed {
                std::process::exit(1);
            }
        }

        Command::Schema(args) => {
            let profile_id = resolve_profile(&api, &args.profile)?;
            let session = api.open_session(profile_id).await?;
            if let Some(db) = &args.database {
                api.use_database(session.id.clone(), db.clone()).await?;
            }
            let scope = mieosql_core::Scope {
                database: args.database,
                schema: args.schema,
            };
            let objects = api
                .list_objects(session.id.clone(), scope, args.kind, args.pattern)
                .await?;
            for obj in &objects {
                let rows = obj
                    .rows_estimate
                    .map(|r| format!("~{r} rows"))
                    .unwrap_or_default();
                let size = obj
                    .size_bytes
                    .map(|s| format!("{:.1} MB", s as f64 / 1_048_576.0))
                    .unwrap_or_default();
                println!(
                    "{:10} {:24} {:12} {:10}",
                    format!("{:?}", obj.kind).to_lowercase(),
                    qualified_name(obj),
                    rows,
                    size
                );
            }
            eprintln!("{} object(s)", objects.len());
            let _ = api.close_session(session.id).await;
        }

        Command::Columns(args) => {
            let profile_id = resolve_profile(&api, &args.profile)?;
            let session = api.open_session(profile_id).await?;
            if let Some(db) = &args.database {
                api.use_database(session.id.clone(), db.clone()).await?;
            }
            let scope = mieosql_core::Scope {
                database: args.database,
                schema: args.schema,
            };
            let columns = api
                .table_columns(session.id.clone(), scope.clone(), args.table.clone())
                .await?;
            for c in &columns {
                println!(
                    "{:4} {:24} {:28} {:8} {:4} {}{}",
                    c.ordinal,
                    c.name,
                    c.native_type,
                    if c.nullable { "NULL" } else { "NOT NULL" },
                    if c.is_primary_key { "PK" } else { "" },
                    c.default_value
                        .as_deref()
                        .map(|d| format!("default {d}"))
                        .unwrap_or_default(),
                    c.comment
                        .as_deref()
                        .map(|c| format!(" // {c}"))
                        .unwrap_or_default(),
                );
            }
            let indexes = api
                .table_indexes(session.id.clone(), scope.clone(), args.table.clone())
                .await?;
            if !indexes.is_empty() {
                println!("\nindexes:");
                for idx in indexes {
                    println!(
                        "  {:24} ({}){}{}",
                        idx.name,
                        idx.columns.join(", "),
                        if idx.is_primary {
                            " PRIMARY"
                        } else if idx.is_unique {
                            " UNIQUE"
                        } else {
                            ""
                        },
                        idx.index_type
                            .map(|t| format!(" [{t}]"))
                            .unwrap_or_default(),
                    );
                }
            }
            let fks = api
                .foreign_keys(session.id.clone(), scope, args.table.clone())
                .await?;
            if !fks.is_empty() {
                println!("\nforeign keys:");
                for fk in fks {
                    println!(
                        "  {:24} ({}) -> {}.{} {}",
                        fk.name,
                        fk.columns.join(", "),
                        fk.ref_table,
                        fk.ref_columns.join(", "),
                        fk.on_delete
                            .map(|d| format!("ON DELETE {d}"))
                            .unwrap_or_default(),
                    );
                }
            }
            let _ = api.close_session(session.id).await;
        }

        Command::Ddl(args) => {
            let profile_id = resolve_profile(&api, &args.profile)?;
            let session = api.open_session(profile_id).await?;
            if let Some(db) = &args.database {
                api.use_database(session.id.clone(), db.clone()).await?;
            }
            let scope = mieosql_core::Scope {
                database: args.database,
                schema: args.schema,
            };
            match api
                .object_ddl(
                    session.id.clone(),
                    scope,
                    mieosql_core::ObjectKind::Table,
                    args.table.clone(),
                )
                .await?
            {
                Some(ddl) => println!("{ddl};"),
                None => eprintln!("no DDL available for {}", args.table),
            }
            let _ = api.close_session(session.id).await;
        }

        Command::History { limit } => {
            let entries = api.list_history(Some(limit));
            for e in entries {
                let status = match e.status {
                    HistoryStatus::Ok => "ok",
                    HistoryStatus::Error => "error",
                    HistoryStatus::Cancelled => "cancelled",
                };
                let sql_one_line = e.sql.replace('\n', " ");
                let sql_short: String = sql_one_line.chars().take(90).collect();
                println!(
                    "{:6} {:>6}ms {:>8} rows  {:<10} {}",
                    status,
                    e.elapsed_ms,
                    e.rows,
                    e.engine.label(),
                    sql_short
                );
                if let Some(err) = &e.error {
                    println!("       ↳ {}", err.message);
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_profile(api: &Api, key: &str) -> Result<String> {
    let profiles = api.list_profiles();
    if let Some(p) = profiles.iter().find(|p| p.id.0 == key || p.name == key) {
        return Ok(p.id.0.clone());
    }
    Err(anyhow!("profile `{key}` not found"))
}

fn parse_engine(s: &str) -> Result<DbEngine, String> {
    match s.to_ascii_lowercase().as_str() {
        "mysql" => Ok(DbEngine::MySql),
        "mariadb" => Ok(DbEngine::MariaDb),
        "postgres" | "postgresql" | "pg" => Ok(DbEngine::Postgres),
        "sqlite" => Ok(DbEngine::Sqlite),
        "sqlserver" | "mssql" => Ok(DbEngine::SqlServer),
        "mongodb" | "mongo" => Ok(DbEngine::MongoDb),
        "redis" => Ok(DbEngine::Redis),
        "oracle" => Ok(DbEngine::Oracle),
        "snowflake" => Ok(DbEngine::Snowflake),
        other => Err(format!("unknown engine `{other}`")),
    }
}

fn parse_ssl_mode(s: &str) -> Result<SslMode, String> {
    match s.to_ascii_lowercase().replace('_', "-").as_str() {
        "disable" | "disabled" => Ok(SslMode::Disable),
        "prefer" | "preferred" => Ok(SslMode::Prefer),
        "require" | "required" => Ok(SslMode::Require),
        "verify-ca" => Ok(SslMode::VerifyCa),
        "verify-full" | "verify-identity" => Ok(SslMode::VerifyFull),
        other => Err(format!("unknown ssl mode `{other}`")),
    }
}

fn parse_kind(s: &str) -> Result<mieosql_core::ObjectKind, String> {
    use mieosql_core::ObjectKind as K;
    match s.to_ascii_lowercase().as_str() {
        "table" | "tables" => Ok(K::Table),
        "view" | "views" => Ok(K::View),
        "matview" | "materialized_view" => Ok(K::MaterializedView),
        "function" | "functions" => Ok(K::Function),
        "procedure" | "procedures" => Ok(K::Procedure),
        "trigger" | "triggers" => Ok(K::Trigger),
        "sequence" | "sequences" => Ok(K::Sequence),
        other => Err(format!("unknown object kind `{other}`")),
    }
}

fn error_code(code: &mieosql_core::ErrorCode) -> String {
    format!("{code:?}").to_lowercase()
}

fn qualified_name(obj: &mieosql_core::ObjectRef) -> String {
    match (&obj.schema, &obj.database) {
        (Some(schema), _) => format!("{schema}.{}", obj.name),
        (None, Some(db)) => format!("{db}.{}", obj.name),
        _ => obj.name.clone(),
    }
}

// ---------------------------------------------------------------------------
// Output printer
// ---------------------------------------------------------------------------

struct Printer {
    format: OutputFormat,
    columns: Vec<String>,
    widths: Option<Vec<usize>>,
    csv_header_done: bool,
}

impl Printer {
    fn new(format: OutputFormat) -> Self {
        Self {
            format,
            columns: Vec::new(),
            widths: None,
            csv_header_done: false,
        }
    }

    fn on_columns(&mut self, columns: &[ColumnMeta]) {
        self.columns = columns.iter().map(|c| c.name.clone()).collect();
    }

    fn on_batch(&mut self, rows: &[Vec<Value>]) -> Result<()> {
        match self.format {
            OutputFormat::Json => {
                for row in rows {
                    let obj: serde_json::Map<String, serde_json::Value> = self
                        .columns
                        .iter()
                        .zip(row.iter())
                        .map(|(name, value)| (name.clone(), natural_json(value)))
                        .collect();
                    println!("{}", serde_json::Value::Object(obj));
                }
            }
            OutputFormat::Csv => {
                if !self.csv_header_done {
                    println!(
                        "{}",
                        self.columns
                            .iter()
                            .map(|c| csv_escape(c))
                            .collect::<Vec<_>>()
                            .join(",")
                    );
                    self.csv_header_done = true;
                }
                for row in rows {
                    let line: Vec<String> = row
                        .iter()
                        .map(|v| match v {
                            Value::Null => String::new(),
                            other => csv_escape(&other.to_display_string()),
                        })
                        .collect();
                    println!("{}", line.join(","));
                }
            }
            OutputFormat::Table => {
                // Lazy header: widths are derived from the first batch so the
                // table stays aligned for the common case.
                if self.widths.is_none() {
                    let mut widths: Vec<usize> =
                        self.columns.iter().map(|c| c.chars().count()).collect();
                    for row in rows {
                        for (i, v) in row.iter().enumerate() {
                            if i < widths.len() {
                                let len = v.to_display_string().chars().count();
                                if len > widths[i] {
                                    widths[i] = len.min(60);
                                }
                            }
                        }
                    }
                    self.widths = Some(widths.clone());
                    let header: Vec<String> = self
                        .columns
                        .iter()
                        .enumerate()
                        .map(|(i, c)| pad(c, widths[i]))
                        .collect();
                    println!("{}", header.join(" | "));
                    println!(
                        "{}",
                        widths
                            .iter()
                            .map(|w| "-".repeat(*w))
                            .collect::<Vec<_>>()
                            .join("-+-")
                    );
                }
                let widths = self.widths.clone().unwrap_or_default();
                for row in rows {
                    let cells: Vec<String> = row
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            pad(&v.to_display_string(), widths.get(i).copied().unwrap_or(0))
                        })
                        .collect();
                    println!("{}", cells.join(" | "));
                }
            }
        }
        std::io::Write::flush(&mut std::io::stdout()).context("flush stdout")?;
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}

fn pad(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Natural JSON rendering for CLI output (tags dropped, big decimals kept as strings).
fn natural_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::UInt(u) => json!(u),
        Value::Float(f) => json!(f),
        Value::Decimal(s) | Value::Text(s) | Value::Bytes(s) => json!(s),
        Value::Date(s) | Value::Time(s) | Value::DateTime(s) | Value::Uuid(s) => json!(s),
        Value::Json(j) => j.clone(),
        Value::Array(items) => serde_json::Value::Array(items.iter().map(natural_json).collect()),
        Value::Unsupported { native_type, .. } => json!(format!("<{native_type}>")),
    }
}
