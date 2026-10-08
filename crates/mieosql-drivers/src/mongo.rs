//! MongoDB / AWS DocumentDB driver (official `mongodb` crate).
//!
//! Query language (v1, parsed by this driver):
//! - `db.<collection>.find(<json filter>).limit(<n>)`
//! - `db.<collection>.find(<json filter>)`
//! - `db.<collection>.aggregate(<json array>)` (optional `.limit(n)` appended as `$limit`)
//! - `db.<collection>.countDocuments(<json filter>)`
//! - `{ "collection": "<name>", "find": { ... }, "limit": 100 }` (JSON form;
//!   `aggregate` / `countDocuments` are accepted as operation keys too)
//!
//! Results stream in batches of 256 documents. The columns of a result set are
//! the union of the top-level fields of the first batch, in first-seen order;
//! later batches are aligned to those columns (missing field => `Null`).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use mongodb::bson::{doc, Bson, Document};
use mongodb::error::{Error as MongoError, ErrorKind as MongoErrorKind};
use mongodb::options::ClientOptions;
use mongodb::results::CollectionType;
use mongodb::{Client, Cursor, IndexModel};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities,
    ColumnDef,
    ColumnMeta,
    ConnectionSpec,
    CreateDbUser,
    DataTypeKind,
    DatabaseRef,
    DbPrincipal,
    DropDbPrincipal,
    Engine,
    ErrorCode,
    ForeignKeyDef,
    GrantRequest,
    GrantTarget,
    IndexDef,
    MieoError,
    MieoResult,
    ObjectKind,
    ObjectRef,
    PlaceholderStyle,
    PrincipalKind,
    QueryEvent,
    QueryOptions,
    QueryStats,
    RevokeRequest,
    RoleGrant,
    SchemaRef,
    Scope,
    ServerInfo,
    SslMode,
    Value,
    DbPrincipalDetail,
    DetailAttribute,
    PrivilegeEntry,
};

use crate::ssh_tunnel::ensure_no_tunnel;
use crate::util::b64_encode;

/// Documents per streamed `Batch` event (contract: first batch = 256 docs).
const BATCH_SIZE: usize = 256;
/// Documents sampled to infer collection fields for `table_columns`.
const SAMPLE_SIZE: usize = 50;
/// Fallback database when the spec has none (the MongoDB shell default).
const DEFAULT_DATABASE: &str = "admin";
/// `appName` sent in the handshake when the profile does not override it.
const DEFAULT_APP_NAME: &str = "MieoSQL";

const QUERY_SYNTAX_HINT: &str = "Expected: db.<collection>.find({...}).limit(n) · \
     db.<collection>.find({...}) · db.<collection>.aggregate([...]) · \
     db.<collection>.countDocuments({...}) · \
     {\"collection\":\"...\",\"find\":{...},\"limit\":n}";

pub struct MongoDriver {
    client: Client,
    spec: ConnectionSpec,
    /// Database targeted when a scope does not name one (`use_database` updates it).
    default_db: String,
}

pub(crate) fn mongo_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::Document,
        placeholder: PlaceholderStyle::Question,
        list_databases: true,
        list_schemas: false,
        schema_is_database: false,
        transactions: false,
        simple_mode: true,
        ddl: false,
        multiple_databases: true,
        user_management: true,
    }
}

impl MongoDriver {
    /// Open a client from `spec`. The connection string is assembled here so
    /// cloud variants (DocumentDB) and the `params` map drive TLS/auth setup.
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        ensure_no_tunnel(spec, "MongoDB")?;
        let uri = build_uri(spec);
        let mut options = ClientOptions::parse(uri.as_str()).await.map_err(|e| {
            map_mongo_err(e).with_hint(
                "check host/port and params: auth_source, replica_set, tls_insecure, direct, app_name",
            )
        })?;

        let default_db = spec
            .database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(DEFAULT_DATABASE)
            .to_string();
        // The URI path stays empty so credentials keep authenticating against
        // `authSource` (default: admin); the default database is set here.
        options.default_database = Some(default_db.clone());

        let client = Client::with_options(options).map_err(map_mongo_err)?;

        // Fail fast on unreachable servers / bad credentials: the driver
        // connects lazily, so without this the error would surface on the
        // first query instead of when the session is opened.
        if let Err(e) = client
            .database(&default_db)
            .run_command(doc! { "ping": 1 })
            .await
        {
            let err = map_mongo_err(e);
            let documentdb_hint = is_documentdb(spec)
                && !param_truthy(&spec.params, "tls_insecure")
                && spec
                    .params
                    .get("tls_ca_file")
                    .is_none_or(|v| v.trim().is_empty());
            return Err(if documentdb_hint {
                err.with_hint(
                    "DocumentDB: set params tls_insecure=true or \
                     tls_ca_file=/path/to/global-bundle.pem when the server certificate is not \
                     publicly trusted",
                )
            } else {
                err
            });
        }

