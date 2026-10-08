//! Database users, roles and grants.
//!
//! Thin async adapters over the engine facade plus input validation. The
//! privilege list of a `Privileges` grant travels as a single string (the UI
//! offers checkboxes); it is checked here against a strict character set so
//! it can be interpolated into `GRANT` safely — every *name* (user, role,
//! database, table) is quoted by the drivers themselves.

use mieosql_core::{
    CreateDbUser,
    DbPrincipal,
    DropDbPrincipal,
    ErrorCode,
    GrantRequest,
    GrantTarget,
    MieoError,
    RevokeRequest,
    RoleGrant,
    DbPrincipalDetail,
};

use crate::dto::{ApiError, ApiResult};
use crate::service::Api;

fn check_name(what: &str, name: &str) -> ApiResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(MieoError::new(ErrorCode::Data, format!("{what} must not be empty")).into());
    }
    if name.len() > 128 {
        return Err(MieoError::new(ErrorCode::Data, format!("{what} is too long")).into());
    }
    if name.contains('\0') {
        return Err(MieoError::new(ErrorCode::Data, format!("{what} is invalid")).into());
    }
    Ok(())
}

/// Privilege lists (`SELECT, INSERT`, `+@read`, ...) may only carry privilege
/// tokens — never quotes, semicolons or comments.
fn check_privileges(privileges: &str) -> ApiResult<()> {
    let trimmed = privileges.trim();
    if trimmed.is_empty() {
        return Err(MieoError::new(ErrorCode::Data, "privileges must not be empty").into());
    }
    if trimmed.len() > 256 {
        return Err(MieoError::new(ErrorCode::Data, "privileges list is too long").into());
    }
    let ok = trimmed.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '+' | '-' | '*' | ',' | ' ' | '\t' | '.')
    });
    if !ok {
        return Err(MieoError::new(
            ErrorCode::Data,
            format!("invalid privileges `{trimmed}`"),
        )
        .with_hint("use privilege names only, e.g. `SELECT, INSERT`")
        .into());
    }
    Ok(())
}

fn check_grant_target(target: &GrantTarget) -> ApiResult<()> {
    match target {
        GrantTarget::Privileges {
            privileges,
            database,
            schema,
            table,
        } => {
            check_privileges(privileges)?;
            if let Some(db) = database.as_deref().filter(|d| !d.trim().is_empty()) {
                check_name("database", db)?;
            }
            if let Some(sc) = schema.as_deref().filter(|s| !s.trim().is_empty()) {
                check_name("schema", sc)?;
            }
            if let Some(tbl) = table.as_deref().filter(|t| !t.trim().is_empty()) {
                check_name("table", tbl)?;
            }
            Ok(())
        }
        GrantTarget::Role { role, host } => {
            check_name("role", role)?;
            if let Some(h) = host.as_deref().filter(|h| !h.trim().is_empty()) {
                check_name("role host", h)?;
            }
            Ok(())
        }
    }
}

impl Api {
    pub async fn list_db_users(&self, session_id: String) -> ApiResult<Vec<DbPrincipal>> {
        self.engine().list_db_users(&session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn list_db_roles(&self, session_id: String) -> ApiResult<Vec<DbPrincipal>> {
        self.engine().list_db_roles(&session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn db_role_members(&self, session_id: String) -> ApiResult<Vec<RoleGrant>> {
        self.engine().db_role_members(&session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn db_user_grants(
        &self,
        session_id: String,
        name: String,
        host: Option<String>,
    ) -> ApiResult<Vec<String>> {
        check_name("user", &name)?;
        self.engine().db_user_grants(&session_id, &name, host.as_deref())
            .await
            .map_err(Into::into)
    }

    pub async fn create_db_user(
        &self,
        session_id: String,
        request: CreateDbUser,
    ) -> ApiResult<()> {
        check_name("user", &request.name)?;
        if let Some(h) = request.host.as_deref().filter(|h| !h.trim().is_empty()) {
            check_name("host", h)?;
        }
        self.engine().create_db_user(&session_id, &request)
            .await
            .map_err(Into::into)
    }

    pub async fn drop_db_principal(
        &self,
        session_id: String,
        request: DropDbPrincipal,
    ) -> ApiResult<()> {
        check_name("principal", &request.name)?;
        self.engine().drop_db_principal(&session_id, &request)
            .await
            .map_err(Into::into)
    }

    pub async fn grant(&self, session_id: String, request: GrantRequest) -> ApiResult<()> {
        check_name("grantee", &request.grantee)?;
        check_grant_target(&request.target)?;
        self.engine().grant(&session_id, &request)
            .await
            .map_err(ApiError::from)
    }

    pub async fn revoke(&self, session_id: String, request: RevokeRequest) -> ApiResult<()> {
        check_name("grantee", &request.grantee)?;
        check_grant_target(&request.target)?;
        self.engine().revoke(&session_id, &request)
            .await
            .map_err(ApiError::from)
    }

    pub async fn db_principal_detail(
        &self,
        session_id: String,
        name: String,
        host: Option<String>,
    ) -> ApiResult<DbPrincipalDetail> {
        check_name("user", &name)?;
        self.engine()
            .db_principal_detail(&session_id, &name, host.as_deref())
            .await
            .map_err(Into::into)
    }

    pub async fn set_user_password(
        &self,
        session_id: String,
        name: String,
        host: Option<String>,
        password: String,
    ) -> ApiResult<()> {
        check_name("user", &name)?;
        if password.is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "password must not be empty").into());
        }
        self.engine().set_user_password(&session_id, &name, host.as_deref(), &password)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_privilege_injection() {
        assert!(check_privileges("SELECT, INSERT").is_ok());
        assert!(check_privileges("+@read +get").is_ok());
        assert!(check_privileges("SELECT; DROP TABLE x").is_err());
        assert!(check_privileges("SELECT' OR '1'='1").is_err());
        assert!(check_privileges("").is_err());
        assert!(check_privileges("   ").is_err());
    }

    #[test]
    fn rejects_empty_names() {
        assert!(check_name("user", "").is_err());
        assert!(check_name("user", "   ").is_err());
        assert!(check_name("user", "alice").is_ok());
    }
}
