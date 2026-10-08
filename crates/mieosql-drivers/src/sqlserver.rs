//! SQL Server / Azure SQL driver (tiberius, pure Rust TDS).
//!
//! The driver owns a single TDS connection (`tiberius::Client` over a Tokio
//! TCP stream) instead of a pool, so `run_query` takes `&mut self` and streams
//! `Columns` / `Batch` / `Affected` events into the engine sink exactly like
//! the sqlx-backed SQL drivers do.
//!
//! Placeholders: the editor emits `?` (see [`sqlserver_capabilities`]); before
//! a prepared execution they are rewritten to tiberius' `@P1..` RPC parameter
//! names by a small scanner that ignores strings, quoted identifiers and
//! comments.
//!
//! `preview_statement` stays unsupported: the API layer generates
//! `SELECT TOP (n) ...` for SQL Server.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, SecondsFormat};
use futures::StreamExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant as TokioInstant;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

use tiberius::error::Error as TiberiusError;
use tiberius::numeric::Numeric;
use tiberius::{
    AuthMethod, Client, Column, ColumnData, ColumnType, Config, EncryptionLevel, FromSql, IntoSql,
    Query, QueryItem, Row,
};

use mieosql_core::{
    Capabilities, ColumnDef, ColumnMeta, ConnectionSpec, CreateDbUser, DataTypeKind, DatabaseRef,
    DbPrincipal, DbPrincipalDetail, DetailAttribute, DropDbPrincipal, Engine, ErrorCode,
    ForeignKeyDef, GrantRequest, GrantTarget, IndexDef, MieoError, MieoResult, ObjectKind,
    ObjectRef, PlaceholderStyle, PrincipalKind, PrivilegeEntry, QueryEvent, QueryMode,
    QueryOptions, QueryStats, RevokeRequest, RoleGrant, SchemaRef, Scope, ServerInfo, SslMode,
    Value,
};

use crate::ssh_tunnel::{SshTunnel, prepare_endpoint};
use crate::util::{
    b64_decode, b64_encode, fmt_naive_date, fmt_naive_dt, fmt_naive_time, quote_literal,
};

/// Concrete TDS client type: a Tokio TCP stream adapted to `futures::io`.
type TdsClient = Client<Compat<TcpStream>>;

pub struct SqlServerDriver {
    client: TdsClient,
    spec: ConnectionSpec,
    /// Mirrors the server-side transaction state; toggled by `begin`/`commit`/`rollback`.
    tx_open: bool,
    /// Keeps the SSH tunnel (if any) alive for the connection lifetime.
    _ssh_tunnel: Option<SshTunnel>,
}

pub(crate) fn sqlserver_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::Sql,
        // `?` positional placeholders are rewritten to `@P1..` internally.
        placeholder: PlaceholderStyle::Question,
        list_databases: true,
        list_schemas: true,
        schema_is_database: false,
        transactions: true,
        simple_mode: true,
        ddl: true,
        multiple_databases: true,
        user_management: true,
    }
}

impl SqlServerDriver {
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        let (host, port, _ssh_tunnel) = prepare_endpoint(spec, "127.0.0.1", 1433).await?;
        let username = spec.username.clone().ok_or_else(|| {
            MieoError::new(
                ErrorCode::Auth,
                "SQL Server requires a username for SQL authentication",
            )
        })?;
        let password = spec.password.clone().unwrap_or_default();

        let mut config = Config::new();
        config.host(&host);
        config.port(port);
        config.authentication(AuthMethod::sql_server(username, password));
        if let Some(db) = spec.database.as_deref().filter(|d| !d.is_empty()) {
            config.database(db);
        }
        config.application_name("MieoSQL");
        apply_tls(&mut config, spec.ssl_mode);

        let timeout = Duration::from_secs(spec.connect_timeout_secs.max(1));
        config.handshake_timeout(Some(timeout));
        // Per-query deadlines come from `QueryOptions::timeout_ms`; a stalled
        // server is additionally interruptible through the cancellation token.
        config.command_timeout(None);

        let addr = config.get_addr();
        let tcp = tokio::time::timeout(timeout, TcpStream::connect(&addr))
            .await
            .map_err(|_| {
                MieoError::new(
                    ErrorCode::Timeout,
                    format!("timed out connecting to {addr}"),
                )
            })?
            .map_err(|e| {
                MieoError::new(
                    ErrorCode::Network,
                    format!("cannot connect to {addr}: {e}"),
                )
            })?;
        let _ = tcp.set_nodelay(true);

        let client = Client::connect(config, tcp.compat_write())
            .await
            .map_err(map_tiberius_err)?;