        Ok(Self {
            client,
            spec: spec.clone(),
            default_db,
        })
    }

    pub fn spec(&self) -> &ConnectionSpec {
        &self.spec
    }

    pub fn in_transaction(&self) -> bool {
        false
    }

    pub async fn ping(&mut self) -> MieoResult<()> {
        self.client
            .database(&self.default_db)
            .run_command(doc! { "ping": 1 })
            .await
            .map_err(map_mongo_err)?;
        Ok(())
    }

    pub async fn server_info(&mut self) -> MieoResult<ServerInfo> {
        let info = self
            .client
            .database(DEFAULT_DATABASE)
            .run_command(doc! { "buildInfo": 1 })
            .await
            .map_err(map_mongo_err)?;

        let version = info
            .get_str("version")
            .map(str::to_string)
            .unwrap_or_else(|_| "unknown".to_string());

        let mut comments: Vec<String> = Vec::new();
        if let Ok(git) = info.get_str("gitVersion") {
            if !git.is_empty() {
                comments.push(format!("git {git}"));
            }
        }
        if let Ok(docdb) = info.get_str("docdbVersion") {
            if !docdb.is_empty() {
                comments.push(format!("docdb {docdb}"));
            }
        }

        Ok(ServerInfo {
            engine: Engine::MongoDb,
            version,
            version_comment: (!comments.is_empty()).then(|| comments.join(", ")),
            current_database: Some(self.default_db.clone()),
            current_user: self.spec.username.clone(),
        })
    }

    pub async fn list_databases(&mut self) -> MieoResult<Vec<DatabaseRef>> {
        let databases = self.client.list_databases().await.map_err(map_mongo_err)?;
        Ok(databases
            .into_iter()
            .map(|db| DatabaseRef {
                is_system: matches!(db.name.as_str(), "admin" | "local" | "config"),
                size_bytes: (db.size_on_disk > 0).then_some(db.size_on_disk),
                name: db.name,
                comment: None,
                charset: None,
                collation: None,
            })
            .collect())
    }

    /// MongoDB has no schemas; the explorer shows databases → collections.
    pub async fn list_schemas(&mut self, database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        let _ = database;
        Ok(Vec::new())
    }

    pub async fn list_objects(
        &mut self,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        match kind {
            Some(
                ObjectKind::Function
                | ObjectKind::Procedure
                | ObjectKind::Trigger
                | ObjectKind::Sequence
                | ObjectKind::MaterializedView,
            ) => return Ok(Vec::new()),
            _ => {}
        }

        let database = self.database_name(scope);
        let mut cursor = Box::pin(
            self.client
                .database(&database)
                .list_collections()
                .await
                .map_err(map_mongo_err)?,
        );

        let mut objects = Vec::new();
        while let Some(spec) = cursor
            .next()
            .await
            .transpose()
            .map_err(map_mongo_err)?
        {
            let object_kind = match spec.collection_type {
                CollectionType::View => ObjectKind::View,
                _ => ObjectKind::Table,
            };
            if let Some(wanted) = kind {
                if wanted != object_kind {
                    continue;
                }
            }
            if let Some(pattern) = pattern.map(str::trim).filter(|p| !p.is_empty()) {
                if !spec
                    .name
                    .to_lowercase()
                    .contains(&pattern.to_lowercase())
                {
                    continue;
                }
            }
            objects.push(ObjectRef {
                name: spec.name,
                kind: object_kind,
                database: Some(database.clone()),
                schema: None,
                comment: None,
                rows_estimate: None,
                size_bytes: None,
                updated_at: None,
            });
        }
        objects.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(objects)
    }

    /// Field inference over a 50-document sample (MongoDB is schemaless).
    pub async fn table_columns(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ColumnDef>> {
        let database = self.database_name(scope);
        let collection = self
            .client
            .database(&database)
            .collection::<Document>(table);

        let mut cursor = Box::pin(
            collection
                .find(doc! {})
                .limit(SAMPLE_SIZE as i64)
                .await
                .map_err(map_mongo_err)?,
        );

        let mut sample: Vec<Document> = Vec::with_capacity(SAMPLE_SIZE);
        while sample.len() < SAMPLE_SIZE {
            match cursor
                .next()
                .await
                .transpose()
                .map_err(map_mongo_err)?
            {
                Some(document) => sample.push(document),
                None => break,
            }
        }

        let sample_len = sample.len();
        let fields = infer_fields(&sample);
        Ok(fields
            .iter()
            .enumerate()
            .map(|(ordinal, field)| ColumnDef {
                name: field.name.clone(),
                ordinal: ordinal as u32,
                native_type: field.native_type.to_string(),
                data_type: field.kind.clone(),
                nullable: field.nullable(sample_len),
                default_value: None,
                is_primary_key: field.name == "_id",
                is_auto_increment: field.name == "_id" && field.kind == DataTypeKind::Uuid,
                is_generated: false,
                comment: None,
                char_max_length: None,
                numeric_precision: None,
                numeric_scale: None,
                collation: None,
            })
            .collect())
    }

    pub async fn table_indexes(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let database = self.database_name(scope);
        let collection = self
            .client
            .database(&database)
            .collection::<Document>(table);

        let mut cursor = Box::pin(collection.list_indexes().await.map_err(map_mongo_err)?);
        let mut indexes = Vec::new();
        while let Some(model) = cursor
            .next()
            .await
            .transpose()
            .map_err(map_mongo_err)?
        {
            indexes.push(index_def_from_model(&model));
        }
        Ok(indexes)
    }

    pub async fn foreign_keys(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        let _ = (scope, table);
        Ok(Vec::new())
    }

    pub async fn object_ddl(
        &mut self,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let _ = (scope, kind, name);
        Ok(None)
    }

    // ------------------------------------------------------------------
    // Users / roles / grants (also drives the authorization UI).
    // MongoDB users and custom roles live per-database; listings merge the
    // `admin` database with the session database. `host` carries the auth db.
    // ------------------------------------------------------------------

    fn user_db(&self, database: Option<&str>) -> String {
        database
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(&self.default_db)
            .to_string()
    }

    fn user_dbs(&self) -> Vec<String> {
        let mut dbs = vec!["admin".to_string()];
        if self.default_db != "admin" {
            dbs.push(self.default_db.clone());
        }
        dbs
    }

    fn doc_users(doc: &Document) -> Vec<(String, String, Vec<(String, String)>)> {
        let mut out = Vec::new();
        let Ok(users) = doc.get_array("users") else {
            return out;
        };
        for u in users {
            let Bson::Document(user) = u else {
                continue;
            };
            let (Ok(name), Ok(db)) = (user.get_str("user"), user.get_str("db")) else {
                continue;
            };
            let mut roles = Vec::new();
            if let Ok(rlist) = user.get_array("roles") {
                for r in rlist {
                    if let Bson::Document(rd) = r
                        && let (Ok(role), Ok(rdb)) = (rd.get_str("role"), rd.get_str("db"))
                    {
                        roles.push((role.to_string(), rdb.to_string()));
                    }
                }
            }
            out.push((name.to_string(), db.to_string(), roles));
        }
        out
    }

    pub async fn list_db_users(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        let mut out = Vec::new();
        for db in self.user_dbs() {
            let doc = self
                .client
                .database(&db)
                .run_command(doc! { "usersInfo": 1, "showCredentials": false })
                .await
                .map_err(map_mongo_err)?;
            for (name, udb, roles) in Self::doc_users(&doc) {
                out.push(DbPrincipal {
                    name,
                    host: Some(udb.clone()),
                    kind: PrincipalKind::User,
                    is_super: roles.iter().any(|(r, _)| {
                        matches!(r.as_str(), "root" | "userAdminAnyDatabase" | "dbOwner")
                    }),
                    can_login: true,
                    comment: if roles.is_empty() {
                        None
                    } else {
                        Some(
                            roles
                                .iter()
                                .map(|(r, d)| format!("{r}@{d}"))
                                .collect::<Vec<_>>()
                                .join(", "),
                        )
                    },
                });
            }
        }
        Ok(out)
    }

    pub async fn list_db_roles(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        let mut out = Vec::new();
        for db in self.user_dbs() {
            let doc = self
                .client
                .database(&db)
                .run_command(doc! { "rolesInfo": 1, "showPrivileges": false })
                .await
                .map_err(map_mongo_err)?;
            let Ok(roles) = doc.get_array("roles") else {
                continue;
            };
            for r in roles {
                let Bson::Document(rd) = r else {
                    continue;
                };
                let (Ok(role), Ok(rdb)) = (rd.get_str("role"), rd.get_str("db")) else {
                    continue;
                };
                out.push(DbPrincipal {
                    name: role.to_string(),
                    host: Some(rdb.to_string()),
                    kind: PrincipalKind::Role,
                    is_super: false,
                    can_login: false,
                    comment: rd
                        .get_bool("isBuiltin")
                        .ok()
                        .filter(|b| *b)
                        .map(|_| "builtin".to_string()),
                });
            }
        }
        Ok(out)
    }

    pub async fn db_role_members(&mut self) -> MieoResult<Vec<RoleGrant>> {
        let mut out = Vec::new();
        for db in self.user_dbs() {
            let doc = self
                .client
                .database(&db)
                .run_command(doc! { "usersInfo": 1, "showCredentials": false })
                .await
                .map_err(map_mongo_err)?;
            for (user, udb, roles) in Self::doc_users(&doc) {
                for (role, rdb) in roles {
                    out.push(RoleGrant {
                        role,
                        role_host: Some(rdb),
                        member: user.clone(),
                        member_host: Some(udb.clone()),
                        admin_option: false,
                    });
                }
            }
        }
        Ok(out)
    }

    pub async fn db_user_grants(
        &mut self,
        name: &str,
        host: Option<&str>,
    ) -> MieoResult<Vec<String>> {
        let db = self.user_db(host);
        let doc = self
            .client
            .database(&db)
            .run_command(doc! {
                "usersInfo": { "user": name, "db": db },
                "showPrivileges": true,
            })
            .await
            .map_err(map_mongo_err)?;
        let mut lines = Vec::new();
        for (user, udb, roles) in Self::doc_users(&doc) {
            for (role, rdb) in roles {
                lines.push(format!("ROLE {role}@{rdb} ON {user}@{udb}"));
            }
        }
        if let Ok(users) = doc.get_array("users") {
            for u in users {
                let Bson::Document(user) = u else {
                    continue;
                };
                let Ok(privs) = user.get_array("inheritedPrivileges") else {
                    continue;
                };
                for p in privs {
                    let Bson::Document(pd) = p else {
                        continue;
                    };
                    let actions = pd
                        .get_array("actions")
                        .ok()
                        .map(|a| {
                            a.iter().filter_map(|b| b.as_str().map(str::to_string)).collect::<Vec<_>>().join(",")
                        })
                        .unwrap_or_default();
                    let resource = pd
                        .get_document("resource")
                        .ok()
                        .map(|r| {
                            if let Ok(c) = r.get_str("collection") {
                                format!(
                                    "{}.{}",
                                    r.get_str("db").unwrap_or("?"),
                                    if c.is_empty() { "*" } else { c }
                                )
                            } else {
                                r.get_str("db").unwrap_or("?").to_string()
                            }
                        })
                        .unwrap_or_else(|| "?".to_string());
                    lines.push(format!("PRIV {actions} ON {resource}"));
                }
            }
        }
        Ok(lines)
    }

    pub async fn db_principal_detail(
        &mut self,
        name: &str,
        host: Option<&str>,
    ) -> MieoResult<DbPrincipalDetail> {
        let db = self.user_db(host);
        let doc = self
            .client
            .database(&db)
            .run_command(doc! {
                "usersInfo": { "user": name, "db": db },
                "showPrivileges": true,
            })
            .await
            .map_err(map_mongo_err)?;
        let found = Self::doc_users(&doc).into_iter().next();
        let Some((user, udb, roles)) = found else {
            return Err(MieoError::new(ErrorCode::NotFound, format!("user `{name}` not found")));
        };
        let principal = DbPrincipal {
            name: user,
            host: Some(udb.clone()),
            kind: PrincipalKind::User,
            is_super: roles.iter().any(|(r, _)| {
                matches!(r.as_str(), "root" | "userAdminAnyDatabase" | "dbOwner")
            }),
            can_login: true,
            comment: if roles.is_empty() {
                None
            } else {
                Some(
                    roles
                        .iter()
                        .map(|(r, d)| format!("{r}@{d}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            },
        };
        let attributes = vec![DetailAttribute {
            key: "Authentication DB".into(),
            value: udb.clone(),
        }];
        let member_of = roles
            .into_iter()
            .map(|(role, rdb)| RoleGrant {
                role,
                role_host: Some(rdb),
                member: name.to_string(),
                member_host: Some(udb.clone()),
                admin_option: false,
            })
            .collect();
        // Inherited (effective) privileges grouped per resource.
        let mut object_privileges: Vec<PrivilegeEntry> = Vec::new();
        if let Ok(users) = doc.get_array("users") {
            for u in users {
                let Bson::Document(user) = u else {
                    continue;
                };
                let Ok(privs) = user.get_array("inheritedPrivileges") else {
                    continue;
                };
                let mut grouped: std::collections::BTreeMap<(String, String), Vec<String>> =
                    std::collections::BTreeMap::new();
                for p in privs {
                    let Bson::Document(pd) = p else {
                        continue;
                    };
                    let actions: Vec<String> = pd
                        .get_array("actions")
                        .ok()
                        .map(|a| {
                            a.iter()
                                .filter_map(|b| b.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let (rdb, coll) = pd
                        .get_document("resource")
                        .ok()
                        .map(|r| {
                            (
                                r.get_str("db").unwrap_or("?").to_string(),
                                r.get_str("collection").ok().map(str::to_string),
                            )
                        })
                        .unwrap_or_else(|| ("?".to_string(), None));
                    grouped.entry((rdb, coll.unwrap_or_default())).or_default().extend(actions);
                }
                for ((rdb, coll), mut actions) in grouped {
                    actions.sort();
                    actions.dedup();
                    object_privileges.push(PrivilegeEntry {
                        privileges: actions,
                        database: Some(rdb),
                        schema: None,
                        table: if coll.is_empty() { None } else { Some(coll) },
                        grantable: false,
                    });
                }
            }
        }
        Ok(DbPrincipalDetail {
            principal,
            attributes,
            global_privileges: Vec::new(),
            object_privileges,
            member_of,
            members: Vec::new(),
        })
    }

    pub async fn create_db_user(&mut self, req: &CreateDbUser) -> MieoResult<()> {
        let db = self.user_db(req.database.as_deref());
        let mut roles = Vec::new();
        if req.superuser {
            roles.push(doc! { "role": "dbOwner", "db": db.clone() });
        }
        let mut cmd = doc! { "createUser": req.name.clone(), "roles": Bson::Array(roles.into_iter().map(Bson::Document).collect()) };
        if let Some(pw) = req.password.as_deref().filter(|p| !p.is_empty()) {
            cmd.insert("pwd", pw);
        }
        self.client
            .database(&db)
            .run_command(cmd)
            .await
            .map_err(map_mongo_err)?;
        Ok(())
    }

    pub async fn drop_db_principal(&mut self, req: &DropDbPrincipal) -> MieoResult<()> {
        let db = self.user_db(req.host.as_deref());
        let cmd = match req.kind {
            PrincipalKind::User => doc! { "dropUser": req.name.clone() },
            PrincipalKind::Role => doc! { "dropRole": req.name.clone() },
        };
        self.client
            .database(&db)
            .run_command(cmd)
            .await
            .map_err(map_mongo_err)?;
        Ok(())
    }

    pub async fn grant(&mut self, req: &GrantRequest) -> MieoResult<()> {
        let db = self.user_db(req.grantee_host.as_deref());
        match &req.target {
            GrantTarget::Role { role, host } => {
                let rdb = host.as_deref().filter(|h| !h.is_empty()).unwrap_or(&db);
                self.client
                    .database(&db)
                    .run_command(doc! {
                        "grantRolesToUser": req.grantee.clone(),
                        "roles": [{ "role": role, "db": rdb }],
                    })
                    .await
                    .map_err(map_mongo_err)?;
                Ok(())
            }
            GrantTarget::Privileges { .. } => Err(MieoError::unsupported(
                "MongoDB object privileges require custom roles: grant a role to the user instead",
            )
            .with_hint("create the role with privileges in mongosh, then grant the role here")),
        }
    }

    pub async fn revoke(&mut self, req: &RevokeRequest) -> MieoResult<()> {
        let db = self.user_db(req.grantee_host.as_deref());
        match &req.target {
            GrantTarget::Role { role, host } => {
                let rdb = host.as_deref().filter(|h| !h.is_empty()).unwrap_or(&db);
                self.client
                    .database(&db)
                    .run_command(doc! {
                        "revokeRolesFromUser": req.grantee.clone(),
                        "roles": [{ "role": role, "db": rdb }],
                    })
                    .await
                    .map_err(map_mongo_err)?;
                Ok(())
            }
            GrantTarget::Privileges { .. } => Err(MieoError::unsupported(
                "MongoDB object privileges require custom roles: revoke the role instead",
            )),
        }
    }

    pub async fn set_user_password(
        &mut self,
        name: &str,
        host: Option<&str>,
        password: &str,
    ) -> MieoResult<()> {
        let db = self.user_db(host);
        self.client
            .database(&db)
            .run_command(doc! { "updateUser": name.to_string(), "pwd": password.to_string() })
            .await
            .map_err(map_mongo_err)?;
        Ok(())
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        let database = database.trim();
        if database.is_empty() {
            return Err(MieoError::new(
                ErrorCode::Data,
                "database name must not be empty",
            ));
        }
        self.default_db = database.to_string();
        self.spec.database = Some(database.to_string());
        Ok(())
    }

    pub async fn begin(&mut self) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "transactions are not supported by the MongoDB driver",
        ))
    }

    pub async fn commit(&mut self) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "transactions are not supported by the MongoDB driver",
        ))
    }

    pub async fn rollback(&mut self) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "transactions are not supported by the MongoDB driver",
        ))
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
        if !params.is_empty() {
            return Err(MieoError::new(
                ErrorCode::Data,
                "MongoDB queries do not support bound parameters",
            ));
        }

        let command = parse_command(&sql)?;
        let start = Instant::now();
        let deadline = options
            .timeout_ms
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        let cap = options.max_rows.unwrap_or(u64::MAX);

        let database = self.client.database(&self.default_db);
        let mut stats = QueryStats::default();

        match command {
            MongoCommand::CountDocuments { collection, filter } => {
                let coll = database.collection::<Document>(&collection);
                let count = with_cancel_and_timeout(
                    coll.count_documents(filter).into_future(),
                    deadline,
                    cancel,
                )
                .await?
                .map_err(map_mongo_err)?;

                let value = i64::try_from(count)
                    .map(Value::Int)
                    .unwrap_or(Value::UInt(count));
                let columns = vec![ColumnMeta {
                    name: "count".to_string(),
                    ordinal: 0,
                    native_type: "long".to_string(),
                    kind: DataTypeKind::Int,
                    nullable: Some(false),
                }];
                send_event(
                    sink,
                    QueryEvent::Columns {
                        query_id: query_id.to_string(),
                        columns,
                    },
                )
                .await?;
                // `max_rows = 0` behaves like the SQL drivers: no row streamed.
                if cap == 0 {
                    stats.truncated = true;
                } else {
                    stats.rows = 1;
                    send_event(
                        sink,
                        QueryEvent::Batch {
                            query_id: query_id.to_string(),
                            rows: vec![vec![value]],
                            batch_index: 1,
                        },
                    )
                    .await?;
                }
            }
            MongoCommand::Find {
                collection,
                filter,
                limit,
            } => {
                let coll = database.collection::<Document>(&collection);
                let mut action = coll.find(filter).batch_size(BATCH_SIZE as u32);
                if let Some(limit) = limit {
                    action = action.limit(clamp_limit(limit));
                }
                let cursor = with_cancel_and_timeout(action.into_future(), deadline, cancel)
                    .await?
                    .map_err(map_mongo_err)?;
                stream_cursor(query_id, cursor, cap, deadline, cancel, sink, &mut stats).await?;
            }
            MongoCommand::Aggregate {
                collection,
                pipeline,
                limit,
            } => {
                let mut pipeline = pipeline;
                if let Some(limit) = limit {
                    pipeline.push(doc! { "$limit": clamp_limit(limit) });
                }
                let coll = database.collection::<Document>(&collection);
                let action = coll.aggregate(pipeline).batch_size(BATCH_SIZE as u32);
                let cursor = with_cancel_and_timeout(action.into_future(), deadline, cancel)
                    .await?
                    .map_err(map_mongo_err)?;
                stream_cursor(query_id, cursor, cap, deadline, cancel, sink, &mut stats).await?;
            }
        }

        stats.elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(stats)
    }

    /// `db.<collection>.find({}).limit(n)` preview.
    pub fn preview_statement(&self, scope: &Scope, table: &str, limit: u64) -> MieoResult<String> {
        let _ = scope;
        Ok(format!("db.{table}.find({{}}).limit({limit})"))
    }

    pub fn engine(&self) -> Engine {
        Engine::MongoDb
    }

    /// Database named by `scope`, falling back to the driver default.
    fn database_name(&self, scope: &Scope) -> String {
        scope
            .database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(&self.default_db)
            .to_string()
    }
}

