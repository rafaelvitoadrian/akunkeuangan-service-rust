//! PostgreSQL implementation of the current Java role/profile service.
use super::postgres_impl::PostgresRepo;
use super::postgres_security_graph::SecurityGraph;
use crate::security::*;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sqlx::PgConnection;

pub(super) async fn require_function(
    conn: &mut PgConnection,
    email: &str,
    function: &str,
    minimum: i64,
) -> Result<()> {
    // ExecutingUser.hasAccess merges all assigned roles. Java currently does
    // not filter role activation or profile deactivation in this calculation.
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u \
        JOIN useraccountpermissionprofile up ON up.user_id=u.id \
        JOIN permissionprofile_userrole pr ON pr.profile_id=up.profile_id \
        JOIN userrolefunction rf ON rf.role_id=pr.role_id \
        JOIN security_function f ON f.id=rf.function_id \
        WHERE u.email=$1 AND u.ident_master IS NULL AND f.function_name=$2 AND rf.accesslevel_id >= $3)")
        .bind(email).bind(function).bind(minimum).fetch_one(&mut *conn).await?;
    if !allowed {
        return Err(AccessDenied(function.into()).into());
    }
    Ok(())
}

async fn role(conn: &mut PgConnection, id: i64) -> Result<Value> {
    SecurityGraph::load(conn).await?.role(id)
}
async fn profile(conn: &mut PgConnection, id: i64) -> Result<Value> {
    SecurityGraph::load(conn).await?.profile(id)
}
pub(super) async fn assemble_user(conn: &mut PgConnection, raw: Value) -> Result<Value> {
    SecurityGraph::load(conn).await?.assemble_user(&raw)
}
async fn save_role(
    conn: &mut PgConnection,
    req: RoleRequest,
    actor: Option<i64>,
    create: bool,
) -> Result<Value> {
    let id = req.ident_user_role;
    // Match UserRoleValidator: duplicate matching ignores case; internal roles
    // cannot be updated/deleted even when the client sends internal=false.
    let duplicate:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM userrole WHERE NOT internal AND upper(shortname)=upper($1) AND id<>$2)")
        .bind(&req.shortname).bind(id).fetch_one(&mut *conn).await?;
    ensure!(
        !duplicate,
        "User Role short name {} is not unique.",
        req.shortname.as_deref().unwrap_or("")
    );
    if !create {
        assert_mutable(conn, id, req.shortname.as_deref()).await?;
    }
    let status = req.activation_status.as_ref().map(ActivationStatus::as_str);
    let now = crate::util::app_now_string();
    let id = if create {
        ensure!(id == 0, "create role: primary key provided");
        sqlx::query_scalar("INSERT INTO userrole(shortname,name,description,activation_status,last_update,applicable_for_poa,internal,last_update_user) VALUES($1,$2,$3,$4,$5::text::timestamp,$6,$7,$8) RETURNING id")
            .bind(&req.shortname).bind(&req.name).bind(&req.description).bind(status).bind(&now).bind(req.applicable_for_p_o_a).bind(req.internal).bind(actor)
            .fetch_one(&mut *conn).await?
    } else {
        sqlx::query_scalar("UPDATE userrole SET shortname=$2,name=$3,description=$4,activation_status=$5,last_update=$6::text::timestamp,applicable_for_poa=$7,internal=$8,last_update_user=$9 WHERE id=$1 RETURNING id")
            .bind(id).bind(&req.shortname).bind(&req.name).bind(&req.description).bind(status).bind(&now).bind(req.applicable_for_p_o_a).bind(req.internal).bind(actor)
            .fetch_one(&mut *conn).await?
    };
    // Preserve row identities for functions that stay selected, as JPA does.
    let mut selected = Vec::new();
    for entry in req.user_role_functions {
        let function = entry.function.context("missing function")?.id;
        let access = entry.access_level.context("missing access level")?.id;
        if selected.contains(&function) {
            continue;
        }
        selected.push(function);
        sqlx::query("INSERT INTO userrolefunction(role_id,function_id,accesslevel_id) VALUES($1,$2,$3) ON CONFLICT(role_id,function_id) DO UPDATE SET accesslevel_id=EXCLUDED.accesslevel_id")
            .bind(id).bind(function).bind(access).execute(&mut *conn).await?;
    }
    sqlx::query("DELETE FROM userrolefunction WHERE role_id=$1 AND NOT(function_id=ANY($2))")
        .bind(id)
        .bind(&selected)
        .execute(&mut *conn)
        .await?;
    role(conn, id).await
}
async fn assert_mutable(conn: &mut PgConnection, id: i64, name: Option<&str>) -> Result<()> {
    let internal: bool = sqlx::query_scalar("SELECT internal FROM userrole WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .context("User role tidak ditemukan")?;
    ensure!(
        !internal,
        "User Role '{}' cannot be modified/deleted since it is internal.",
        name.unwrap_or("")
    );
    Ok(())
}
async fn save_profile(conn: &mut PgConnection, req: ProfileRequest, create: bool) -> Result<i64> {
    let id = req.ident_permission_profile;
    let owner = req.user_account.as_ref().map(|u| u.ident);
    let date = req.deactivation_date.map(|d| d.to_string());
    let id = if create {
        ensure!(id == 0, "create profile: primary key provided");
        sqlx::query_scalar("INSERT INTO permissionprofile(name,user_id,deactivation_date) VALUES($1,$2,$3::text::date) RETURNING id")
            .bind(req.name).bind(owner).bind(date).fetch_one(&mut *conn).await?
    } else {
        sqlx::query_scalar("UPDATE permissionprofile SET name=$2,user_id=$3,deactivation_date=$4::text::date WHERE id=$1 RETURNING id")
            .bind(id).bind(req.name).bind(owner).bind(date).fetch_one(&mut *conn).await?
    };
    sqlx::query("DELETE FROM permissionprofile_userrole WHERE profile_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for role in req.user_roles {
        sqlx::query("INSERT INTO permissionprofile_userrole(profile_id,role_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
            .bind(id).bind(role.ident_user_role).execute(&mut *conn).await?;
    }
    Ok(id)
}

pub(super) struct PreparedAssignments {
    pub ids: Vec<i64>,
    pub individual: Vec<i64>,
}
pub(super) async fn prepare_assignments(
    conn: &mut PgConnection,
    req: &crate::models::UsersRequest,
) -> Result<PreparedAssignments> {
    let mut result = PreparedAssignments {
        ids: Vec::new(),
        individual: Vec::new(),
    };
    for item in req.permission_profiles.as_deref().unwrap_or(&[]) {
        let id = if item.ident_permission_profile > 0 && item.user_account.is_none() {
            sqlx::query_scalar("SELECT id FROM permissionprofile WHERE id=$1")
                .bind(item.ident_permission_profile)
                .fetch_one(&mut *conn)
                .await?
        } else {
            let mut item = item.clone();
            if item.ident_permission_profile <= 0 {
                let Some(username) = req.username.as_ref() else {
                    continue;
                };
                item.name = Some(format!("AKN/{}", username.to_uppercase()));
            }
            // UsersConverter derives AKN/USERNAME only for a new profile.
            // Existing individual profiles are copied with the requested
            // name/roles/date intact, regardless of a username change.
            item.ident_permission_profile = 0;
            item.user_account = None;
            let id = save_profile(conn, item, true).await?;
            result.individual.push(id);
            id
        };
        if !result.ids.contains(&id) {
            result.ids.push(id);
        }
    }
    Ok(result)
}
pub(super) async fn replace_assignments(
    conn: &mut PgConnection,
    user: i64,
    profiles: &[i64],
) -> Result<()> {
    sqlx::query("DELETE FROM useraccountpermissionprofile WHERE user_id=$1")
        .bind(user)
        .execute(&mut *conn)
        .await?;
    for id in profiles {
        sqlx::query("INSERT INTO useraccountpermissionprofile VALUES($1,$2)")
            .bind(user)
            .bind(id)
            .execute(&mut *conn)
            .await?;
        // Shared profiles keep their owner null; individual clones are marked
        // by the caller after the user has a primary key.
    }
    Ok(())
}
pub(super) async fn copy_assignments(
    conn: &mut PgConnection,
    source: i64,
    target: i64,
) -> Result<()> {
    if source == target {
        return Ok(());
    }
    sqlx::query("DELETE FROM useraccountpermissionprofile WHERE user_id=$1")
        .bind(target)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO useraccountpermissionprofile SELECT $2,profile_id FROM useraccountpermissionprofile WHERE user_id=$1")
        .bind(source).bind(target).execute(conn).await?;
    Ok(())
}

impl PostgresRepo {
    /// Explicit operator action, never an HTTP endpoint or an automatic grant
    /// to imported users. Useful for assigning the first PostgreSQL admin.
    pub async fn grant_security_admin(&self, email: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(724891345013::bigint)")
            .execute(&mut *tx)
            .await?;
        let users = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM users WHERE email=$1 AND ident_master IS NULL",
        )
        .bind(email)
        .fetch_all(&mut *tx)
        .await?;
        ensure!(
            users.len() == 1,
            "admin email must resolve to exactly one existing master user"
        );
        grant_admin(&mut tx, users[0]).await?;
        tx.commit().await?;
        Ok(())
    }
    pub(super) async fn security_read(&self, query: SecurityQuery, email: &str) -> Result<Value> {
        let mut conn = self.pool.acquire().await?;
        let graph = SecurityGraph::load(&mut conn).await?;
        match query {
            SecurityQuery::Roles(pattern) => {
                let ids = if let Some(pattern) = pattern {
                    ensure!(!pattern.is_empty(), "shortName is mandatory");
                    let chars: Vec<char> = pattern.chars().collect();
                    if chars.len() >= 3 {
                        ensure!(
                            !chars[1..chars.len() - 1]
                                .iter()
                                .any(|c| matches!(c, '*' | '?')),
                            "wildcard may only be first and/or last"
                        );
                    }
                    sqlx::query_scalar::<_,i64>("SELECT id FROM userrole WHERE NOT internal AND upper(shortname) LIKE upper($1) ORDER BY id")
                        .bind(pattern.replace(['*','?'],"%")).fetch_all(&mut *conn).await?
                } else {
                    sqlx::query_scalar("SELECT id FROM userrole ORDER BY id")
                        .fetch_all(&mut *conn)
                        .await?
                };
                let mut values = Vec::new();
                for id in ids {
                    values.push(graph.role(id)?);
                }
                Ok(json!(values))
            }
            SecurityQuery::PoaRoles => {
                let ids = sqlx::query_scalar(
                    "SELECT id FROM userrole WHERE applicable_for_poa ORDER BY id",
                )
                .fetch_all(&mut *conn)
                .await?;
                let mut values = Vec::new();
                for id in ids {
                    values.push(graph.role(id)?);
                }
                Ok(json!(values))
            }
            SecurityQuery::Functions => {
                let mut values = Vec::new();
                for function in graph.functions.values() {
                    let mut value = base(0);
                    value["function"] = function.clone();
                    value["accessLevel"] = graph.access.get(&2).context("View access level not found")?.clone();
                    values.push(value);
                }
                Ok(json!(values))
            }
            SecurityQuery::AuthLevels | SecurityQuery::AccessLevels => {
                let levels = if matches!(query, SecurityQuery::AuthLevels) {
                    &graph.auth
                } else {
                    &graph.access
                };
                Ok(json!(levels.values().collect::<Vec<_>>()))
            }
            SecurityQuery::AuthorizedProfiles => {
                let id=sqlx::query_scalar("SELECT id FROM users WHERE email=$1 AND ident_master IS NULL ORDER BY id LIMIT 1")
                    .bind(email).fetch_one(&mut *conn).await?;
                graph.profiles(id)
            }
        }
    }
    pub(super) async fn security_mutate(
        &self,
        command: SecurityWrite,
        email: &str,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        let actor = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM users WHERE email=$1 AND ident_master IS NULL ORDER BY id LIMIT 1",
        )
        .bind(email)
        .fetch_one(&mut *tx)
        .await?;
        let result = match command {
            SecurityWrite::CreateRole(req) => save_role(&mut tx, req, Some(actor), true).await?,
            SecurityWrite::UpdateRole(req) => save_role(&mut tx, req, Some(actor), false).await?,
            SecurityWrite::DeleteRole(req) => {
                assert_mutable(&mut tx, req.ident_user_role, req.shortname.as_deref()).await?;
                sqlx::query("DELETE FROM userrole WHERE id=$1")
                    .bind(req.ident_user_role)
                    .execute(&mut *tx)
                    .await?;
                Value::Null
            }
            command => {
                let (mut req, create, deactivate) = match command {
                    SecurityWrite::CreateProfile(r) => (r, true, false),
                    SecurityWrite::UpdateProfile(r) => (r, false, false),
                    SecurityWrite::DeactivateProfile(r) => (r, false, true),
                    _ => unreachable!(),
                };
                if deactivate {
                    req.deactivation_date = Some(crate::util::app_now().date());
                }
                let id = save_profile(&mut tx, req, create).await?;
                profile(&mut tx, id).await?
            }
        };
        tx.commit().await?;
        Ok(result)
    }
}