        Ok(Self {
            client,
            spec: spec.clone(),
            tx_open: false,
            _ssh_tunnel,
        })
    }

    pub fn spec(&self) -> &ConnectionSpec {
        &self.spec
    }

    pub fn in_transaction(&self) -> bool {
        self.tx_open
    }

    pub async fn ping(&mut self) -> MieoResult<()> {
        self.simple_exec("SELECT 1").await
    }

    pub async fn server_info(&mut self) -> MieoResult<ServerInfo> {
        let rows = self
            .fetch_rows(
                "SELECT CAST(SERVERPROPERTY('ProductVersion') AS NVARCHAR(128)), \
                        CAST(SERVERPROPERTY('Edition') AS NVARCHAR(128)), \
                        CAST(SERVERPROPERTY('ProductLevel') AS NVARCHAR(128)), \
                        DB_NAME(), SUSER_SNAME()",
                Vec::new(),
            )
            .await?;
        let row = rows.first().ok_or_else(|| {
            MieoError::new(ErrorCode::Internal, "SERVERPROPERTY returned no rows")
        })?;
        let edition = cell_opt_str(row, 1).filter(|e| !e.is_empty());
        let level = cell_opt_str(row, 2).filter(|l| !l.is_empty());
        let version_comment = match (edition, level) {
            (Some(e), Some(l)) => Some(format!("{e} ({l})")),
            (Some(e), None) => Some(e),
            (None, Some(l)) => Some(l),
            (None, None) => None,
        };
        Ok(ServerInfo {
            engine: Engine::SqlServer,
            version: cell_str(row, 0),
            version_comment,
            current_database: cell_opt_str(row, 3),
            current_user: cell_opt_str(row, 4),
        })
    }

    pub async fn list_databases(&mut self) -> MieoResult<Vec<DatabaseRef>> {
        let rows = self
            .fetch_rows(
                "SELECT name, collation_name, database_id, state_desc \
                 FROM sys.databases ORDER BY name",
                Vec::new(),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|row| DatabaseRef {
                name: cell_str(row, 0),
                // SQL Server's first four database ids are the system databases.
                is_system: cell_i64(row, 2).unwrap_or(0) <= 4,
                charset: None,
                collation: cell_opt_str(row, 1),
                comment: cell_opt_str(row, 3).filter(|s| !s.eq_ignore_ascii_case("ONLINE")),
                size_bytes: None,
            })
            .collect())
    }

    pub async fn list_schemas(&mut self, database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        let db = database
            .filter(|d| !d.is_empty())
            .map(str::to_string)
            .or_else(|| self.spec.database.clone());
        let sql = format!(
            "SELECT name FROM {}sys.schemas ORDER BY name",
            db_prefix(db.as_deref())
        );
        let rows = self.fetch_rows(&sql, Vec::new()).await?;
        Ok(rows
            .iter()
            .map(|row| {
                let name = cell_str(row, 0);
                let is_system = matches!(
                    name.to_ascii_lowercase().as_str(),
                    "sys" | "information_schema" | "guest"
                );
                SchemaRef { name, is_system }
            })
            .collect())
    }

    pub async fn list_objects(
        &mut self,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        let db = self
            .resolve_db(scope)
            .map(str::to_string)
            .or_else(|| self.spec.database.clone());
        let prefix = db_prefix(db.as_deref());

        // Stored routines + triggers live in sys.objects / sys.schemas, not in
        // INFORMATION_SCHEMA.TABLES.
        if matches!(
            kind,
            Some(ObjectKind::Function | ObjectKind::Procedure | ObjectKind::Trigger)
        ) {
            let want = kind.expect("matched routine/trigger kind");
            // sys.objects type codes: FN scalar function, IF inline table
            // function, TF table function, P procedure, TR trigger.
            let Some(in_list) = Self::sys_types_for_kind(want) else {
                return Ok(Vec::new());
            };
            let mut sql = format!(
                "SELECT s.name, o.name, OBJECT_NAME(o.parent_object_id) AS parent_name \
                 FROM {prefix}sys.objects o \
                 JOIN {prefix}sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE o.type IN ({in_list})"
            );
            let mut params: Vec<Value> = Vec::new();
            if let Some(schema) = scope.schema.as_deref().filter(|s| !s.is_empty()) {
                sql.push_str(&format!(" AND s.name = @P{}", params.len() + 1));
                params.push(Value::Text(schema.to_string()));
            }
            if let Some(pattern) = pattern {
                sql.push_str(&format!(" AND o.name LIKE @P{}", params.len() + 1));
                params.push(Value::Text(format!("%{pattern}%")));
            }
            sql.push_str(" ORDER BY s.name, o.name");

            let rows = self.fetch_rows(&sql, params).await?;
            return Ok(rows
                .iter()
                .map(|row| {
                    // Triggers carry their table in the comment so the UI can
                    // group them per table; routines have no parent.
                    let parent = cell_str(row, 2);
                    let comment = if want == ObjectKind::Trigger && !parent.is_empty() {
                        Some(format!("ON {parent}"))
                    } else {
                        None
                    };
                    ObjectRef {
                        name: cell_str(row, 1),
                        kind: want,
                        database: db.clone(),
                        schema: Some(cell_str(row, 0)),
                        comment,
                        rows_estimate: None,
                        size_bytes: None,
                        updated_at: None,
                    }
                })
                .collect());
        }
        match kind {
            Some(ObjectKind::Table) | Some(ObjectKind::View) | None => {}
            Some(_) => return Ok(Vec::new()),
        }

        let mut sql = format!(
            "SELECT t.TABLE_SCHEMA, t.TABLE_NAME, t.TABLE_TYPE, \
                    CAST(COALESCE(SUM(p.rows), 0) AS BIGINT) AS rows_estimate \
             FROM {prefix}INFORMATION_SCHEMA.TABLES t \
             LEFT JOIN {prefix}sys.schemas s ON s.name = t.TABLE_SCHEMA \
             LEFT JOIN {prefix}sys.objects o \
                    ON o.schema_id = s.schema_id AND o.name = t.TABLE_NAME AND o.type IN ('U', 'V') \
             LEFT JOIN {prefix}sys.partitions p ON p.object_id = o.object_id AND p.index_id IN (0, 1) \
             WHERE t.TABLE_TYPE IN ('BASE TABLE', 'VIEW')"
        );
        let mut params: Vec<Value> = Vec::new();
        match kind {
            Some(ObjectKind::Table) => sql.push_str(" AND t.TABLE_TYPE = 'BASE TABLE'"),
            Some(ObjectKind::View) => sql.push_str(" AND t.TABLE_TYPE = 'VIEW'"),
            _ => {}
        }
        if let Some(schema) = scope.schema.as_deref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND t.TABLE_SCHEMA = @P1");
            params.push(Value::Text(schema.to_string()));
        }
        if let Some(pattern) = pattern {
            sql.push_str(&format!(" AND t.TABLE_NAME LIKE @P{}", params.len() + 1));
            params.push(Value::Text(format!("%{pattern}%")));
        }
        sql.push_str(" GROUP BY t.TABLE_SCHEMA, t.TABLE_NAME, t.TABLE_TYPE");
        sql.push_str(" ORDER BY t.TABLE_SCHEMA, t.TABLE_NAME");

        let rows = self.fetch_rows(&sql, params).await?;
        Ok(rows
            .iter()
            .map(|row| {
                let table_type = cell_str(row, 2);
                ObjectRef {
                    name: cell_str(row, 1),
                    kind: if table_type.eq_ignore_ascii_case("VIEW") {
                        ObjectKind::View
                    } else {
                        ObjectKind::Table
                    },
                    database: db.clone(),
                    schema: Some(cell_str(row, 0)),
                    comment: None,
                    rows_estimate: cell_i64(row, 3).filter(|v| *v >= 0).map(|v| v as u64),
                    size_bytes: None,
                    updated_at: None,
                }
            })
            .collect())
    }

    pub async fn table_columns(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<ColumnDef>> {
        let prefix = db_prefix(self.resolve_db(scope));
        let schema = self.resolve_schema(scope).to_string();
        let sql = format!(
            "SELECT c.COLUMN_NAME, c.ORDINAL_POSITION, c.DATA_TYPE, c.IS_NULLABLE, c.COLUMN_DEFAULT, \
                    c.CHARACTER_MAXIMUM_LENGTH, c.NUMERIC_PRECISION, c.NUMERIC_SCALE, \
                    c.DATETIME_PRECISION, c.COLLATION_NAME, \
                    CASE WHEN ic.column_id IS NULL THEN 0 ELSE 1 END AS is_primary_key, \
                    COLUMNPROPERTY(o.object_id, c.COLUMN_NAME, 'IsIdentity') AS is_identity, \
                    COLUMNPROPERTY(o.object_id, c.COLUMN_NAME, 'IsComputed') AS is_computed, \
                    CAST(ep.value AS NVARCHAR(MAX)) AS column_comment \
             FROM {prefix}INFORMATION_SCHEMA.COLUMNS c \
             JOIN {prefix}sys.objects o ON o.name = c.TABLE_NAME \
             JOIN {prefix}sys.schemas s ON s.schema_id = o.schema_id AND s.name = c.TABLE_SCHEMA \
             JOIN {prefix}sys.columns sc ON sc.object_id = o.object_id AND sc.name = c.COLUMN_NAME \
             LEFT JOIN {prefix}sys.indexes i ON i.object_id = o.object_id AND i.is_primary_key = 1 \
             LEFT JOIN {prefix}sys.index_columns ic \
                    ON ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.column_id = sc.column_id \
             LEFT JOIN {prefix}sys.extended_properties ep \
                    ON ep.class = 1 AND ep.major_id = o.object_id AND ep.minor_id = sc.column_id \
                   AND ep.name = 'MS_Description' \
             WHERE c.TABLE_NAME = @P1 AND c.TABLE_SCHEMA = @P2 \
             ORDER BY c.ORDINAL_POSITION"
        );
        let rows = self
            .fetch_rows(
                &sql,
                vec![Value::Text(table.to_string()), Value::Text(schema)],
            )
            .await?;

        Ok(rows
            .iter()
            .map(|row| {
                let data_type = cell_str(row, 2);
                let char_len = cell_i64(row, 5);
                let native_type = format_native_type(
                    &data_type,
                    char_len,
                    cell_i64(row, 6),
                    cell_i64(row, 7),
                    cell_i64(row, 8),
                );
                ColumnDef {
                    name: cell_str(row, 0),
                    ordinal: cell_i64(row, 1).unwrap_or(0).max(0) as u32,
                    data_type: kind_sqlserver(&native_type),
                    native_type,
                    nullable: cell_str(row, 3).eq_ignore_ascii_case("YES"),
                    default_value: cell_opt_str(row, 4),
                    is_primary_key: cell_i64(row, 10) == Some(1),
                    is_auto_increment: cell_i64(row, 11) == Some(1),
                    is_generated: cell_i64(row, 12) == Some(1),
                    comment: cell_opt_str(row, 13).filter(|c| !c.is_empty()),
                    char_max_length: char_len.filter(|v| *v >= 0).map(|v| v as u64),
                    numeric_precision: cell_i64(row, 6).filter(|v| *v >= 0).map(|v| v as u32),
                    numeric_scale: cell_i64(row, 7).filter(|v| *v >= 0).map(|v| v as u32),
                    collation: cell_opt_str(row, 9),
                }
            })
            .collect())
    }

    pub async fn table_indexes(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let prefix = db_prefix(self.resolve_db(scope));
        let schema = self.resolve_schema(scope).to_string();
        let sql = format!(
            "SELECT i.name, i.is_unique, i.is_primary_key, i.type_desc, i.filter_definition, \
                    c.name AS column_name, ic.key_ordinal \
             FROM {prefix}sys.indexes i \
             JOIN {prefix}sys.objects o ON o.object_id = i.object_id \
             JOIN {prefix}sys.schemas s ON s.schema_id = o.schema_id \
             LEFT JOIN {prefix}sys.index_columns ic \
                    ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
                   AND ic.is_included_column = 0 \
             LEFT JOIN {prefix}sys.columns c \
                    ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             WHERE o.name = @P1 AND s.name = @P2 AND i.type <> 0 AND i.name IS NOT NULL \
             ORDER BY i.name, ic.key_ordinal"
        );
        let rows = self
            .fetch_rows(
                &sql,
                vec![Value::Text(table.to_string()), Value::Text(schema)],
            )
            .await?;

        let mut out: Vec<IndexDef> = Vec::new();
        for row in &rows {
            let name = cell_str(row, 0);
            let column = cell_opt_str(row, 5);
            match out.last_mut() {
                Some(index) if index.name == name => {
                    if let Some(column) = column {
                        index.columns.push(column);
                    }
                }
                _ => out.push(IndexDef {
                    is_primary: cell_i64(row, 2) == Some(1),
                    is_unique: cell_i64(row, 1) == Some(1),
                    name,
                    columns: column.into_iter().collect(),
                    index_type: cell_opt_str(row, 3),
                    comment: cell_opt_str(row, 4).filter(|c| !c.is_empty()),
                }),
            }
        }
        Ok(out)
    }

    pub async fn foreign_keys(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        let prefix = db_prefix(self.resolve_db(scope));
        let schema = self.resolve_schema(scope).to_string();
        let sql = format!(
            "SELECT fk.name, pc.name AS parent_column, rs.name AS ref_schema, rt.name AS ref_table, \
                    rc.name AS ref_column, fk.update_referential_action_desc, \
                    fk.delete_referential_action_desc \
             FROM {prefix}sys.foreign_keys fk \
             JOIN {prefix}sys.objects pt ON pt.object_id = fk.parent_object_id \
             JOIN {prefix}sys.schemas ps ON ps.schema_id = pt.schema_id \
             JOIN {prefix}sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
             JOIN {prefix}sys.columns pc \
                    ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
             JOIN {prefix}sys.objects rt ON rt.object_id = fk.referenced_object_id \
             JOIN {prefix}sys.schemas rs ON rs.schema_id = rt.schema_id \
             JOIN {prefix}sys.columns rc \
                    ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
             WHERE pt.name = @P1 AND ps.name = @P2 \
             ORDER BY fk.name, fkc.constraint_column_id"
        );
        let rows = self
            .fetch_rows(
                &sql,
                vec![Value::Text(table.to_string()), Value::Text(schema)],
            )
            .await?;

        let mut out: Vec<ForeignKeyDef> = Vec::new();
        for row in &rows {
            let name = cell_str(row, 0);
            match out.last_mut() {
                Some(fk) if fk.name == name => {
                    if let Some(column) = cell_opt_str(row, 1) {
                        fk.columns.push(column);
                    }
                    if let Some(column) = cell_opt_str(row, 4) {
                        fk.ref_columns.push(column);
                    }
                }
                _ => out.push(ForeignKeyDef {
                    name,
                    columns: cell_opt_str(row, 1).into_iter().collect(),
                    ref_table: cell_str(row, 3),
                    ref_schema: cell_opt_str(row, 2),
                    ref_columns: cell_opt_str(row, 4).into_iter().collect(),
                    on_update: cell_opt_str(row, 5),
                    on_delete: cell_opt_str(row, 6),
                }),
            }
        }
        Ok(out)
    }

    pub async fn object_ddl(
        &mut self,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        if kind == ObjectKind::Table {
            let columns = self.table_columns(scope, name).await?;
            if columns.is_empty() {
                return Ok(None);
            }
            let qualified = self.qualified_name(scope, name);
            return Ok(Some(generate_create_table(&qualified, &columns)));
        }

        if !matches!(
            kind,
            ObjectKind::View | ObjectKind::Procedure | ObjectKind::Function | ObjectKind::Trigger
        ) {
            return Ok(None);
        }

        // When the caller did not pin a schema, resolve it from the catalog so
        // OBJECT_ID hits routines outside `dbo` (the explorer lists every
        // schema, but the scope often carries none).
        let schema = match scope.schema.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => self
                .resolve_object_schema(scope, kind, name)
                .await?
                .unwrap_or_else(|| self.resolve_schema(scope).to_string()),
        };
        let qualified = match self.resolve_db(scope) {
            Some(db) => format!(
                "{}.{}.{}",
                quote_sqlserver_ident(db),
                quote_sqlserver_ident(&schema),
                quote_sqlserver_ident(name)
            ),
            None => format!(
                "{}.{}",
                quote_sqlserver_ident(&schema),
                quote_sqlserver_ident(name)
            ),
        };
        let rows = self
            .fetch_rows(
                "SELECT OBJECT_DEFINITION(OBJECT_ID(@P1))",
                vec![Value::Text(qualified)],
            )
            .await?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        Ok(cell_opt_str(row, 0).filter(|ddl| !ddl.trim().is_empty()))
    }

    // ------------------------------------------------------------------
    // Users / roles / grants (also drives the authorization UI).
    // Users come from `sys.database_principals`; roles merge database roles
    // with server roles (marked in `comment`).
    // ------------------------------------------------------------------

    pub async fn list_db_users(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        let rows = self
            .fetch_rows(
                "SELECT name, type_desc FROM sys.database_principals                  WHERE type IN ('S', 'U', 'G') AND name NOT IN ('dbo', 'guest', 'INFORMATION_SCHEMA', 'sys')                  ORDER BY name",
                Vec::new(),
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| DbPrincipal {
                name: cell_str(&row, 0),
                host: None,
                kind: PrincipalKind::User,
                is_super: false,
                can_login: true,
                comment: Some(cell_str(&row, 1)),
            })
            .collect())
    }

    pub async fn list_db_roles(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        let mut out: Vec<DbPrincipal> = Vec::new();
        if let Ok(rows) = self
            .fetch_rows(
                "SELECT name FROM sys.database_principals                  WHERE type = 'R' AND name NOT IN ('public') ORDER BY name",
                Vec::new(),
            )
            .await
        {
            out.extend(rows.into_iter().map(|row| DbPrincipal {
                name: cell_str(&row, 0),
                host: None,
                kind: PrincipalKind::Role,
                is_super: false,
                can_login: false,
                comment: Some("database role".into()),
            }));
        }
        if let Ok(rows) = self
            .fetch_rows(
                "SELECT name FROM sys.server_principals WHERE type = 'R' ORDER BY name",
                Vec::new(),
            )
            .await
        {
            out.extend(rows.into_iter().map(|row| DbPrincipal {
                name: cell_str(&row, 0),
                host: None,
                kind: PrincipalKind::Role,
                is_super: row_is_sysadmin(&row),
                can_login: false,
                comment: Some("server role".into()),
            }));
        }
        Ok(out)
    }

    pub async fn db_role_members(&mut self) -> MieoResult<Vec<RoleGrant>> {
        let mut out: Vec<RoleGrant> = Vec::new();
        if let Ok(rows) = self
            .fetch_rows(
                "SELECT r.name, m.name FROM sys.database_role_members a                  JOIN sys.database_principals r ON r.principal_id = a.role_principal_id                  JOIN sys.database_principals m ON m.principal_id = a.member_principal_id                  ORDER BY 1, 2",
                Vec::new(),
            )
            .await
        {
            out.extend(rows.into_iter().map(|row| RoleGrant {
                role: cell_str(&row, 0),
                role_host: None,
                member: cell_str(&row, 1),
                member_host: None,
                admin_option: false,
            }));
        }
        if let Ok(rows) = self
            .fetch_rows(
                "SELECT r.name, m.name FROM sys.server_role_members a                  JOIN sys.server_principals r ON r.principal_id = a.role_principal_id                  JOIN sys.server_principals m ON m.principal_id = a.member_principal_id                  ORDER BY 1, 2",
                Vec::new(),
            )
            .await
        {
            out.extend(rows.into_iter().map(|row| RoleGrant {
                role: cell_str(&row, 0),
                role_host: Some("server".into()),
                member: cell_str(&row, 1),
                member_host: None,
                admin_option: false,
            }));
        }
        Ok(out)
    }

    pub async fn db_user_grants(
        &mut self,
        name: &str,
        _host: Option<&str>,
    ) -> MieoResult<Vec<String>> {
        // fn_my_permissions needs an object context; the stable surface is
        // database-role membership for this principal.
        let rows = self
            .fetch_rows(
                "SELECT r.name FROM sys.database_role_members a                  JOIN sys.database_principals r ON r.principal_id = a.role_principal_id                  JOIN sys.database_principals m ON m.principal_id = a.member_principal_id                  WHERE m.name = @P1 ORDER BY 1",
                vec![Value::Text(name.to_string())],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| format!("MEMBER OF {}", cell_str(&row, 0)))
            .collect())
    }

    pub async fn db_principal_detail(
        &mut self,
        name: &str,
        _host: Option<&str>,
    ) -> MieoResult<DbPrincipalDetail> {
        let rows = self
            .fetch_rows(
                "SELECT name, type_desc, default_schema_name, create_date FROM sys.database_principals WHERE name = @P1",
                vec![Value::Text(name.to_string())],
            )
            .await?;
        let Some(row) = rows.first() else {
            return Err(MieoError::new(ErrorCode::NotFound, format!("principal `{name}` not found")));
        };
        let kind = if cell_str(row, 1) == "DATABASE_ROLE" || cell_str(row, 1) == "SERVER_ROLE" {
            PrincipalKind::Role
        } else {
            PrincipalKind::User
        };
        // Sysadmin membership (server role) counts as superuser.
        let mut is_super = false;
        if let Ok(sysrows) = self
            .fetch_rows(
                "SELECT r.name FROM sys.server_role_members a                  JOIN sys.server_principals r ON r.principal_id = a.role_principal_id                  JOIN sys.server_principals m ON m.principal_id = a.member_principal_id                  WHERE m.name = @P1 AND r.name = 'sysadmin'",
                vec![Value::Text(name.to_string())],
            )
            .await
        {
            is_super = !sysrows.is_empty();
        }
        let principal = DbPrincipal {
            name: cell_str(row, 0),
            host: None,
            kind,
            is_super,
            can_login: kind == PrincipalKind::User,
            comment: Some(cell_str(row, 1)),
        };
        let mut attributes: Vec<DetailAttribute> = Vec::new();
        let kv = |k: &str, v: String| DetailAttribute { key: k.to_string(), value: v };
        attributes.push(kv("Type", cell_str(row, 1)));
        if let Some(v) = cell_opt_str(row, 2).filter(|v| !v.is_empty()) {
            attributes.push(kv("Default schema", v));
        }
        if let Some(v) = cell_opt_str(row, 3).filter(|v| !v.is_empty()) {
            attributes.push(kv("Created", v));
        }

        // Database-level, schema-level and object permissions with grant state.
        let mut object_privileges: Vec<PrivilegeEntry> = Vec::new();
        // Object level (class = 1): schema + object name resolved via sys.objects.
        if let Ok(prows) = self
            .fetch_rows(
                "SELECT sch.name, OBJECT_NAME(p.major_id), p.permission_name, p.state_desc \
                 FROM sys.database_permissions p \
                 JOIN sys.database_principals u ON u.principal_id = p.grantee_principal_id \
                 JOIN sys.objects o ON o.object_id = p.major_id \
                 JOIN sys.schemas sch ON sch.schema_id = o.schema_id \
                 WHERE u.name = @P1 AND p.class = 1 ORDER BY 1, 2",
                vec![Value::Text(name.to_string())],
            )
            .await
        {
            let mut grouped: std::collections::BTreeMap<(String, String), (Vec<String>, bool)> =
                std::collections::BTreeMap::new();
            for r in prows {
                let key = (cell_str(&r, 0), cell_str(&r, 1));
                let e = grouped.entry(key).or_default();
                e.0.push(cell_str(&r, 2));
                if cell_str(&r, 3) == "GRANT_WITH_GRANT_OPTION" {
                    e.1 = true;
                }
            }
            for ((schema, table), (mut privs, grantable)) in grouped {
                privs.sort();
                privs.dedup();
                object_privileges.push(PrivilegeEntry {
                    privileges: privs,
                    database: None,
                    schema: Some(schema),
                    table: Some(table),
                    grantable,
                });
            }
        }
        // Database level (class = 0) and schema level (class = 3).
        if let Ok(prows) = self
            .fetch_rows(
                "SELECT p.class, s.name, p.permission_name, p.state_desc \
                 FROM sys.database_permissions p \
                 JOIN sys.database_principals u ON u.principal_id = p.grantee_principal_id \
                 LEFT JOIN sys.schemas s ON s.schema_id = p.major_id AND p.class = 3 \
                 WHERE u.name = @P1 AND p.class IN (0, 3) ORDER BY 1",
                vec![Value::Text(name.to_string())],
            )
            .await
        {
            let mut grouped: std::collections::BTreeMap<(String, String), (Vec<String>, bool)> =
                std::collections::BTreeMap::new();
            for r in prows {
                // class 0 has no name; use the current database as the label.
                let key = (cell_str(&r, 0), cell_opt_str(&r, 1).unwrap_or_default());
                let e = grouped.entry(key).or_default();
                e.0.push(cell_str(&r, 2));
                if cell_str(&r, 3) == "GRANT_WITH_GRANT_OPTION" {
                    e.1 = true;
                }
            }
            for ((class, name_), (mut privs, grantable)) in grouped {
                privs.sort();
                privs.dedup();
                let (database, schema) = if class == "0" {
                    (self.resolve_db(&Scope { database: None, schema: None }).map(str::to_string), None)
                } else {
                    (None, Some(name_))
                };
                object_privileges.push(PrivilegeEntry {
                    privileges: privs,
                    database,
                    schema,
                    table: None,
                    grantable,
                });
            }
        }

        let all_members = self.db_role_members().await.unwrap_or_default();
        let member_of = all_members.iter().filter(|g| g.member == name).cloned().collect();
        let members = all_members.iter().filter(|g| g.role == name).cloned().collect();

        Ok(DbPrincipalDetail {
            principal,
            attributes,
            global_privileges: Vec::new(),
            object_privileges,
            member_of,
            members,
        })
    }

    pub async fn create_db_user(&mut self, req: &CreateDbUser) -> MieoResult<()> {
        let login = quote_sqlserver_ident(&req.name);
        if let Some(pw) = req.password.as_deref().filter(|p| !p.is_empty()) {
            self.simple_exec(&format!(
                "CREATE LOGIN {login} WITH PASSWORD = {}",
                quote_literal(pw)
            ))
            .await?;
            self.simple_exec(&format!("CREATE USER {login} FOR LOGIN {login}"))
                .await?;
        } else {
            self.simple_exec(&format!("CREATE USER {login} WITHOUT LOGIN"))
                .await?;
        }
        if req.superuser {
            self.simple_exec(&format!("ALTER SERVER ROLE [sysadmin] ADD MEMBER {login}"))
                .await?;
        }
        Ok(())
    }

    pub async fn drop_db_principal(&mut self, req: &DropDbPrincipal) -> MieoResult<()> {
        let name = quote_sqlserver_ident(&req.name);
        match req.kind {
            PrincipalKind::User => {
                // Drop the database user; a same-named login is dropped too
                // when it exists (best effort).
                self.simple_exec(&format!("DROP USER {name}")).await?;
                let _ = self.simple_exec(&format!("DROP LOGIN {name}")).await;
                Ok(())
            }
            PrincipalKind::Role => {
                self.simple_exec(&format!("DROP ROLE {name}")).await
            }
        }
    }

    fn grant_object_ms(
        database: Option<&str>,
        schema: Option<&str>,
        table: Option<&str>,
    ) -> MieoResult<String> {
        match (schema.filter(|s| !s.is_empty()), table.filter(|t| !t.is_empty())) {
            (Some(sc), Some(tbl)) => Ok(format!(
                "OBJECT::{}.{}",
                quote_sqlserver_ident(sc),
                quote_sqlserver_ident(tbl)
            )),
            _ => match database.filter(|d| !d.is_empty()) {
                Some(_) => Ok("DATABASE".to_string()),
                None => Err(MieoError::new(
                    ErrorCode::Data,
                    "choose a table or the current database for this grant",
                )),
            },
        }
    }

    pub async fn grant(&mut self, req: &GrantRequest) -> MieoResult<()> {
        let grantee = quote_sqlserver_ident(&req.grantee);
        let stmt = match &req.target {
            GrantTarget::Privileges {
                privileges,
                database,
                schema,
                table,
            } => {
                let object = Self::grant_object_ms(
                    database.as_deref(),
                    schema.as_deref().or(Some("dbo")),
                    table.as_deref(),
                )?;
                let mut s = format!("GRANT {privileges} ON {object} TO {grantee}");
                if req.with_grant_option {
                    s.push_str(" WITH GRANT OPTION");
                }
                s
            }
            GrantTarget::Role { role, .. } => {
                format!(
                    "ALTER ROLE {} ADD MEMBER {grantee}",
                    quote_sqlserver_ident(role)
                )
            }
        };
        self.simple_exec(&stmt).await
    }

    pub async fn revoke(&mut self, req: &RevokeRequest) -> MieoResult<()> {
        let grantee = quote_sqlserver_ident(&req.grantee);
        let stmt = match &req.target {
            GrantTarget::Privileges {
                privileges,
                database,
                schema,
                table,
            } => {
                let object = Self::grant_object_ms(
                    database.as_deref(),
                    schema.as_deref().or(Some("dbo")),
                    table.as_deref(),
                )?;
                format!("REVOKE {privileges} ON {object} FROM {grantee}")
            }
            GrantTarget::Role { role, .. } => {
                format!(
                    "ALTER ROLE {} DROP MEMBER {grantee}",
                    quote_sqlserver_ident(role)
                )
            }
        };
        self.simple_exec(&stmt).await
    }

    pub async fn set_user_password(
        &mut self,
        name: &str,
        _host: Option<&str>,
        password: &str,
    ) -> MieoResult<()> {
        self.simple_exec(&format!(
            "ALTER LOGIN {} WITH PASSWORD = {}",
            quote_sqlserver_ident(name),
            quote_literal(password)
        ))
        .await
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        if database.trim().is_empty() {
            return Err(MieoError::new(
                ErrorCode::Data,
                "database name must not be empty",
            ));
        }
        if self.tx_open {
            return Err(MieoError::new(
                ErrorCode::Conflict,
                "cannot switch database inside an open transaction",
            ));
        }
        let stmt = format!("USE {}", quote_sqlserver_ident(database));
        self.simple_exec(&stmt).await?;
        self.spec.database = Some(database.to_string());
        Ok(())
    }

    pub async fn begin(&mut self) -> MieoResult<()> {
        if self.tx_open {
            return Err(MieoError::new(
                ErrorCode::Conflict,
                "a transaction is already open on this session",
            ));
        }
        self.client
            .begin_transaction()
            .await
            .map_err(map_tiberius_err)?;
        self.tx_open = true;
        Ok(())
    }

    pub async fn commit(&mut self) -> MieoResult<()> {
        if !self.tx_open {
            return Err(MieoError::new(
                ErrorCode::Conflict,
                "no open transaction to commit",
            ));
        }
        self.client
            .commit_transaction()
            .await
            .map_err(map_tiberius_err)?;
        self.tx_open = false;
        Ok(())
    }

    pub async fn rollback(&mut self) -> MieoResult<()> {
        if !self.tx_open {
            return Err(MieoError::new(
                ErrorCode::Conflict,
                "no open transaction to roll back",
            ));
        }
        self.client
            .rollback_transaction()
            .await
            .map_err(map_tiberius_err)?;
        self.tx_open = false;
        Ok(())
    }

    pub async fn run_query(
        &mut self,
        query_id: &str,
        sql: String,
        params: Vec<Value>,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        // tiberius' row stream does not expose DONE tokens, so the number of
        // affected rows is only available through the `execute` API. Route
        // plain DML there; everything that may return rows (OUTPUT clauses,
        // batches with several statements, SELECTs) goes through the stream.
        if wants_affected_count(&sql) {
            self.run_affected(query_id, sql, params, options, sink, cancel)
                .await
        } else {
            self.run_streaming(query_id, sql, params, options, sink, cancel)
                .await
        }
    }

    /// `SELECT * FROM [schema].[table]` preview used by "Open data grid".
    pub fn preview_statement(&self, scope: &Scope, table: &str, limit: u64) -> MieoResult<String> {
        let _ = (scope, table, limit);
        Err(MieoError::unsupported(
            "SQL Server preview statements are generated by the API layer (`SELECT TOP (n) ...`)",
        ))
    }

    pub fn engine(&self) -> Engine {
        Engine::SqlServer
    }

    // ------------------------------------------------------------------
    // Private execution helpers
    // ------------------------------------------------------------------

    /// Streamed (row-returning) execution path.
    async fn run_streaming(
        &mut self,
        query_id: &str,
        sql: String,
        params: Vec<Value>,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        let start = Instant::now();
        let fetch_size = options.fetch_size.max(1) as usize;
        let deadline = options
            .timeout_ms
            .map(|ms| TokioInstant::now() + Duration::from_millis(ms));

        let mut stream = if options.mode == QueryMode::Simple {
            if !params.is_empty() {
                return Err(MieoError::new(
                    ErrorCode::Data,
                    "bound parameters are not supported in simple (unprepared) mode",
                ));
            }
            self.client
                .simple_query(sql)
                .await
                .map_err(map_tiberius_err)?
        } else {
            let (rewritten, placeholders) = rewrite_placeholders(&sql);
            if placeholders != params.len() {
                return Err(param_mismatch(placeholders, params.len()));
            }
            let mut query = Query::new(rewritten);
            for value in &params {
                query.bind(value_to_column_data(value)?);
            }
            query
                .query(&mut self.client)
                .await
                .map_err(map_tiberius_err)?
        };

        let mut stats = QueryStats::default();
        let mut batch: Vec<Vec<Value>> = Vec::with_capacity(fetch_size.min(4096));
        let mut batch_index: u64 = 0;
        let mut current_sig: Vec<String> = Vec::new();
        let mut truncated = false;

        // In prepared mode the statement is described up front so the UI gets
        // column metadata even when the result set is empty.
        if options.mode == QueryMode::Prepared {
            match stream.columns().await {
                Ok(Some(columns)) => {
                    let (columns, sig) = columns_meta(columns);
                    if !columns.is_empty() {
                        current_sig = sig;
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
                Ok(None) => {}
                Err(e) => return Err(map_tiberius_err(e)),
            }
        }

        enum Step {
            Next(Option<tiberius::Result<QueryItem>>),
            Cancelled,
            TimedOut,
        }

        loop {
            let step = match deadline {
                Some(deadline) => tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Step::Cancelled,
                    _ = tokio::time::sleep_until(deadline) => Step::TimedOut,
                    item = stream.next() => Step::Next(item),
                },
                None => tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Step::Cancelled,
                    item = stream.next() => Step::Next(item),
                },
            };

            match step {
                Step::Cancelled => {
                    // Dropping a result stream mid-response leaves the TDS
                    // connection out of sync; send Attention and drain the
                    // acknowledgement so the session stays reusable.
                    drop(stream);
                    let _ = self.client.cancel_query().await;
                    return Err(MieoError::cancelled());
                }
                Step::TimedOut => {
                    drop(stream);
                    let _ = self.client.cancel_query().await;
                    return Err(MieoError::new(ErrorCode::Timeout, "query timed out"));
                }
                Step::Next(None) => break,
                Step::Next(Some(item)) => {
                    let item = item.map_err(map_tiberius_err)?;
                    match item {
                        QueryItem::Metadata(metadata) => {
                            let (columns, sig) = columns_meta(metadata.columns());
                            if sig != current_sig {
                                current_sig = sig;
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
                        QueryItem::Row(row) => {
                            let within_limit =
                                options.max_rows.is_none_or(|m| stats.rows < m);
                            if within_limit {
                                stats.rows += 1;
                                batch.push(decode_row(&row));
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
                                // Drain silently to keep the connection clean.
                                truncated = true;
                            }
                        }
                    }
                }
            }
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

    /// DML execution path that reports the number of affected rows.
    async fn run_affected(
        &mut self,
        query_id: &str,
        sql: String,
        params: Vec<Value>,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        let start = Instant::now();
        let deadline = options
            .timeout_ms
            .map(|ms| TokioInstant::now() + Duration::from_millis(ms));

        enum Outcome {
            Done(MieoResult<tiberius::ExecuteResult>),
            Cancelled,
            TimedOut,
        }

        let outcome = match deadline {
            Some(deadline) => tokio::select! {
                biased;
                _ = cancel.cancelled() => Outcome::Cancelled,
                _ = tokio::time::sleep_until(deadline) => Outcome::TimedOut,
                result = execute_impl(&mut self.client, &sql, &params, options.mode) => {
                    Outcome::Done(result)
                }
            },
            None => tokio::select! {
                biased;
                _ = cancel.cancelled() => Outcome::Cancelled,
                result = execute_impl(&mut self.client, &sql, &params, options.mode) => {
                    Outcome::Done(result)
                }
            },
        };

        let affected = match outcome {
            Outcome::Cancelled => {
                let _ = self.client.cancel_query().await;
                return Err(MieoError::cancelled());
            }
            Outcome::TimedOut => {
                let _ = self.client.cancel_query().await;
                return Err(MieoError::new(ErrorCode::Timeout, "query timed out"));
            }
            Outcome::Done(result) => result?,
        };

        let rows_affected = affected.total();
        let stats = QueryStats {
            rows: 0,
            rows_affected,
            elapsed_ms: start.elapsed().as_millis() as u64,
            truncated: false,
            // SQL Server has no session-independent last-insert-id; the caller
            // can query `SCOPE_IDENTITY()` in the same batch when needed.
            last_insert_id: None,
        };
        if rows_affected > 0 {
            send_evt(
                sink,
                QueryEvent::Affected {
                    query_id: query_id.to_string(),
                    rows_affected,
                    last_insert_id: None,
                },
            )
            .await?;
        }
        Ok(stats)
    }

    /// Execute an introspection statement and collect its first result set.
    async fn fetch_rows(&mut self, sql: &str, params: Vec<Value>) -> MieoResult<Vec<Row>> {
        let mut query = Query::new(sql.to_string());
        for value in &params {
            query.bind(value_to_column_data(value)?);
        }
        let stream = query
            .query(&mut self.client)
            .await
            .map_err(map_tiberius_err)?;
        stream.into_first_result().await.map_err(map_tiberius_err)
    }

    /// Run a batch without binding parameters and drain its result stream.
    async fn simple_exec(&mut self, sql: &str) -> MieoResult<()> {
        let mut stream = self
            .client
            .simple_query(sql.to_string())
            .await
            .map_err(map_tiberius_err)?;
        while let Some(item) = stream.next().await {
            item.map_err(map_tiberius_err)?;
        }
        Ok(())
    }

    fn resolve_db<'a>(&'a self, scope: &'a Scope) -> Option<&'a str> {
        scope
            .database
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.spec.database.as_deref().filter(|s| !s.is_empty()))
    }

    fn resolve_schema<'a>(&'a self, scope: &'a Scope) -> &'a str {
        scope
            .schema
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("dbo")
    }

    /// `sys.objects.type` codes surfaced under each explorer kind
    /// (FN scalar function, IF inline table function, TF table function,
    /// P procedure, TR trigger, V view). `None` for table-like kinds that
    /// are listed from `INFORMATION_SCHEMA` instead.
    fn sys_types_for_kind(kind: ObjectKind) -> Option<&'static str> {
        match kind {
            ObjectKind::Function => Some("'FN', 'IF', 'TF'"),
            ObjectKind::Procedure => Some("'P'"),
            ObjectKind::Trigger => Some("'TR'"),
            ObjectKind::View => Some("'V'"),
            _ => None,
        }
    }

    /// Schema owning `name` for a view / routine / trigger kind, used by
    /// `object_ddl` when the scope carries no schema.
    async fn resolve_object_schema(
        &mut self,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let Some(in_list) = Self::sys_types_for_kind(kind) else {
            return Ok(None);
        };
        let prefix = db_prefix(self.resolve_db(scope));
        let sql = format!(
            "SELECT TOP (1) s.name FROM {prefix}sys.objects o \
             JOIN {prefix}sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.name = @P1 AND o.type IN ({in_list}) ORDER BY s.name"
        );
        let rows = self
            .fetch_rows(&sql, vec![Value::Text(name.to_string())])
            .await?;
        Ok(rows.first().map(|row| cell_str(row, 0)))
    }

    /// `[db].[schema].[name]` (or `[schema].[name]` when no database is set).
    fn qualified_name(&self, scope: &Scope, name: &str) -> String {
        match self.resolve_db(scope) {
            Some(db) => format!(
                "{}.{}.{}",
                quote_sqlserver_ident(db),
                quote_sqlserver_ident(self.resolve_schema(scope)),
                quote_sqlserver_ident(name)
            ),
            None => format!(
                "{}.{}",
                quote_sqlserver_ident(self.resolve_schema(scope)),
                quote_sqlserver_ident(name)
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// TLS / error mapping
// ---------------------------------------------------------------------------

/// Configure the TDS encryption level for the requested [`SslMode`].
fn apply_tls(config: &mut Config, ssl_mode: SslMode) {
    match ssl_mode {
        // Never negotiate TLS.
        SslMode::Disable => config.encryption(EncryptionLevel::NotSupported),
        // Encrypt everything when the server supports it, fall back to a
        // login-only encrypted handshake otherwise.
        SslMode::Prefer => config.encryption(EncryptionLevel::On),
        // Require TLS but accept any certificate (self-signed certs are common
        // on on-premise instances and on Azure SQL with custom hostnames).
        SslMode::Require => {
            config.encryption(EncryptionLevel::Required);
            config.trust_cert();
        }
        // tiberius/rustls validates chain *and* hostname with the OS trust
        // store and has no CA-only switch, so VerifyCa is best-effort and
        // behaves like VerifyFull.
        SslMode::VerifyCa | SslMode::VerifyFull => {
            config.encryption(EncryptionLevel::Required);
        }
    }
}

/// Map a tiberius error into the structured [`MieoError`].
fn map_tiberius_err(error: TiberiusError) -> MieoError {
    match &error {
        TiberiusError::Server(token) => {
            let code = token.code();
            let mut mapped = MieoError::new(
                classify_sqlserver(code, token.message(), token.class()),
                token.message(),
            )
            .with_db_code(code.to_string());
            if token.line() > 0 {
                mapped = mapped.with_position(token.line());
            }
            if !token.procedure().is_empty() {
                mapped = mapped.with_detail(format!("procedure: {}", token.procedure()));
            }
            mapped.with_source(error)
        }
        TiberiusError::Io { kind, message } => {
            let code = match kind {
                std::io::ErrorKind::TimedOut => ErrorCode::Timeout,
                _ => ErrorCode::Network,
            };
            MieoError::new(code, format!("I/O error: {message}")).with_source(error)
        }
        TiberiusError::Tls(message) => {
            MieoError::new(ErrorCode::Network, format!("TLS error: {message}")).with_source(error)
        }
        TiberiusError::Protocol(message) => {
            MieoError::new(ErrorCode::Connection, format!("protocol error: {message}"))
                .with_source(error)
        }
        TiberiusError::Encoding(message) => {
            MieoError::new(ErrorCode::Data, format!("encoding error: {message}"))
                .with_source(error)
        }
        TiberiusError::Conversion(message) => {
            MieoError::new(ErrorCode::Data, format!("conversion error: {message}"))
                .with_source(error)
        }
        TiberiusError::Utf8 | TiberiusError::Utf16 => {
            MieoError::new(ErrorCode::Data, format!("text decoding error: {error}"))
                .with_source(error)
        }
        TiberiusError::ParseInt(source) => {
            MieoError::new(ErrorCode::Data, format!("integer parse error: {source}"))
                .with_source(error)
        }
        TiberiusError::Routing { host, port } => MieoError::new(
            ErrorCode::Connection,
            format!("server requested routing to {host}:{port}; reconnect to that address"),
        )
        .with_source(error),
        TiberiusError::BulkInput(message) => {
            MieoError::new(ErrorCode::Data, format!("bulk input error: {message}"))
                .with_source(error)
        }
    }
}

/// Classify a SQL Server error number (plus severity/message heuristics).
fn classify_sqlserver(code: u32, message: &str, class: u8) -> ErrorCode {
    match code {
        // Login failed / cannot open requested database / login disabled.
        18456 | 18452 | 18470 | 18487 | 18488 | 4060 => ErrorCode::Auth,
        229 | 230 | 262 | 297 => ErrorCode::Permission,
        102 | 105 | 106 | 107 | 156 | 170 | 319 | 1038 => ErrorCode::Syntax,
        207 | 208 | 2812 | 4902 | 1088 | 15151 => ErrorCode::NotFound,
        2601 | 2627 => ErrorCode::Constraint,
        515 | 547 | 2628 | 8152 | 8153 => ErrorCode::Constraint,
        2714 | 2705 | 1505 => ErrorCode::Conflict,
        1205 => ErrorCode::Conflict,
        245 | 241 | 8114 | 8115 => ErrorCode::Data,
        53 | 40 | 64 | 233 | 10054 | 10060 | 11001 => ErrorCode::Connection,
        _ => {
            // Severity >= 20 is a fatal error that terminates the connection.
            if class >= 20 {
                return ErrorCode::Connection;
            }
            let msg = message.to_ascii_lowercase();
            if msg.contains("login failed") || msg.contains("authentication") {
                ErrorCode::Auth
            } else if msg.contains("permission") || msg.contains("denied") {
                ErrorCode::Permission
            } else if msg.contains("syntax") {
                ErrorCode::Syntax
            } else if msg.contains("invalid object")
                || msg.contains("invalid column")
                || msg.contains("could not find")
                || msg.contains("does not exist")
            {
                ErrorCode::NotFound
            } else if msg.contains("duplicate")
                || msg.contains("constraint")
                || msg.contains("truncat")
            {
                ErrorCode::Constraint
            } else if msg.contains("deadlock") {
                ErrorCode::Conflict
            } else if msg.contains("timeout") || msg.contains("timed out") {
                ErrorCode::Timeout
            } else if msg.contains("conversion") {
                ErrorCode::Data
            } else {
                ErrorCode::Internal
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SQL text scanning (placeholders, keywords)
// ---------------------------------------------------------------------------

/// A single lexical pass over a T-SQL text.
#[derive(Debug, Default)]
struct SqlScan {
    /// Byte offsets of `?` placeholders outside strings/identifiers/comments.
    placeholders: Vec<usize>,
    /// Byte offsets of top-level `;` statement separators.
    semicolons: Vec<usize>,
    /// `(byte offset, UPPERCASED word)` for identifier-like words outside
    /// strings/identifiers/comments.
    words: Vec<(usize, String)>,
}

/// Scan `sql`, skipping `'...'`/`N'...'` strings, `"..."` identifiers,
/// `[...]` identifiers and `--` / nested `/* ... */` comments.
fn scan_sql(sql: &str) -> SqlScan {
    let mut scan = SqlScan::default();
    let mut chars = sql.char_indices().peekable();

    while let Some((offset, ch)) = chars.next() {
        match ch {
            '\'' => skip_quoted(&mut chars, '\''),
            '"' => skip_quoted(&mut chars, '"'),
            '[' => {
                // T-SQL bracket identifier: `]]` escapes a literal `]`.
                while let Some((_, c)) = chars.next() {
                    if c == ']' {
                        if chars.peek().map(|(_, c)| *c) == Some(']') {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            '-' if chars.peek().map(|(_, c)| *c) == Some('-') => {
                for (_, c) in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek().map(|(_, c)| *c) == Some('*') => {
                chars.next(); // consume `*`
                // T-SQL block comments nest.
                let mut depth = 1usize;
                while depth > 0 {
                    match chars.next() {
                        Some((_, '*')) if chars.peek().map(|(_, c)| *c) == Some('/') => {
                            chars.next();
                            depth -= 1;
                        }
                        Some((_, '/')) if chars.peek().map(|(_, c)| *c) == Some('*') => {
                            chars.next();
                            depth += 1;
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
            }
            '?' => scan.placeholders.push(offset),
            ';' => scan.semicolons.push(offset),
            c if c.is_ascii_alphanumeric() || c == '_' => {
                let mut word = String::from(c);
                while let Some((_, next)) = chars.peek() {
                    if next.is_ascii_alphanumeric() || *next == '_' {
                        word.push(*next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                scan.words.push((offset, word.to_ascii_uppercase()));
            }
            _ => {}
        }
    }

    scan
}

fn skip_quoted(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>, quote: char) {
    while let Some((_, c)) = chars.next() {
        if c == quote {
            if chars.peek().map(|(_, c)| *c) == Some(quote) {
                chars.next(); // doubled quote escape
            } else {
                break;
            }
        }
    }
}

/// Rewrite `?` placeholders to tiberius' `@P1..` names, returning the new SQL
/// and the number of placeholders found.
fn rewrite_placeholders(sql: &str) -> (String, usize) {
    let scan = scan_sql(sql);
    if scan.placeholders.is_empty() {
        return (sql.to_string(), 0);
    }
    let mut out = String::with_capacity(sql.len() + scan.placeholders.len() * 3);
    let mut last = 0usize;
    for (index, &offset) in scan.placeholders.iter().enumerate() {
        out.push_str(&sql[last..offset]);
        out.push_str("@P");
        out.push_str(&(index + 1).to_string());
        last = offset + 1; // `?` is a single ASCII byte
    }
    out.push_str(&sql[last..]);
    (out, scan.placeholders.len())
}

fn param_mismatch(expected: usize, got: usize) -> MieoError {
    MieoError::new(
        ErrorCode::Data,
        format!(
            "statement has {expected} `?` placeholder(s) but {got} parameter(s) were supplied"
        ),
    )
}

/// True when a statement only produces affected-row counts, i.e. plain DML
/// without an `OUTPUT` clause and without additional statements in the batch.
fn wants_affected_count(sql: &str) -> bool {
    let scan = scan_sql(sql);
    let Some((_, first)) = scan.words.first() else {
        return false;
    };
    if !matches!(first.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE") {
        return false;
    }
    let has_output = scan.words.iter().any(|(_, word)| word == "OUTPUT");
    let multiple = scan
        .semicolons
        .iter()
        .any(|separator| scan.words.iter().any(|(offset, _)| offset > separator));
    !has_output && !multiple
}

// ---------------------------------------------------------------------------
// Parameter binding (Value -> tiberius)
// ---------------------------------------------------------------------------

fn data_err(message: impl Into<String>) -> MieoError {
    MieoError::new(ErrorCode::Data, message)
}

/// Parse a plain decimal string into tiberius' fixed-point [`Numeric`].
fn parse_decimal_numeric(s: &str) -> MieoResult<Numeric> {
    let trimmed = s.trim();
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let (int_part, frac_part) = match digits.split_once('.') {
        Some((int_part, frac_part)) => (int_part, frac_part),
        None => (digits, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(data_err(format!("invalid decimal value `{s}`")));
    }
    if !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return Err(data_err(format!("invalid decimal value `{s}`")));
    }
    if frac_part.len() > Numeric::MAX_NUMERIC_SCALE as usize {
        return Err(data_err(format!(
            "decimal value `{s}` has more than {} fractional digits",
            Numeric::MAX_NUMERIC_SCALE
        )));
    }
    let scale = frac_part.len() as u8;
    let combined = format!("{int_part}{frac_part}");
    let combined = combined.trim_start_matches('0');
    let magnitude: i128 = if combined.is_empty() {
        0
    } else {
        combined.parse::<i128>().map_err(|_| {
            data_err(format!(
                "decimal value `{s}` exceeds the SQL Server numeric range"
            ))
        })?
    };
    Ok(Numeric::new_with_scale(
        if negative { -magnitude } else { magnitude },
        scale,
    ))
}

/// Render a tiberius [`Numeric`] as a plain decimal string (no exponent,
/// trailing zeros preserved by scale).
fn numeric_to_string(value: &Numeric) -> String {
    let raw = value.value();
    let scale = value.scale() as usize;
    let negative = raw < 0;
    let magnitude = raw.unsigned_abs();
    let divisor = 10u128.pow(scale as u32);
    let integer = magnitude / divisor;
    let fraction = magnitude % divisor;

    let mut out = String::new();
    if negative && magnitude != 0 {
        out.push('-');
    }
    out.push_str(&integer.to_string());
    if scale > 0 {
        out.push('.');
        out.push_str(&format!("{:0>width$}", fraction, width = scale));
    }
    out
}

/// Map a wire [`Value`] to a tiberius bind parameter.
fn value_to_column_data(value: &Value) -> MieoResult<ColumnData<'static>> {
    Ok(match value {
        // `NULL` is typed by the destination column; nvarchar is a safe carrier.
        Value::Null => ColumnData::String(None),
        Value::Bool(b) => ColumnData::Bit(Some(*b)),
        Value::Int(i) => ColumnData::I64(Some(*i)),
        Value::UInt(u) => match i64::try_from(*u) {
            Ok(i) => ColumnData::I64(Some(i)),
            // SQL Server has no unsigned integers; values above i64 go out as
            // a 38-digit fixed-point number.
            Err(_) => ColumnData::Numeric(Some(Numeric::new_with_scale(*u as i128, 0))),
        },
        Value::Float(f) => ColumnData::F64(Some(*f)),
        Value::Decimal(s) => ColumnData::Numeric(Some(parse_decimal_numeric(s)?)),
        Value::Text(s) => ColumnData::String(Some(Cow::Owned(s.clone()))),
        Value::Bytes(b) => ColumnData::Binary(Some(Cow::Owned(b64_decode(b)?))),
        Value::Date(s) => {
            let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|e| data_err(format!("invalid date value `{s}`: {e}")))?;
            let data: ColumnData<'static> = date.into_sql();
            data
        }
        Value::Time(s) => {
            let time = NaiveTime::parse_from_str(s, "%H:%M:%S%.f")
                .map_err(|e| data_err(format!("invalid time value `{s}`: {e}")))?;
            let data: ColumnData<'static> = time.into_sql();
            data
        }
        Value::DateTime(s) => {
            let data: ColumnData<'static> = if let Ok(offset) = DateTime::parse_from_rfc3339(s) {
                offset.into_sql()
            } else if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
                naive.into_sql()
            } else if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
                naive.into_sql()
            } else {
                return Err(data_err(format!("invalid datetime value `{s}`")));
            };
            data
        }
        Value::Json(json) => ColumnData::String(Some(Cow::Owned(json.to_string()))),
        Value::Uuid(s) => ColumnData::Guid(Some(
            uuid::Uuid::parse_str(s)
                .map_err(|e| data_err(format!("invalid uuid value `{s}`: {e}")))?,
        )),
        Value::Array(_) | Value::Unsupported { .. } => {
            return Err(data_err(format!(
                "cannot bind value of type `{}` as a parameter",
                value.type_name()
            )));
        }
    })
}

/// Execute an affected-rows statement (simple batch or prepared RPC).
async fn execute_impl(
    client: &mut TdsClient,
    sql: &str,
    params: &[Value],
    mode: QueryMode,
) -> MieoResult<tiberius::ExecuteResult> {
    if mode == QueryMode::Simple {
        if !params.is_empty() {
            return Err(MieoError::new(
                ErrorCode::Data,
                "bound parameters are not supported in simple (unprepared) mode",
            ));
        }
        let no_params: &[&dyn tiberius::ToSql] = &[];
        return client
            .execute(sql.to_string(), no_params)
            .await
            .map_err(map_tiberius_err);
    }

    let (rewritten, placeholders) = rewrite_placeholders(sql);
    if placeholders != params.len() {
        return Err(param_mismatch(placeholders, params.len()));
    }
    let mut query = Query::new(rewritten);
    for value in params {
        query.bind(value_to_column_data(value)?);
    }
    query.execute(client).await.map_err(map_tiberius_err)
}

// ---------------------------------------------------------------------------
// Result decoding (tiberius -> Value)
// ---------------------------------------------------------------------------

/// Native type name for a wire column type.
fn column_type_name(column_type: ColumnType) -> &'static str {
    match column_type {
        ColumnType::Null => "NULL",
        ColumnType::Bit | ColumnType::Bitn => "BIT",
        ColumnType::Int1 => "TINYINT",
        ColumnType::Int2 => "SMALLINT",
        ColumnType::Int4 => "INT",
        ColumnType::Int8 => "BIGINT",
        ColumnType::Intn => "INT",
        ColumnType::Datetime4 => "SMALLDATETIME",
        ColumnType::Datetime | ColumnType::Datetimen => "DATETIME",
        ColumnType::Float4 => "REAL",
        ColumnType::Float8 | ColumnType::Floatn => "FLOAT",
        ColumnType::Money | ColumnType::Money4 => "MONEY",
        ColumnType::Guid => "UNIQUEIDENTIFIER",
        ColumnType::Decimaln => "DECIMAL",
        ColumnType::Numericn => "NUMERIC",
        ColumnType::Daten => "DATE",
        ColumnType::Timen => "TIME",
        ColumnType::Datetime2 => "DATETIME2",
        ColumnType::DatetimeOffsetn => "DATETIMEOFFSET",
        ColumnType::BigVarBin => "VARBINARY",
        ColumnType::BigBinary => "BINARY",
        ColumnType::Image => "IMAGE",
        ColumnType::BigVarChar => "VARCHAR",
        ColumnType::BigChar => "CHAR",
        ColumnType::Text => "TEXT",
        ColumnType::NVarchar => "NVARCHAR",
        ColumnType::NChar => "NCHAR",
        ColumnType::NText => "NTEXT",
        ColumnType::Xml => "XML",
        ColumnType::Udt => "UDT",
        ColumnType::SSVariant => "SQL_VARIANT",
    }
}

/// Normalized kind from a native SQL Server type name (e.g. `NVARCHAR(50)`).
fn kind_sqlserver(native: &str) -> DataTypeKind {
    let base = native
        .split('(')
        .next()
        .unwrap_or(native)
        .trim()
        .to_ascii_uppercase();
    match base.as_str() {
        "TINYINT" | "SMALLINT" | "INT" | "BIGINT" => DataTypeKind::Int,
        "BIT" => DataTypeKind::Bool,
        "DECIMAL" | "NUMERIC" | "MONEY" | "SMALLMONEY" => DataTypeKind::Decimal,
        "FLOAT" | "REAL" => DataTypeKind::Float,
        "CHAR" | "VARCHAR" | "NCHAR" | "NVARCHAR" | "TEXT" | "NTEXT" => DataTypeKind::Text,
        "BINARY" | "VARBINARY" | "IMAGE" => DataTypeKind::Bytes,
        "DATE" => DataTypeKind::Date,
        "TIME" => DataTypeKind::Time,
        "DATETIME" | "DATETIME2" | "SMALLDATETIME" => DataTypeKind::DateTime,
        "DATETIMEOFFSET" => DataTypeKind::TimestampTz,
        "UNIQUEIDENTIFIER" => DataTypeKind::Uuid,
        "XML" | "JSON" => DataTypeKind::Json,
        "GEOGRAPHY" | "GEOMETRY" => DataTypeKind::Geometry,
        _ => DataTypeKind::Unknown,
    }
}

fn columns_meta(columns: &[Column]) -> (Vec<ColumnMeta>, Vec<String>) {
    let metas: Vec<ColumnMeta> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let native_type = column_type_name(column.column_type());
            ColumnMeta {
                name: column.name().to_string(),
                ordinal: index as u32,
                native_type: native_type.to_string(),
                kind: kind_sqlserver(native_type),
                nullable: None,
            }
        })
        .collect();
    let signature = metas.iter().map(|column| column.name.clone()).collect();
    (metas, signature)
}

fn decode_row(row: &Row) -> Vec<Value> {
    row.cells()
        .map(|(column, data)| decode_cell(column, data))
        .collect()
}

fn decode_cell(column: &Column, data: &ColumnData<'static>) -> Value {
    let native_type = column_type_name(column.column_type());
    let unsupported = |reason: String| Value::Unsupported {
        native_type: native_type.to_string(),
        reason,
    };

    match data {
        ColumnData::U8(Some(v)) => Value::Int(i64::from(*v)),
        ColumnData::I16(Some(v)) => Value::Int(i64::from(*v)),
        ColumnData::I32(Some(v)) => Value::Int(i64::from(*v)),
        ColumnData::I64(Some(v)) => Value::Int(*v),
        ColumnData::F32(Some(v)) => Value::Float(f64::from(*v)),
        ColumnData::F64(Some(v)) => Value::Float(*v),
        ColumnData::Bit(Some(v)) => Value::Bool(*v),
        ColumnData::String(Some(s)) => Value::Text(s.to_string()),
        ColumnData::Guid(Some(uuid)) => Value::Uuid(uuid.to_string()),
        ColumnData::Binary(Some(bytes)) => Value::Bytes(b64_encode(bytes)),
        ColumnData::Numeric(Some(numeric)) => Value::Decimal(numeric_to_string(numeric)),
        // SQL Server's XML is not JSON; keep it as text under the Json kind.
        ColumnData::Xml(Some(xml)) => Value::Text(xml.to_string()),
        ColumnData::DateTime(Some(_))
        | ColumnData::SmallDateTime(Some(_))
        | ColumnData::DateTime2(Some(_)) => match NaiveDateTime::from_sql(data) {
            Ok(Some(dt)) => Value::DateTime(fmt_naive_dt(dt)),
            Ok(None) => Value::Null,
            Err(e) => unsupported(e.to_string()),
        },
        ColumnData::Date(Some(_)) => match NaiveDate::from_sql(data) {
            Ok(Some(date)) => Value::Date(fmt_naive_date(date)),
            Ok(None) => Value::Null,
            Err(e) => unsupported(e.to_string()),
        },
        ColumnData::Time(Some(_)) => match NaiveTime::from_sql(data) {
            Ok(Some(time)) => Value::Time(fmt_naive_time(time)),
            Ok(None) => Value::Null,
            Err(e) => unsupported(e.to_string()),
        },
        ColumnData::DateTimeOffset(Some(_)) => match DateTime::<FixedOffset>::from_sql(data) {
            Ok(Some(dt)) => Value::DateTime(dt.to_rfc3339_opts(SecondsFormat::Millis, false)),
            Ok(None) => Value::Null,
            Err(e) => unsupported(e.to_string()),
        },
        // Every remaining arm is the `None` (NULL) form of a known variant.
        _ => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// Introspection helpers
// ---------------------------------------------------------------------------

/// `[db].` prefix for cross-database catalog queries (empty = current database).
fn db_prefix(db: Option<&str>) -> String {
    match db {
        Some(db) => format!("{}.", quote_sqlserver_ident(db)),
        None => String::new(),
    }
}

/// SQL Server identifier quoting: `[name]` with `]` doubling.
fn quote_sqlserver_ident(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

/// Build the full native type text from INFORMATION_SCHEMA fields.
fn format_native_type(
    data_type: &str,
    char_len: Option<i64>,
    precision: Option<i64>,
    scale: Option<i64>,
    datetime_precision: Option<i64>,
) -> String {
    let base = data_type.to_ascii_lowercase();
    match base.as_str() {
        "char" | "varchar" | "nchar" | "nvarchar" | "binary" | "varbinary" => match char_len {
            Some(-1) => format!("{base}(max)"),
            Some(length) if length >= 0 => format!("{base}({length})"),
            _ => base,
        },
        "decimal" | "numeric" => match (precision, scale) {
            (Some(p), Some(s)) => format!("{base}({p},{s})"),
            (Some(p), None) => format!("{base}({p})"),
            _ => base,
        },
        "datetime2" | "time" | "datetimeoffset" => match datetime_precision {
            Some(scale) if scale >= 0 => format!("{base}({scale})"),
            _ => base,
        },
        _ => base,
    }
}

/// Best-effort `CREATE TABLE` generated from introspected column metadata.
fn generate_create_table(qualified: &str, columns: &[ColumnDef]) -> String {
    let mut defs: Vec<String> = Vec::with_capacity(columns.len() + 1);
    let mut primary_key: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    for column in columns {
        let mut def = format!(
            "    {} {}",
            quote_sqlserver_ident(&column.name),
            column.native_type
        );
        if column.is_auto_increment {
            def.push_str(" IDENTITY(1,1)");
        }
        if !column.nullable {
            def.push_str(" NOT NULL");
        }
        if let Some(default) = &column.default_value {
            def.push_str(&format!(" DEFAULT {default}"));
        }
        if column.is_generated {
            notes.push(format!(
                "-- computed column `{}`: expression not available from the catalog",
                column.name
            ));
        }
        defs.push(def);
        if column.is_primary_key {
            primary_key.push(quote_sqlserver_ident(&column.name));
        }
    }

    if !primary_key.is_empty() {
        defs.push(format!("    PRIMARY KEY ({})", primary_key.join(", ")));
    }

    let mut out = format!("CREATE TABLE {qualified} (\n{}\n)", defs.join(",\n"));
    for note in notes {
        out.push('\n');
        out.push_str(&note);
    }
    out
}

// ---------------------------------------------------------------------------
// Row helpers for introspection queries
// ---------------------------------------------------------------------------

fn row_is_sysadmin(row: &Row) -> bool {
    cell_str(row, 0).eq_ignore_ascii_case("sysadmin")
}

fn cell_str(row: &Row, index: usize) -> String {
    row.try_get::<&str, _>(index)
        .ok()
        .flatten()
        .unwrap_or_default()
        .to_string()
}

fn cell_opt_str(row: &Row, index: usize) -> Option<String> {
    row.try_get::<&str, _>(index)
        .ok()
        .flatten()
        .map(str::to_string)
}

/// `try_get` dispatches on the wire type; catalog counters arrive as different
/// integer widths (tinyint/int/bigint), so probe widest first.
fn cell_i64(row: &Row, index: usize) -> Option<i64> {
    if let Ok(Some(v)) = row.try_get::<i64, _>(index) {
        return Some(v);
    }
    if let Ok(Some(v)) = row.try_get::<i32, _>(index) {
        return Some(i64::from(v));
    }
    if let Ok(Some(v)) = row.try_get::<i16, _>(index) {
        return Some(i64::from(v));
    }
    if let Ok(Some(v)) = row.try_get::<u8, _>(index) {
        return Some(i64::from(v));
    }
    None
}

async fn send_evt(sink: &mpsc::Sender<QueryEvent>, event: QueryEvent) -> MieoResult<()> {
    sink.send(event).await.map_err(|_| MieoError::cancelled())
}

// ---------------------------------------------------------------------------
// Unit tests (no server required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- placeholder rewriting -----------------------------------------

    #[test]
    fn rewrite_rewrites_plain_placeholders() {
        assert_eq!(rewrite_placeholders("SELECT ?, ?").0, "SELECT @P1, @P2");
        assert_eq!(rewrite_placeholders("SELECT 1").0, "SELECT 1");
        assert_eq!(rewrite_placeholders("SELECT ?").1, 1);
    }

    #[test]
    fn rewrite_skips_string_literals() {
        assert_eq!(
            rewrite_placeholders("SELECT '?' AS q, ?").0,
            "SELECT '?' AS q, @P1"
        );
        assert_eq!(
            rewrite_placeholders("SELECT 'it''s ?', ?").0,
            "SELECT 'it''s ?', @P1"
        );
        assert_eq!(
            rewrite_placeholders("SELECT N'a?b', ?").0,
            "SELECT N'a?b', @P1"
        );
    }

    #[test]
    fn rewrite_skips_quoted_identifiers() {
        assert_eq!(
            rewrite_placeholders("SELECT [a?b], ?").0,
            "SELECT [a?b], @P1"
        );
        assert_eq!(
            rewrite_placeholders("SELECT [we]]ird?], ?").0,
            "SELECT [we]]ird?], @P1"
        );
        assert_eq!(
            rewrite_placeholders("SELECT \"a?b\", ?").0,
            "SELECT \"a?b\", @P1"
        );
        assert_eq!(
            rewrite_placeholders("SELECT \"a\"\"?b\", ?").0,
            "SELECT \"a\"\"?b\", @P1"
        );
    }

    #[test]
    fn rewrite_skips_comments() {
        assert_eq!(
            rewrite_placeholders("SELECT ? -- trailing ?\n, ?").0,
            "SELECT @P1 -- trailing ?\n, @P2"
        );
        assert_eq!(
            rewrite_placeholders("SELECT ? /* ? */ , ?").0,
            "SELECT @P1 /* ? */ , @P2"
        );
        // T-SQL block comments nest.
        assert_eq!(
            rewrite_placeholders("SELECT ? /* outer /* ? */ ? */ , ?").0,
            "SELECT @P1 /* outer /* ? */ ? */ , @P2"
        );
    }

    #[test]
    fn rewrite_counts_placeholders() {
        let (rewritten, count) = rewrite_placeholders("SELECT ?, '?', [?'?], ? /* ? */");
        assert_eq!(rewritten, "SELECT @P1, '?', [?'?], @P2 /* ? */");
        assert_eq!(count, 2);
    }

    // --- DML classification --------------------------------------------

    #[test]
    fn affected_count_only_for_plain_dml() {
        assert!(wants_affected_count("INSERT INTO t (a) VALUES (?)"));
        assert!(wants_affected_count("  -- hi\nUPDATE t SET a = 1"));
        assert!(wants_affected_count("DELETE FROM t WHERE a = 1"));
        // OUTPUT clauses return rows, so they must be streamed.
        assert!(!wants_affected_count("INSERT INTO t (a) OUTPUT inserted.id VALUES (1)"));
        assert!(!wants_affected_count("UPDATE t SET a = 1 OUTPUT inserted.a"));
        // Multi-statement batches may contain row-returning statements.
        assert!(!wants_affected_count("INSERT INTO t (a) VALUES (1); SELECT * FROM t"));
        // Strings/comments must not confuse the first keyword.
        assert!(!wants_affected_count("SELECT 'INSERT'"));
        assert!(!wants_affected_count("SELECT 1"));
        assert!(!wants_affected_count(""));
    }

    // --- type mapping ---------------------------------------------------

    #[test]
    fn kind_maps_native_names() {
        for (native, expected) in [
            ("INT", DataTypeKind::Int),
            ("BIGINT", DataTypeKind::Int),
            ("SMALLINT", DataTypeKind::Int),
            ("TINYINT", DataTypeKind::Int),
            ("BIT", DataTypeKind::Bool),
            ("DECIMAL(10,2)", DataTypeKind::Decimal),
            ("NUMERIC(38,10)", DataTypeKind::Decimal),
            ("MONEY", DataTypeKind::Decimal),
            ("FLOAT", DataTypeKind::Float),
            ("REAL", DataTypeKind::Float),
            ("NVARCHAR(50)", DataTypeKind::Text),
            ("VARCHAR(max)", DataTypeKind::Text),
            ("CHAR(1)", DataTypeKind::Text),
            ("TEXT", DataTypeKind::Text),
            ("NTEXT", DataTypeKind::Text),
            ("VARBINARY(max)", DataTypeKind::Bytes),
            ("BINARY(16)", DataTypeKind::Bytes),
            ("IMAGE", DataTypeKind::Bytes),
            ("DATE", DataTypeKind::Date),
            ("TIME(7)", DataTypeKind::Time),
            ("DATETIME", DataTypeKind::DateTime),
            ("DATETIME2(3)", DataTypeKind::DateTime),
            ("SMALLDATETIME", DataTypeKind::DateTime),
            ("DATETIMEOFFSET(7)", DataTypeKind::TimestampTz),
            ("UNIQUEIDENTIFIER", DataTypeKind::Uuid),
            ("XML", DataTypeKind::Json),
            ("JSON", DataTypeKind::Json),
            ("SQL_VARIANT", DataTypeKind::Unknown),
        ] {
            assert_eq!(kind_sqlserver(native), expected, "native type {native}");
        }
    }

    #[test]
    fn column_type_names_and_kinds_match() {
        let (metas, signature) = columns_meta(&[
            Column::new("id".into(), ColumnType::Int4),
            Column::new("name".into(), ColumnType::NVarchar),
            Column::new("amount".into(), ColumnType::Decimaln),
            Column::new("created".into(), ColumnType::Datetime2),
        ]);
        assert_eq!(signature, vec!["id", "name", "amount", "created"]);
        assert_eq!(metas[0].native_type, "INT");
        assert_eq!(metas[0].kind, DataTypeKind::Int);
        assert_eq!(metas[1].native_type, "NVARCHAR");
        assert_eq!(metas[1].kind, DataTypeKind::Text);
        assert_eq!(metas[2].kind, DataTypeKind::Decimal);
        assert_eq!(metas[3].kind, DataTypeKind::DateTime);
    }

    #[test]
    fn native_type_text_is_assembled_from_catalog_fields() {
        assert_eq!(
            format_native_type("nvarchar", Some(50), None, None, None),
            "nvarchar(50)"
        );
        assert_eq!(
            format_native_type("varchar", Some(-1), None, None, None),
            "varchar(max)"
        );
        assert_eq!(
            format_native_type("decimal", None, Some(10), Some(2), None),
            "decimal(10,2)"
        );
        assert_eq!(
            format_native_type("datetime2", None, None, None, Some(7)),
            "datetime2(7)"
        );
        assert_eq!(
            format_native_type("uniqueidentifier", None, None, None, None),
            "uniqueidentifier"
        );
    }

    // --- decimal handling ----------------------------------------------

    #[test]
    fn parse_decimal_keeps_scale() {
        let n = parse_decimal_numeric("123.45").unwrap();
        assert_eq!((n.value(), n.scale()), (12345, 2));
        let n = parse_decimal_numeric("-0.50").unwrap();
        assert_eq!((n.value(), n.scale()), (-50, 2));
        let n = parse_decimal_numeric("10").unwrap();
        assert_eq!((n.value(), n.scale()), (10, 0));
        assert!(parse_decimal_numeric("").is_err());
        assert!(parse_decimal_numeric("1e3").is_err());
        assert!(parse_decimal_numeric("abc").is_err());
    }

    #[test]
    fn numeric_renders_as_plain_decimal() {
        assert_eq!(numeric_to_string(&Numeric::new_with_scale(12345, 2)), "123.45");
        assert_eq!(numeric_to_string(&Numeric::new_with_scale(-5, 0)), "-5");
        assert_eq!(numeric_to_string(&Numeric::new_with_scale(5, 3)), "0.005");
        assert_eq!(numeric_to_string(&Numeric::new_with_scale(123, 5)), "0.00123");
        assert_eq!(numeric_to_string(&Numeric::new_with_scale(0, 2)), "0.00");
    }

    // --- parameter binding ----------------------------------------------

    #[test]
    fn binds_scalar_values() {
        assert_eq!(
            value_to_column_data(&Value::Null).unwrap(),
            ColumnData::String(None)
        );
        assert_eq!(
            value_to_column_data(&Value::Bool(true)).unwrap(),
            ColumnData::Bit(Some(true))
        );
        assert_eq!(
            value_to_column_data(&Value::Int(-7)).unwrap(),
            ColumnData::I64(Some(-7))
        );
        assert_eq!(
            value_to_column_data(&Value::Float(1.5)).unwrap(),
            ColumnData::F64(Some(1.5))
        );
        assert_eq!(
            value_to_column_data(&Value::Text("hi".into())).unwrap(),
            ColumnData::String(Some(Cow::Owned("hi".to_string())))
        );
        let uuid = uuid::Uuid::new_v4();
        assert_eq!(
            value_to_column_data(&Value::Uuid(uuid.to_string())).unwrap(),
            ColumnData::Guid(Some(uuid))
        );
    }

    #[test]
    fn binds_uint_above_i64_as_numeric() {
        let big = u64::MAX;
        let data = value_to_column_data(&Value::UInt(big)).unwrap();
        assert_eq!(
            data,
            ColumnData::Numeric(Some(Numeric::new_with_scale(big as i128, 0)))
        );
    }

    #[test]
    fn binds_bytes_and_dates() {
        let encoded = b64_encode(&[1u8, 2, 3]);
        assert_eq!(
            value_to_column_data(&Value::Bytes(encoded)).unwrap(),
            ColumnData::Binary(Some(Cow::Owned(vec![1, 2, 3])))
        );

        let data = value_to_column_data(&Value::Date("2024-02-29".into())).unwrap();
        assert_eq!(
            NaiveDate::from_sql(&data).unwrap(),
            Some(NaiveDate::from_ymd_opt(2024, 2, 29).unwrap())
        );

        let data = value_to_column_data(&Value::Time("12:34:56.789".into())).unwrap();
        assert_eq!(
            NaiveTime::from_sql(&data).unwrap(),
            Some(NaiveTime::from_hms_milli_opt(12, 34, 56, 789).unwrap())
        );

        let data =
            value_to_column_data(&Value::DateTime("2024-01-02 03:04:05.678".into())).unwrap();
        assert!(matches!(data, ColumnData::DateTime2(Some(_))));

        let data =
            value_to_column_data(&Value::DateTime("2024-01-02T03:04:05+07:00".into())).unwrap();
        assert!(matches!(data, ColumnData::DateTimeOffset(Some(_))));

        assert!(value_to_column_data(&Value::DateTime("not a date".into())).is_err());
        assert!(value_to_column_data(&Value::Array(vec![Value::Int(1)])).is_err());
    }

    #[test]
    fn binds_decimal_strings_as_numeric() {
        let data = value_to_column_data(&Value::Decimal("12.34".into())).unwrap();
        assert_eq!(
            data,
            ColumnData::Numeric(Some(Numeric::new_with_scale(1234, 2)))
        );
        assert!(value_to_column_data(&Value::Decimal("oops".into())).is_err());
    }

    // --- identifiers ----------------------------------------------------

    #[test]
    fn quotes_identifiers_with_brackets() {
        assert_eq!(quote_sqlserver_ident("users"), "[users]");
        assert_eq!(quote_sqlserver_ident("we]ird"), "[we]]ird]");
        assert_eq!(db_prefix(Some("my]db")), "[my]]db].");
        assert_eq!(db_prefix(None), "");
    }

    // --- introspection kinds ----------------------------------------------

    #[test]
    fn sys_types_cover_routines_and_triggers() {
        assert_eq!(
            SqlServerDriver::sys_types_for_kind(ObjectKind::Function),
            Some("'FN', 'IF', 'TF'")
        );
        assert_eq!(
            SqlServerDriver::sys_types_for_kind(ObjectKind::Procedure),
            Some("'P'")
        );
        assert_eq!(
            SqlServerDriver::sys_types_for_kind(ObjectKind::Trigger),
            Some("'TR'")
        );
        assert_eq!(
            SqlServerDriver::sys_types_for_kind(ObjectKind::View),
            Some("'V'")
        );
        assert_eq!(SqlServerDriver::sys_types_for_kind(ObjectKind::Table), None);
    }
}