// ---------------------------------------------------------------------------
// Query language v1
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum MongoCommand {
    Find {
        collection: String,
        filter: Document,
        limit: Option<u64>,
    },
    Aggregate {
        collection: String,
        pipeline: Vec<Document>,
        limit: Option<u64>,
    },
    CountDocuments {
        collection: String,
        filter: Document,
    },
}

fn syntax_err(message: impl Into<String>) -> MieoError {
    MieoError::new(ErrorCode::Syntax, message).with_hint(QUERY_SYNTAX_HINT)
}

fn parse_command(input: &str) -> MieoResult<MongoCommand> {
    let text = input.trim();
    if text.is_empty() {
        return Err(syntax_err("empty query"));
    }
    if text.starts_with('{') {
        parse_json_command(text)
    } else {
        parse_shell_command(text)
    }
}

fn parse_shell_command(text: &str) -> MieoResult<MongoCommand> {
    let rest = text
        .strip_prefix("db.")
        .ok_or_else(|| syntax_err("query must start with `db.`"))?;

    let (collection, rest) = rest
        .split_once('.')
        .ok_or_else(|| syntax_err("expected `db.<collection>.<method>(...)`"))?;
    let collection = collection.trim();
    if collection.is_empty() {
        return Err(syntax_err("collection name is empty"));
    }
    if collection.chars().any(char::is_whitespace) {
        return Err(syntax_err("collection name must not contain whitespace"));
    }

    let rest = rest.trim_start();
    let open = rest
        .find('(')
        .ok_or_else(|| syntax_err("expected `(` after the method name"))?;
    let method = rest[..open].trim();
    let close = find_matching_paren(rest, open)?;
    let arguments = rest[open + 1..close].trim();
    let tail = rest[close + 1..].trim();

    match method.to_ascii_lowercase().as_str() {
        "find" => {
            let filter = json_object(arguments)?;
            let limit = parse_tail_limit(tail)?;
            Ok(MongoCommand::Find {
                collection: collection.to_string(),
                filter,
                limit,
            })
        }
        "aggregate" => {
            let stages = json_array(arguments)?;
            let mut pipeline = Vec::with_capacity(stages.len());
            for stage in &stages {
                pipeline.push(json_value_to_document(stage)?);
            }
            let limit = parse_tail_limit(tail)?;
            Ok(MongoCommand::Aggregate {
                collection: collection.to_string(),
                pipeline,
                limit,
            })
        }
        "countdocuments" => {
            if !tail.is_empty() {
                return Err(syntax_err("countDocuments does not take trailing methods"));
            }
            let filter = json_object(arguments)?;
            Ok(MongoCommand::CountDocuments {
                collection: collection.to_string(),
                filter,
            })
        }
        other => Err(syntax_err(format!(
            "unsupported method `{other}` — expected find, aggregate or countDocuments"
        ))),
    }
}

