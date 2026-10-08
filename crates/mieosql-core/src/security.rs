//! Database users, roles and grants shared by all engines.
//!
//! The model is intentionally small: a principal (user or role), role
//! membership edges, textual grant lines for display, and the three write
//! requests the authorization UI needs (create / drop / grant / revoke /
//! password). Every identifier travels as a plain name — drivers quote it
//! with the engine's own rules, so generated SQL can never be injected
//! through these DTOs.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// User or role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PrincipalKind {
    User,
    Role,
}

/// A login / user account or a role, depending on `kind`.
///
/// - MySQL/MariaDB: `name` = `user`, `host` = `host` (`user@host`).
/// - PostgreSQL: roles and users share `pg_roles`; `can_login` tells them
///   apart. `host` is unused.
/// - SQL Server: `name` is the database principal; server-level roles carry
///   `comment = "server role"`. `host` is unused.
/// - MongoDB: `name` = username, `host` = authentication database.
/// - Redis: `name` = ACL username. There are no roles.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DbPrincipal {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub host: Option<String>,
    pub kind: PrincipalKind,
    /// Superuser / root-like flag (`SUPER`, `rolsuper`, `sysadmin`, ...).
    #[serde(default)]
    pub is_super: bool,
    /// Whether the principal can open sessions (login / `rolcanlogin`).
    #[serde(default)]
    pub can_login: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
}

/// One role-membership edge: `member` is granted `role`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoleGrant {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub role_host: Option<String>,
    pub member: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub member_host: Option<String>,
    #[serde(default)]
    pub admin_option: bool,
}

/// Create a user (or role, where the engine supports it).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateDbUser {
    pub name: String,
    /// MySQL/MariaDB host part (`user@host`); defaults to `%` when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub host: Option<String>,
    /// MongoDB authentication database; defaults to the session database.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub password: Option<String>,
    /// Grant administrative rights right away (`ALL PRIVILEGES` / `SUPERUSER`).
    #[serde(default)]
    pub superuser: bool,
    /// The principal may open sessions (Postgres `LOGIN`, Redis `on`).
    #[serde(default)]
    pub login: bool,
}

/// Drop a user or role.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DropDbPrincipal {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub host: Option<String>,
    pub kind: PrincipalKind,
}

/// What to grant (or revoke): object privileges or role membership.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GrantTarget {
    /// Object privileges, e.g. `SELECT, INSERT`.
    /// No table = database-level grant (`db.*`, `DATABASE`, ...).
    Privileges {
        privileges: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        database: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        schema: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        table: Option<String>,
    },
    /// Role membership (`GRANT role TO member`).
    Role {
        role: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        host: Option<String>,
    },
}

/// Grant privileges or a role to a principal.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GrantRequest {
    pub grantee: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub grantee_host: Option<String>,
    pub target: GrantTarget,
    #[serde(default)]
    pub with_grant_option: bool,
}

/// Revoke privileges or a role from a principal.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RevokeRequest {
    pub grantee: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub grantee_host: Option<String>,
    pub target: GrantTarget,
}

/// One key/value row for the Advanced tab (limits, expiry, defaults...).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DetailAttribute {
    pub key: String,
    pub value: String,
}

/// Object-level grant: `privileges` on one database / schema / table.
/// No table = database-level grant.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PrivilegeEntry {
    pub privileges: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub table: Option<String>,
    /// May re-grant to others (`WITH GRANT OPTION` / grant option).
    #[serde(default)]
    pub grantable: bool,
}

/// Everything the user/role editor needs for one principal.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DbPrincipalDetail {
    pub principal: DbPrincipal,
    pub attributes: Vec<DetailAttribute>,
    pub global_privileges: Vec<String>,
    pub object_privileges: Vec<PrivilegeEntry>,
    /// Roles this principal is a member of.
    pub member_of: Vec<RoleGrant>,
    /// Members of this role (empty for users).
    pub members: Vec<RoleGrant>,
}
