//! Request-scoped security graph. Each table is read once regardless of the
//! number of nested roles/functions; no cross-request permission cache.
use super::{json_shape, postgres_impl::raw_users};
use crate::security::{base, user_model};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
use std::collections::BTreeMap;

pub(super) struct SecurityGraph {
    users: BTreeMap<i64, Value>,
    pub auth: BTreeMap<i64, Value>,
    pub access: BTreeMap<i64, Value>,
    pub functions: BTreeMap<i64, Value>,
    pub roles: BTreeMap<i64, Value>,
    profiles: BTreeMap<i64, Value>,
    profile_roles: BTreeMap<i64, Vec<i64>>,
    assignments: BTreeMap<i64, Vec<i64>>,
}

impl SecurityGraph {
    pub async fn load(conn: &mut PgConnection) -> Result<Self> {
        let mut graph = Self {
            users: raw_users(conn)
                .await?
                .into_iter()
                .map(|v| (v["id"].as_i64().expect("database user id"), v))
                .collect(),
            auth: BTreeMap::new(),
            access: BTreeMap::new(),
            functions: BTreeMap::new(),
            roles: BTreeMap::new(),
            profiles: BTreeMap::new(),
            profile_roles: BTreeMap::new(),
            assignments: BTreeMap::new(),
        };
        for (table, is_access) in [("authlevel", false), ("accesslevel", true)] {
            for row in sqlx::query(&format!("SELECT id,name FROM {table} ORDER BY id"))
                .fetch_all(&mut *conn)
                .await?
            {
                let id: i64 = row.get(0);
                let mut v = base(id);
                if is_access {
                    v["identAccessLevel"] = json!(id);
                    v["accessLevelName"] = json!(row.get::<String, _>(1));
                    for (key, level) in [("view", 2), ("initiate", 3), ("approve", 4)] {
                        v[key] = json!(id == level);
                    }
                    graph.access.insert(id, v);
                } else {
                    v["identAuthLevel"] = json!(id);
                    v["authLevelName"] = json!(row.get::<String, _>(1));
                    graph.auth.insert(id, v);
                }
            }
        }
        for row in sqlx::query(
            "SELECT id,function_name,description,authlevel_id FROM security_function ORDER BY id",
        )
        .fetch_all(&mut *conn)
        .await?
        {
            let id: i64 = row.get(0);
            let mut v = base(id);
            v["identFunction"] = json!(id);
            v["functionName"] = json!(row.get::<String, _>(1));
            v["description"] = json!(row.get::<Option<String>, _>(2));
            v["category"] = Value::Null;
            v["authLevel"] = match row.get::<Option<i64>, _>(3) {
                Some(id) => graph.auth.get(&id).context("auth level not found")?.clone(),
                None => Value::Null,
            };
            graph.functions.insert(id, v);
        }
        for row in sqlx::query("SELECT id,shortname,name,description,activation_status,to_char(last_update,'YYYY-MM-DD\"T\"HH24:MI:SS.US'),applicable_for_poa,internal FROM userrole ORDER BY id")
            .fetch_all(&mut *conn).await? {
            let id: i64 = row.get(0);
            let mut v = base(id);
            v["identUserRole"] = json!(id);
            for (i, key) in [(1,"shortname"),(2,"name"),(3,"description"),(4,"activationStatus")] {
                v[key] = json!(row.get::<Option<String>,_>(i));
            }
            v["lastUpdate"] = json_shape::fmt_local_datetime(row.get::<Option<String>,_>(5).as_deref());
            v["applicableForPOA"] = json!(row.get::<bool,_>(6));
            v["internal"] = json!(row.get::<bool,_>(7));
            v["userRoleFunctions"] = json!([]);
            graph.roles.insert(id, v);
        }
        for row in sqlx::query(
            "SELECT role_id,function_id,accesslevel_id FROM userrolefunction ORDER BY id",
        )
        .fetch_all(&mut *conn)
        .await?
        {
            let mut v = base(0); // Java's composite UserRoleFunction key has no Base.ident.
            v["function"] = graph
                .functions
                .get(&row.get::<i64, _>(1))
                .context("function not found")?
                .clone();
            v["accessLevel"] = graph
                .access
                .get(&row.get::<i64, _>(2))
                .context("access level not found")?
                .clone();
            graph
                .roles
                .get_mut(&row.get::<i64, _>(0))
                .context("role not found")?["userRoleFunctions"]
                .as_array_mut()
                .expect("role functions array")
                .push(v);
        }
        for row in sqlx::query(
            "SELECT id,name,user_id,deactivation_date::text FROM permissionprofile ORDER BY id",
        )
        .fetch_all(&mut *conn)
        .await?
        {
            let id: i64 = row.get(0);
            let mut v = base(id);
            v["identPermissionProfile"] = json!(id);
            v["name"] = json!(row.get::<Option<String>, _>(1));
            v["deactivationDate"] = json!(row.get::<Option<String>, _>(3));
            // Resolve only profiles actually rendered, preserving legacy null
            // activation handling without failing on an unrelated user.
            v["userAccount"] = json!(row.get::<Option<i64>, _>(2));
            v["individual"] = json!(!v["userAccount"].is_null());
            graph.profiles.insert(id, v);
        }
        for row in sqlx::query(
            "SELECT profile_id,role_id FROM permissionprofile_userrole ORDER BY profile_id,role_id",
        )
        .fetch_all(&mut *conn)
        .await?
        {
            graph
                .profile_roles
                .entry(row.get(0))
                .or_default()
                .push(row.get(1));
        }
        for row in sqlx::query("SELECT user_id,profile_id FROM useraccountpermissionprofile ORDER BY user_id,profile_id")
            .fetch_all(conn).await? {
            graph.assignments.entry(row.get(0)).or_default().push(row.get(1));
        }
        Ok(graph)
    }