fn parse_json_command(text: &str) -> MieoResult<MongoCommand> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| syntax_err(format!("invalid JSON query: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| syntax_err("JSON query must be an object"))?;

    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "collection" | "find" | "aggregate" | "countDocuments" | "count_documents" | "limit"
        ) {
            return Err(syntax_err(format!("unknown JSON query field `{key}`")));
        }
    }

    let collection = object
        .get("collection")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| syntax_err("JSON query requires a non-empty `collection` string"))?
        .to_string();

    let limit = match object.get("limit") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(number)) => Some(
            number
                .as_u64()
                .ok_or_else(|| syntax_err("`limit` must be a non-negative integer"))?,
        ),
        Some(_) => return Err(syntax_err("`limit` must be a non-negative integer")),
    };

    let mut operations: Vec<&str> = Vec::new();
    if object.contains_key("find") {
        operations.push("find");
    }
    if object.contains_key("aggregate") {
        operations.push("aggregate");
    }
    if object.contains_key("countDocuments") || object.contains_key("count_documents") {
        operations.push("countDocuments");
    }
    if operations.len() != 1 {
        return Err(syntax_err(
            "JSON query must contain exactly one of `find`, `aggregate`, `countDocuments`",
        ));
    }

    match operations[0] {
        "find" => Ok(MongoCommand::Find {
            collection,
            filter: json_field_to_document(object.get("find"), "find")?,
            limit,
        }),
        "aggregate" => {
            let stages = object
                .get("aggregate")
                .and_then(|v| v.as_array())
                .ok_or_else(|| syntax_err("`aggregate` must be an array of stage objects"))?;
            let mut pipeline = Vec::with_capacity(stages.len());
            for stage in stages {
                pipeline.push(json_value_to_document(stage)?);
            }
            Ok(MongoCommand::Aggregate {
                collection,
                pipeline,
                limit,
            })
        }
        _ => {
            if limit.is_some() {
                return Err(syntax_err("`limit` is not supported with `countDocuments`"));
            }
            let filter = json_field_to_document(
                object
                    .get("countDocuments")
                    .or_else(|| object.get("count_documents")),
                "countDocuments",
            )?;
            Ok(MongoCommand::CountDocuments { collection, filter })
        }
    }
}

/// Byte index of the `)` matching the `(` at `open`, skipping string literals.
fn find_matching_paren(text: &str, open: usize) -> MieoResult<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string: Option<u8> = None;
    let mut escaped = false;

    for (offset, &byte) in bytes[open..].iter().enumerate() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote {
                in_string = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => in_string = Some(byte),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(open + offset);
                }
            }
            _ => {}
        }
    }
    Err(syntax_err("unbalanced parentheses"))
}

/// Parse an optional trailing `.limit(n)`.
fn parse_tail_limit(tail: &str) -> MieoResult<Option<u64>> {
    let tail = tail.trim();
    if tail.is_empty() {
        return Ok(None);
    }
    let tail = tail
        .strip_prefix('.')
        .ok_or_else(|| syntax_err("unexpected text after the query"))?
        .trim_start();
    let open = tail
        .find('(')
        .ok_or_else(|| syntax_err("expected `.limit(n)`"))?;
    let method = tail[..open].trim();
    if !method.eq_ignore_ascii_case("limit") {
        return Err(syntax_err(format!("unsupported method `.{method}`")));
    }
    let close = find_matching_paren(tail, open)?;
    let argument = tail[open + 1..close].trim();
    if argument.is_empty() {
        return Err(syntax_err("`.limit(n)` requires a value"));
    }
    if !argument.bytes().all(|b| b.is_ascii_digit()) {
        return Err(syntax_err("`limit` must be a non-negative integer"));
    }
    let limit: u64 = argument
        .parse()
        .map_err(|_| syntax_err("`limit` is too large"))?;
    if !tail[close + 1..].trim().is_empty() {
        return Err(syntax_err("unexpected text after `.limit(n)`"));
    }
    Ok(Some(limit))
}