pub(super) async fn grant_admin(conn: &mut PgConnection, user: i64) -> Result<()> {
    let role_id=match sqlx::query_scalar::<_,i64>("SELECT id FROM userrole WHERE shortname='AKN_SECURITY_ADMIN' AND internal ORDER BY id LIMIT 1")
        .fetch_optional(&mut *conn).await? {
        Some(id)=>id,None=>sqlx::query_scalar("INSERT INTO userrole(shortname,name,activation_status,internal,last_update) VALUES('AKN_SECURITY_ADMIN','Security administrator','Active',true,LOCALTIMESTAMP) RETURNING id")
            .fetch_one(&mut *conn).await?,
    };
    sqlx::query("INSERT INTO userrolefunction(role_id,function_id,accesslevel_id) SELECT $1,id,4 FROM security_function WHERE function_name='UserAccount' ON CONFLICT(role_id,function_id) DO NOTHING")
        .bind(role_id).execute(&mut *conn).await?;
    let profile_id=match sqlx::query_scalar::<_,i64>("SELECT pp.id FROM permissionprofile pp JOIN permissionprofile_userrole pr ON pr.profile_id=pp.id WHERE pp.user_id=$1 AND pr.role_id=$2 ORDER BY pp.id LIMIT 1")
        .bind(user).bind(role_id).fetch_optional(&mut *conn).await? {
        Some(id)=>id,None=>{
            let id=sqlx::query_scalar("INSERT INTO permissionprofile(name,user_id) SELECT 'AKN/'||upper(username),id FROM users WHERE id=$1 RETURNING id")
                .bind(user).fetch_one(&mut *conn).await?;
            sqlx::query("INSERT INTO permissionprofile_userrole VALUES($1,$2)").bind(id).bind(role_id).execute(&mut *conn).await?;
            id
        },
    };
    sqlx::query("INSERT INTO useraccountpermissionprofile(user_id,profile_id) SELECT id,$2 FROM users WHERE id=$1 OR id=(SELECT ident_revision FROM users WHERE id=$1) ON CONFLICT DO NOTHING")
        .bind(user).bind(profile_id).execute(conn).await?;
    Ok(())
}