    fn user(&self, id: i64) -> Result<&Value> {
        let raw = self.users.get(&id).context("referenced user not found")?;
        let legacy_crash = std::env::var("LEGACY_NULL_ACTIVATION_CRASH")
            .map(|v| !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);
        ensure!(
            !(raw["activationType"].is_null() && legacy_crash),
            "null value was assigned to ident_activationtype (legacy behaviour: HTTP 500)"
        );
        Ok(raw)
    }
    pub fn role(&self, id: i64) -> Result<Value> {
        Ok(self.roles.get(&id).context("role not found")?.clone())
    }
    pub fn profile(&self, id: i64) -> Result<Value> {
        let mut v = self.profiles.get(&id).context("profile not found")?.clone();
        if let Some(id) = v["userAccount"].as_i64() {
            let mut owner = user_model(self.user(id)?, Value::Null, None);
            owner["lifecycleEntity"] = json!({"identEntity":0,"identMaster":null,"identRevision":null,
                "revisionEffectiveFrom":null,"revisionEffectiveTo":null,"requestedActivationDate":null,
                "activationType":null,"lastUpdateUser":null,"lastUpdateTime":null,"revisionState":null,"workflowStatus":null});
            v["userAccount"] = owner;
        }
        let roles = self
            .profile_roles
            .get(&id)
            .into_iter()
            .flatten()
            .map(|id| self.role(*id))
            .collect::<Result<Vec<_>>>()?;
        v["userRoles"] = json!(roles);
        Ok(v)
    }
    pub fn profiles(&self, user: i64) -> Result<Value> {
        Ok(json!(self
            .assignments
            .get(&user)
            .into_iter()
            .flatten()
            .map(|id| self.profile(*id))
            .collect::<Result<Vec<_>>>()?))
    }
    pub fn assemble_user(&self, raw: &Value) -> Result<Value> {
        let id = raw["id"].as_i64().context("missing user id")?;
        let actor = match raw["lastUpdateUserIdent"].as_i64() {
            Some(id) => {
                let mut v = user_model(self.user(id)?, Value::Null, None);
                for key in ["pwdHash", "password", "salt"] {
                    v[key] = Value::Null;
                }
                v["lifecycleEntity"] =
                    user_model(&json!({}), Value::Null, None)["lifecycleEntity"].clone();
                Some(v)
            }
            None => None,
        };
        Ok(user_model(raw, self.profiles(id)?, actor))
    }
    pub fn master_users(&self) -> Result<Vec<Value>> {
        self.users
            .iter()
            .filter(|(_, raw)| raw["identMaster"].is_null())
            .map(|(id, _)| self.assemble_user(self.user(*id)?))
            .collect()
    }
}