fn json_object(text: &str) -> MieoResult<Document> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| syntax_err(format!("invalid JSON argument: {e}")))?;
    if !value.is_object() {
        return Err(syntax_err("expected a JSON object argument"));
    }
    json_value_to_document(&value)
}

fn json_array(text: &str) -> MieoResult<Vec<serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| syntax_err(format!("invalid JSON argument: {e}")))?;
    match value {
        serde_json::Value::Array(items) => Ok(items),
        _ => Err(syntax_err("expected a JSON array argument")),
    }
}

fn json_field_to_document(value: Option<&serde_json::Value>, field: &str) -> MieoResult<Document> {
    let value = value.ok_or_else(|| syntax_err(format!("missing `{field}`")))?;
    json_value_to_document(value)
}

fn json_value_to_document(value: &serde_json::Value) -> MieoResult<Document> {
    if !value.is_object() {
        return Err(syntax_err(
            "filters and aggregation stages must be JSON objects",
        ));
    }
    mongodb::bson::to_document(value)
        .map_err(|e| syntax_err(format!("cannot convert JSON to BSON: {e}")))
}

// ---------------------------------------------------------------------------
// BSON → Value / column inference
// ---------------------------------------------------------------------------

/// One top-level field inferred from a document sample.
#[derive(Debug, Clone)]
struct InferredField {
    name: String,
    /// BSON type name (as reported by the wire protocol).
    native_type: &'static str,
    kind: DataTypeKind,
    /// Number of sampled documents where the field is present.
    present: usize,
    /// The field was explicitly `null` in at least one sampled document.
    saw_null: bool,
}

impl InferredField {
    fn nullable(&self, sample_len: usize) -> bool {
        self.saw_null || self.present < sample_len
    }

    fn to_column_meta(&self, ordinal: usize, sample_len: usize) -> ColumnMeta {
        ColumnMeta {
            name: self.name.clone(),
            ordinal: ordinal as u32,
            native_type: self.native_type.to_string(),
            kind: self.kind.clone(),
            nullable: Some(self.nullable(sample_len)),
        }
    }
}

/// Union of top-level fields, in first-seen order. When the same field carries
/// different BSON types across the sample, the widest kind wins (see
/// [`kind_rank`]); `null` only widens nullability.
fn infer_fields(documents: &[Document]) -> Vec<InferredField> {
    let mut fields: Vec<InferredField> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for document in documents {
        for (name, value) in document {
            if let Some(&position) = index.get(name) {
                let field = &mut fields[position];
                field.present += 1;
                if matches!(value, Bson::Null) {
                    field.saw_null = true;
                } else {
                    let kind = bson_kind(value);
                    if kind_rank(&kind) > kind_rank(&field.kind) {
                        field.kind = kind;
                        field.native_type = bson_native_name(value);
                    }
                }
            } else {
                index.insert(name.clone(), fields.len());
                fields.push(InferredField {
                    name: name.clone(),
                    native_type: bson_native_name(value),
                    kind: bson_kind(value),
                    present: 1,
                    saw_null: matches!(value, Bson::Null),
                });
            }
        }
    }
    fields
}

/// Specificity order used when a field has mixed types across the sample.
fn kind_rank(kind: &DataTypeKind) -> u8 {
    match kind {
        DataTypeKind::Unknown => 0,
        DataTypeKind::Bool => 1,
        DataTypeKind::Int | DataTypeKind::UInt => 2,
        DataTypeKind::Float => 3,
        DataTypeKind::Decimal => 4,
        DataTypeKind::Date
        | DataTypeKind::Time
        | DataTypeKind::DateTime
        | DataTypeKind::TimestampTz => 5,
        DataTypeKind::Uuid => 6,
        DataTypeKind::Bytes => 7,
        DataTypeKind::Text | DataTypeKind::Enum | DataTypeKind::Set => 8,
        DataTypeKind::Json
        | DataTypeKind::Array(_)
        | DataTypeKind::Geometry
        | DataTypeKind::Bit
        | DataTypeKind::Interval => 9,
    }
}

fn bson_kind(value: &Bson) -> DataTypeKind {
    match value {
        Bson::Double(_) => DataTypeKind::Float,
        Bson::String(_) => DataTypeKind::Text,
        Bson::Array(_) | Bson::Document(_) => DataTypeKind::Json,
        Bson::Boolean(_) => DataTypeKind::Bool,
        Bson::Null | Bson::Undefined | Bson::MinKey | Bson::MaxKey => DataTypeKind::Unknown,
        Bson::Int32(_) | Bson::Int64(_) => DataTypeKind::Int,
        Bson::Timestamp(_) => DataTypeKind::Json,
        Bson::Binary(_) => DataTypeKind::Bytes,
        Bson::ObjectId(_) => DataTypeKind::Uuid,
        Bson::DateTime(_) => DataTypeKind::DateTime,
        Bson::Decimal128(_) => DataTypeKind::Decimal,
        Bson::RegularExpression(_)
        | Bson::JavaScriptCode(_)
        | Bson::JavaScriptCodeWithScope(_)
        | Bson::Symbol(_)
        | Bson::DbPointer(_) => DataTypeKind::Json,
    }
}

fn bson_native_name(value: &Bson) -> &'static str {
    match value {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Array(_) => "array",
        Bson::Document(_) => "object",
        Bson::Boolean(_) => "bool",
        Bson::Null => "null",
        Bson::RegularExpression(_) => "regex",
        Bson::JavaScriptCode(_) => "javascript",
        Bson::JavaScriptCodeWithScope(_) => "javascriptWithScope",
        Bson::Int32(_) => "int",
        Bson::Int64(_) => "long",
        Bson::Timestamp(_) => "timestamp",
        Bson::Binary(_) => "binData",
        Bson::ObjectId(_) => "objectId",
        Bson::DateTime(_) => "date",
        Bson::Symbol(_) => "symbol",
        Bson::Decimal128(_) => "decimal",
        Bson::Undefined => "undefined",
        Bson::MaxKey => "maxKey",
        Bson::MinKey => "minKey",
        Bson::DbPointer(_) => "dbPointer",
    }
}

fn bson_to_value(value: &Bson) -> Value {
    match value {
        Bson::Double(v) => Value::Float(*v),
        Bson::String(v) => Value::Text(v.clone()),
        Bson::Array(_)
        | Bson::Document(_)
        | Bson::RegularExpression(_)
        | Bson::JavaScriptCode(_)
        | Bson::JavaScriptCodeWithScope(_)
        | Bson::Symbol(_)
        | Bson::Timestamp(_)
        | Bson::Undefined
        | Bson::MinKey
        | Bson::MaxKey
        | Bson::DbPointer(_) => Value::Json(value.clone().into_relaxed_extjson()),
        Bson::Boolean(v) => Value::Bool(*v),
        Bson::Null => Value::Null,
        Bson::Int32(v) => Value::Int(i64::from(*v)),
        Bson::Int64(v) => Value::Int(*v),
        Bson::Binary(v) => Value::Bytes(b64_encode(&v.bytes)),
        Bson::ObjectId(v) => Value::Uuid(v.to_hex()),
        Bson::DateTime(v) => match v.try_to_rfc3339_string() {
            Ok(text) => Value::DateTime(text),
            // Out-of-range dates (bson stores i64 millis) keep their extended
            // JSON form instead of panicking.
            Err(_) => Value::Json(value.clone().into_relaxed_extjson()),
        },
        Bson::Decimal128(v) => Value::Decimal(v.to_string()),
    }
}

fn document_to_row(document: &Document, columns: &[InferredField]) -> Vec<Value> {
    columns
        .iter()
        .map(|column| {
            document
                .get(&column.name)
                .map(bson_to_value)
                .unwrap_or(Value::Null)
        })
        .collect()
}

fn index_def_from_model(model: &IndexModel) -> IndexDef {
    let name = model
        .options
        .as_ref()
        .and_then(|options| options.name.clone())
        .unwrap_or_else(|| index_name_from_keys(&model.keys));
    IndexDef {
        is_primary: name == "_id_",
        name,
        columns: model.keys.keys().cloned().collect(),
        is_unique: model
            .options
            .as_ref()
            .and_then(|options| options.unique)
            .unwrap_or(false),
        index_type: None,
        comment: None,
    }
}

/// Rebuild the conventional index name (`a_1_b_-1`) when the server omitted it.
fn index_name_from_keys(keys: &Document) -> String {
    keys.iter()
        .map(|(field, value)| format!("{field}_{}", index_key_suffix(value)))
        .collect::<Vec<_>>()
        .join("_")
}

fn index_key_suffix(value: &Bson) -> String {
    match value {
        Bson::Int32(v) => v.to_string(),
        Bson::Int64(v) => v.to_string(),
        Bson::Double(v) => v.to_string(),
        Bson::String(v) => v.clone(),
        _ => "1".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

async fn send_event(sink: &mpsc::Sender<QueryEvent>, event: QueryEvent) -> MieoResult<()> {
    sink.send(event).await.map_err(|_| MieoError::cancelled())
}

/// Await `future`, aborting on cancellation or when the query deadline passes.
async fn with_cancel_and_timeout<F: Future>(
    future: F,
    deadline: Option<tokio::time::Instant>,
    cancel: &CancellationToken,
) -> MieoResult<F::Output> {
    tokio::pin!(future);
    if let Some(deadline) = deadline {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MieoError::cancelled()),
            _ = tokio::time::sleep_until(deadline) => {
                Err(MieoError::new(ErrorCode::Timeout, "query timed out"))
            }
            output = &mut future => Ok(output),
        }
    } else {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MieoError::cancelled()),
            output = &mut future => Ok(output),
        }
    }
}

async fn next_document(
    cursor: &mut Pin<Box<Cursor<Document>>>,
    deadline: Option<tokio::time::Instant>,
    cancel: &CancellationToken,
) -> MieoResult<Option<Document>> {
    let next = with_cancel_and_timeout(cursor.next(), deadline, cancel).await?;
    next.transpose().map_err(map_mongo_err)
}

/// Stream a document cursor as `Columns` + `Batch` events, 256 docs per batch.
async fn stream_cursor(
    query_id: &str,
    cursor: Cursor<Document>,
    cap: u64,
    deadline: Option<tokio::time::Instant>,
    cancel: &CancellationToken,
    sink: &mpsc::Sender<QueryEvent>,
    stats: &mut QueryStats,
) -> MieoResult<()> {
    let mut cursor = Box::pin(cursor);
    let mut pending: Vec<Document> = Vec::with_capacity(BATCH_SIZE);
    let mut columns: Option<(Vec<InferredField>, usize)> = None;
    let mut batch_index: u64 = 0;
    let mut truncated = false;

    loop {
        let Some(document) = next_document(&mut cursor, deadline, cancel).await? else {
            break;
        };
        // One document past the cap is enough to know the result was cut short.
        if stats.rows >= cap {
            truncated = true;
            break;
        }
        stats.rows += 1;
        pending.push(document);
        if pending.len() >= BATCH_SIZE {
            emit_pending(query_id, &mut pending, &mut columns, &mut batch_index, sink).await?;
        }
    }

    if !pending.is_empty() {
        emit_pending(query_id, &mut pending, &mut columns, &mut batch_index, sink).await?;
    }
    stats.truncated = truncated;
    Ok(())
}

/// Emit the buffered documents; the first call infers and sends the columns.
async fn emit_pending(
    query_id: &str,
    pending: &mut Vec<Document>,
    columns: &mut Option<(Vec<InferredField>, usize)>,
    batch_index: &mut u64,
    sink: &mpsc::Sender<QueryEvent>,
) -> MieoResult<()> {
    let documents = std::mem::take(pending);

    if columns.is_none() {
        let inferred = infer_fields(&documents);
        let sample_len = documents.len();
        let metas = inferred
            .iter()
            .enumerate()
            .map(|(ordinal, field)| field.to_column_meta(ordinal, sample_len))
            .collect();
        send_event(
            sink,
            QueryEvent::Columns {
                query_id: query_id.to_string(),
                columns: metas,
            },
        )
        .await?;
        *columns = Some((inferred, sample_len));
    }

    let fields = match columns.as_ref() {
        Some((fields, _)) => fields.as_slice(),
        None => &[],
    };
    let rows: Vec<Vec<Value>> = documents
        .iter()
        .map(|document| document_to_row(document, fields))
        .collect();
    *batch_index += 1;
    send_event(
        sink,
        QueryEvent::Batch {
            query_id: query_id.to_string(),
            rows,
            batch_index: *batch_index,
        },
    )
    .await
}

fn clamp_limit(limit: u64) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

// ---------------------------------------------------------------------------
// Connection string & error mapping
// ---------------------------------------------------------------------------

/// Assemble a `mongodb://` connection string from the spec and its params.
///
/// The path component stays empty (auth defaults to `admin` unless
/// `auth_source` says otherwise); the default database is applied on the
/// parsed [`ClientOptions`]. TLS is enabled when `ssl_mode != disable`, the
/// profile is a `documentdb` variant, `tls_insecure` is set or a CA bundle is
/// given. `tls_insecure` (and `ssl_mode = require`) disables certificate
/// verification — required for the DocumentDB global bundle workflow.
fn build_uri(spec: &ConnectionSpec) -> String {
    let mut uri = String::from("mongodb://");

    if let Some(username) = spec.username.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        uri.push_str(&pct_encode(username));
        if let Some(password) = spec.password.as_deref() {
            uri.push(':');
            uri.push_str(&pct_encode(password));
        }
        uri.push('@');
    }

    let port = spec.port.unwrap_or(27017);
    let host = spec.host.as_deref().unwrap_or("localhost");
    let hosts: Vec<String> = host
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(|h| {
            if h.starts_with('[') {
                // IPv6 literal: only append the port when none is present.
                if h.contains("]:") {
                    h.to_string()
                } else {
                    format!("{h}:{port}")
                }
            } else if h.contains(':') {
                h.to_string()
            } else {
                format!("{h}:{port}")
            }
        })
        .collect();
    if hosts.is_empty() {
        uri.push_str(&format!("localhost:{port}"));
    } else {
        uri.push_str(&hosts.join(","));
    }
    uri.push('/');

    let insecure = param_truthy(&spec.params, "tls_insecure");
    let ca_file = spec
        .params
        .get("tls_ca_file")
        .map(String::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let tls_enabled =
        spec.ssl_mode != SslMode::Disable || is_documentdb(spec) || insecure || ca_file.is_some();
    let allow_invalid = insecure || spec.ssl_mode == SslMode::Require;

    let mut options: Vec<(String, String)> = Vec::new();
    // `tls` must come first: TLS options after `tls=false` are rejected.
    options.push(("tls".to_string(), tls_enabled.to_string()));
    if allow_invalid {
        options.push(("tlsInsecure".to_string(), "true".to_string()));
    }
    if let Some(ca_file) = ca_file {
        options.push(("tlsCAFile".to_string(), ca_file.to_string()));
    }
    if let Some(replica_set) = spec
        .params
        .get("replica_set")
        .map(String::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        options.push(("replicaSet".to_string(), replica_set.to_string()));
    }
    if param_truthy(&spec.params, "direct") {
        options.push(("directConnection".to_string(), "true".to_string()));
    }
    if let Some(auth_source) = spec
        .params
        .get("auth_source")
        .map(String::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        options.push(("authSource".to_string(), auth_source.to_string()));
    }
    let app_name = spec
        .params
        .get("app_name")
        .map(String::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_APP_NAME);
    options.push(("appName".to_string(), app_name.to_string()));
    let timeout_ms = spec.connect_timeout_secs.max(1).saturating_mul(1000);
    options.push(("connectTimeoutMS".to_string(), timeout_ms.to_string()));
    options.push(("serverSelectionTimeoutMS".to_string(), timeout_ms.to_string()));

    uri.push('?');
    uri.push_str(
        &options
            .iter()
            .map(|(key, value)| format!("{key}={}", pct_encode(value)))
            .collect::<Vec<_>>()
            .join("&"),
    );
    uri
}

fn is_documentdb(spec: &ConnectionSpec) -> bool {
    spec.variant
        .as_deref()
        .is_some_and(|variant| variant.eq_ignore_ascii_case("documentdb"))
}

/// `true`/`1`/`yes`/`on` (case-insensitive) are treated as enabled.
fn param_truthy(params: &BTreeMap<String, String>, key: &str) -> bool {
    params.get(key).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes" | "on"
        )
    })
}

/// Percent-encode everything outside the RFC 3986 unreserved set.
fn pct_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn map_mongo_err(error: MongoError) -> MieoError {
    let (code, message, db_code, hint) = match &*error.kind {
        MongoErrorKind::InvalidArgument { message, .. } => {
            (ErrorCode::Connection, message.clone(), None, None)
        }
        MongoErrorKind::Authentication { message, .. } => {
            (ErrorCode::Auth, message.clone(), None, None)
        }
        MongoErrorKind::Command(command) => {
            let code = match command.code {
                13 | 18 => ErrorCode::Auth, // Unauthorized / AuthenticationFailed
                11 | 26 | 27 | 43 => ErrorCode::NotFound, // user / namespace / index / cursor
                50 => ErrorCode::Timeout, // MaxTimeMSExpired
                11000 | 11001 => ErrorCode::Constraint, // duplicate key
                91 | 189 | 11600 | 11601 | 11602 | 13435 | 13436 => ErrorCode::Connection,
                _ => ErrorCode::Internal,
            };
            let hint = (!command.code_name.is_empty()).then(|| command.code_name.clone());
            (
                code,
                command.message.clone(),
                Some(command.code.to_string()),
                hint,
            )
        }
        MongoErrorKind::DnsResolve { message, .. } => {
            (ErrorCode::Network, message.clone(), None, None)
        }
        MongoErrorKind::ServerSelection { message, .. } => {
            (ErrorCode::Connection, message.clone(), None, None)
        }
        MongoErrorKind::Io(io) => (ErrorCode::Network, format!("I/O error: {io}"), None, None),
        MongoErrorKind::InvalidTlsConfig { message, .. } => {
            (ErrorCode::Network, format!("TLS error: {message}"), None, None)
        }
        MongoErrorKind::ConnectionPoolCleared { message, .. } => {
            (ErrorCode::Connection, message.clone(), None, None)
        }
        MongoErrorKind::InvalidResponse { message, .. } => {
            (ErrorCode::Connection, message.clone(), None, None)
        }
        MongoErrorKind::Internal { message, .. } => {
            (ErrorCode::Internal, message.clone(), None, None)
        }
        MongoErrorKind::BsonDeserialization(inner) => (
            ErrorCode::Data,
            format!("BSON decode error: {inner}"),
            None,
            None,
        ),
        MongoErrorKind::BsonSerialization(inner) => (
            ErrorCode::Data,
            format!("BSON encode error: {inner}"),
            None,
            None,
        ),
        MongoErrorKind::Write(failure) => {
            (ErrorCode::Data, format!("write error: {failure:?}"), None, None)
        }
        MongoErrorKind::Transaction { message, .. } => {
            (ErrorCode::Conflict, message.clone(), None, None)
        }
        MongoErrorKind::IncompatibleServer { message, .. } => {
            (ErrorCode::Unsupported, message.clone(), None, None)
        }
        MongoErrorKind::SessionsNotSupported => (
            ErrorCode::Unsupported,
            "server does not support sessions".to_string(),
            None,
            None,
        ),
        other => (ErrorCode::Internal, other.to_string(), None, None),
    };

    let mut mapped = MieoError::new(code, message);
    if let Some(db_code) = db_code {
        mapped = mapped.with_db_code(db_code);
    }
    if let Some(hint) = hint {
        mapped = mapped.with_hint(hint);
    }
    mapped.with_source(error)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mieosql_core::ProfileId;
    use mongodb::bson::spec::BinarySubtype;
    use mongodb::bson::{oid::ObjectId, Binary, DateTime, Decimal128};

    fn base_spec() -> ConnectionSpec {
        ConnectionSpec {
            profile_id: Some(ProfileId("p_mongo_test".to_string())),
            name: "mongo-test".to_string(),
            engine: Engine::MongoDb,
            host: Some("localhost".to_string()),
            port: Some(27017),
            username: Some("admin".to_string()),
            password: Some("p@ss:w/rd".to_string()),
            database: Some("app".to_string()),
            file: None,
            params: BTreeMap::new(),
            ssl_mode: SslMode::Prefer,
            variant: None,
            ssh: None,
            ssh_password: None,
            connect_timeout_secs: 10,
        }
    }

    #[test]
    fn uri_encodes_credentials_and_defaults() {
        let uri = build_uri(&base_spec());
        assert!(
            uri.starts_with("mongodb://admin:p%40ss%3Aw%2Frd@localhost:27017/?"),
            "unexpected uri: {uri}"
        );
        assert!(uri.contains("tls=true"));
        assert!(uri.contains("appName=MieoSQL"));
        assert!(uri.contains("connectTimeoutMS=10000"));
        assert!(uri.contains("serverSelectionTimeoutMS=10000"));
        // No database in the path: the auth source stays `admin`.
        assert!(!uri.contains("/app?"));
    }

    #[test]
    fn uri_multiple_hosts_get_the_default_port() {
        let mut spec = base_spec();
        spec.host = Some("h1, h2".to_string());
        spec.port = Some(27018);
        spec.ssl_mode = SslMode::Disable;
        let uri = build_uri(&spec);
        assert!(uri.contains("@h1:27018,h2:27018/"), "unexpected uri: {uri}");
        assert!(uri.contains("tls=false"));
    }

    #[test]
    fn uri_documentdb_forces_tls() {
        let mut spec = base_spec();
        spec.ssl_mode = SslMode::Disable;
        spec.variant = Some("documentdb".to_string());
        let uri = build_uri(&spec);
        assert!(uri.contains("tls=true"));
    }

    #[test]
    fn uri_require_disables_certificate_verification() {
        let mut spec = base_spec();
        spec.ssl_mode = SslMode::Require;
        let uri = build_uri(&spec);
        assert!(uri.contains("tls=true"));
        assert!(uri.contains("tlsInsecure=true"));
    }

    #[test]
    fn uri_params_are_forwarded() {
        let mut spec = base_spec();
        spec.ssl_mode = SslMode::Disable;
        spec.params.insert("auth_source".to_string(), "admin".to_string());
        spec.params.insert("replica_set".to_string(), "rs0".to_string());
        spec.params.insert("tls_insecure".to_string(), "true".to_string());
        spec.params.insert("direct".to_string(), "1".to_string());
        spec.params.insert("app_name".to_string(), "My App".to_string());
        let uri = build_uri(&spec);
        assert!(uri.contains("tls=true"));
        assert!(uri.contains("tlsInsecure=true"));
        assert!(uri.contains("replicaSet=rs0"));
        assert!(uri.contains("directConnection=true"));
        assert!(uri.contains("authSource=admin"));
        assert!(uri.contains("appName=My%20App"));
    }

    #[test]
    fn uri_ca_file_enables_tls() {
        let mut spec = base_spec();
        spec.ssl_mode = SslMode::Disable;
        spec.params.insert(
            "tls_ca_file".to_string(),
            "/etc/ssl/global-bundle.pem".to_string(),
        );
        let uri = build_uri(&spec);
        assert!(uri.contains("tls=true"));
        assert!(uri.contains("tlsCAFile=%2Fetc%2Fssl%2Fglobal-bundle.pem"));
    }

    #[test]
    fn percent_encoding_keeps_unreserved_bytes() {
        assert_eq!(pct_encode("aZ0-._~"), "aZ0-._~");
        assert_eq!(pct_encode("p@ss:w/rd"), "p%40ss%3Aw%2Frd");
    }

    #[test]
    fn parse_find_with_limit() {
        let command = parse_command("db.users.find({\"age\": {\"$gt\": 21}}).limit(10)").unwrap();
        match command {
            MongoCommand::Find {
                collection,
                filter,
                limit,
            } => {
                assert_eq!(collection, "users");
                assert_eq!(limit, Some(10));
                // JSON integers may arrive as Int32 or Int64 depending on serde.
                let gt = filter
                    .get_document("age")
                    .expect("age filter")
                    .get("$gt")
                    .expect("$gt");
                assert!(
                    matches!(gt, Bson::Int32(21) | Bson::Int64(21)),
                    "unexpected $gt: {gt:?}"
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_find_without_limit_and_with_spaces() {
        let command = parse_command("  db.users.find( {} )  ").unwrap();
        match command {
            MongoCommand::Find {
                collection,
                filter,
                limit,
            } => {
                assert_eq!(collection, "users");
                assert!(filter.is_empty());
                assert_eq!(limit, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_find_ignores_braces_and_parens_inside_strings() {
        let command =
            parse_command("db.orders.find({\"note\": \"a)b}c\", \"meta\": {\"x\": 1}}).limit(5)")
                .unwrap();
        match command {
            MongoCommand::Find {
                collection,
                filter,
                limit,
            } => {
                assert_eq!(collection, "orders");
                assert_eq!(limit, Some(5));
                assert_eq!(
                    filter.get_str("note").expect("note"),
                    "a)b}c",
                    "string content must not confuse the scanner"
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_aggregate_pipeline() {
        let command = parse_command(
            "db.events.aggregate([{\"$match\": {\"kind\": \"click\"}}, {\"$group\": {\"_id\": \"$user\", \"n\": {\"$sum\": 1}}}])",
        )
        .unwrap();
        match command {
            MongoCommand::Aggregate {
                collection,
                pipeline,
                limit,
            } => {
                assert_eq!(collection, "events");
                assert_eq!(pipeline.len(), 2);
                assert_eq!(limit, None);
                assert!(pipeline[0].contains_key("$match"));
                assert!(pipeline[1].contains_key("$group"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_aggregate_with_trailing_limit() {
        let command =
            parse_command("db.events.aggregate([{\"$match\": {}}]).limit(3)").unwrap();
        match command {
            MongoCommand::Aggregate { limit, .. } => assert_eq!(limit, Some(3)),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_count_documents() {
        let command = parse_command("db.users.countDocuments({\"active\": true})").unwrap();
        match command {
            MongoCommand::CountDocuments { collection, filter } => {
                assert_eq!(collection, "users");
                assert_eq!(filter.get_bool("active").expect("active"), true);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_json_form() {
        let command =
            parse_command("{\"collection\": \"users\", \"find\": {\"active\": true}, \"limit\": 25}")
                .unwrap();
        match command {
            MongoCommand::Find {
                collection,
                filter,
                limit,
            } => {
                assert_eq!(collection, "users");
                assert_eq!(limit, Some(25));
                assert_eq!(filter.get_bool("active").expect("active"), true);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let aggregate = parse_command(
            "{\"collection\": \"events\", \"aggregate\": [{\"$match\": {}}], \"limit\": 5}",
        )
        .unwrap();
        match aggregate {
            MongoCommand::Aggregate {
                pipeline, limit, ..
            } => {
                assert_eq!(pipeline.len(), 1);
                assert_eq!(limit, Some(5));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let count = parse_command("{\"collection\": \"c\", \"countDocuments\": {}}").unwrap();
        assert!(matches!(count, MongoCommand::CountDocuments { .. }));
    }

    #[test]
    fn parse_errors_are_syntax_with_hint() {
        for query in [
            "SELECT 1",
            "db.users.find({\"a\": 1}",
            "db.users.drop({})",
            "db.users.find({}).limit(x)",
            "db.users.find({}).limit()",
            "db.users.countDocuments({}).limit(1)",
            "{}",
            "{\"collection\": \"c\"}",
            "{\"collection\": \"c\", \"find\": {}, \"wat\": 1}",
            "{\"collection\": \"c\", \"find\": [], }",
        ] {
            let error = parse_command(query).expect_err(query);
            assert_eq!(error.code(), ErrorCode::Syntax, "query: {query}");
            assert!(error.info.hint.is_some(), "query: {query}");
        }
    }

    #[test]
    fn bson_to_value_maps_primitives() {
        assert_eq!(bson_to_value(&Bson::Double(1.5)), Value::Float(1.5));
        assert_eq!(
            bson_to_value(&Bson::String("hi".into())),
            Value::Text("hi".into())
        );
        assert_eq!(bson_to_value(&Bson::Boolean(true)), Value::Bool(true));
        assert_eq!(bson_to_value(&Bson::Null), Value::Null);
        assert_eq!(bson_to_value(&Bson::Int32(7)), Value::Int(7));
        assert_eq!(bson_to_value(&Bson::Int64(1 << 40)), Value::Int(1 << 40));

        let oid = ObjectId::parse_str("64b7f0f0f0f0f0f0f0f0f0f0").expect("oid");
        assert_eq!(
            bson_to_value(&Bson::ObjectId(oid)),
            Value::Uuid("64b7f0f0f0f0f0f0f0f0f0f0".into())
        );

        let binary = Binary {
            subtype: BinarySubtype::Generic,
            bytes: vec![1, 2, 3],
        };
        assert_eq!(
            bson_to_value(&Bson::Binary(binary)),
            Value::Bytes(b64_encode(&[1, 2, 3]))
        );

        match bson_to_value(&Bson::DateTime(DateTime::from_millis(0))) {
            Value::DateTime(text) => assert!(text.starts_with("1970-01-01T00:00:00")),
            other => panic!("unexpected date value: {other:?}"),
        }

        let decimal: Decimal128 = "123.45".parse().expect("decimal");
        assert_eq!(bson_to_value(&Bson::Decimal128(decimal)), Value::Decimal("123.45".into()));
    }

    #[test]
    fn bson_to_value_maps_documents_to_extended_json() {
        let document = doc! { "a": 1, "nested": { "b": true }, "arr": [1, "x"] };
        match bson_to_value(&Bson::Document(document)) {
            Value::Json(json) => {
                assert_eq!(json["a"], serde_json::json!(1));
                assert_eq!(json["nested"]["b"], serde_json::json!(true));
                assert_eq!(json["arr"][1], serde_json::json!("x"));
            }
            other => panic!("unexpected document value: {other:?}"),
        }
    }

    #[test]
    fn infer_fields_unions_in_first_seen_order() {
        let documents = vec![
            doc! { "a": 1, "b": "x" },
            doc! { "a": 2.5, "c": true },
            doc! { "a": 3, "b": "y" },
        ];
        let fields = infer_fields(&documents);
        let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);

        // `int` widened to `double`; both documents contain it.
        assert_eq!(fields[0].kind, DataTypeKind::Float);
        assert_eq!(fields[0].native_type, "double");
        assert!(!fields[0].nullable(3));

        // Present in 2 of 3 documents.
        assert_eq!(fields[1].kind, DataTypeKind::Text);
        assert!(fields[1].nullable(3));

        assert_eq!(fields[2].kind, DataTypeKind::Bool);
        assert!(fields[2].nullable(3));
    }

    #[test]
    fn infer_fields_null_then_value() {
        let documents = vec![doc! { "a": null }, doc! { "a": "text" }];
        let fields = infer_fields(&documents);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].kind, DataTypeKind::Text);
        assert!(fields[0].nullable(2));
    }

    #[test]
    fn document_row_alignment_fills_missing_fields_with_null() {
        let documents = vec![doc! { "a": 1 }, doc! { "b": 2 }];
        let fields = infer_fields(&documents);
        assert_eq!(document_to_row(&documents[0], &fields), vec![Value::Int(1), Value::Null]);
        assert_eq!(document_to_row(&documents[1], &fields), vec![Value::Null, Value::Int(2)]);
    }

    #[test]
    fn index_name_from_keys_uses_field_order() {
        let keys = doc! { "a": 1, "b": -1, "c": "text" };
        assert_eq!(index_name_from_keys(&keys), "a_1_b_-1_c_text");
    }

    #[test]
    fn json_form_parses_documents_from_serde_values() {
        let filter = json_object("{\"nested\": {\"x\": [1, 2]}}").expect("filter");
        assert!(filter.contains_key("nested"));
    }
}
