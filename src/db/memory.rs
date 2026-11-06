//! Implementasi in-memory Repository — untuk pengembangan & golden test kontrak HTTP
//! tanpa DB Oracle. Format respons berbagi json_shape dengan Oracle, tetapi
//! lifecycle, relasi, otorisasi dan update belum sepenuhnya setara. Backend ini
//! bukan pengganti acceptance test Oracle (lihat docs/VALIDASI-2026-10-18.md).

use crate::db::json_shape::{self, Lifecycle};
use crate::db::Repository;
use crate::models::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send + 'a>>;

#[derive(Default)]
pub struct MemoryRepo {
    users: Mutex<Vec<Value>>,
    akun: Mutex<Vec<Value>>,
    categories: Mutex<Vec<Value>>,
    transaksi: Mutex<Vec<Value>>,
    event_entries: Mutex<Vec<Value>>,
    props: Mutex<HashMap<String, String>>,
    seq: Mutex<Seq>,
}

#[derive(Default)]
struct Seq {
    user: i64,
    akun: i64,
    category: i64,
    transaksi: i64,
}

/// Waktu aplikasi (UTC secara default — lihat util::app_now)
fn now_local() -> String {
    crate::util::app_now_string()
}

impl MemoryRepo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed user dengan bentuk JSON yang sama seperti sistem lama.
    pub fn seed_user(&self, id: i64, email: &str, password: &str, name: Option<&str>) {
        let salt = crate::auth::new_password_salt();
        let pwd_hash = crate::auth::hash_password_sha256(&salt, password);
        let sha1 = crate::auth::hash_password_sha1(password);
        let lc = Lifecycle {
            ident_master: None,
            ident_revision: Some(id + 10),
            revision_effective_from: Some(now_local()),
            activation_type: Some((1, "Active".into())),
            ident_activation_type: 1,
            last_update_user_ident: Some(id),
            last_update_time: Some(now_local()),
            revision_state: Some((1, "Ready".into())),
            ..Default::default()
        };
        let user = json_shape::user_json(
            id,
            &lc,
            name,
            Some(email),
            Some(email),
            Some(&pwd_hash),
            Some(&sha1),
            Some(&salt),
            None,
            None,
        );
        self.users.lock().unwrap().push(user);
    }

    pub fn set_prop(&self, name: &str, value: &str) {
        self.props
            .lock()
            .unwrap()
            .insert(name.to_string(), value.to_string());
    }

    pub fn seed_akun(&self, id: i64, name: &str, id_user: i64) {
        let akun = json_shape::akun_json(
            id,
            &Lifecycle::active_ready(id_user),
            Some(name),
            None,
            Some(id_user),
            Some("0"),
            Some("AKN01AAAAAA00100"),
            None,
            None,
        );
        self.akun.lock().unwrap().push(akun);
    }
}

impl Repository for MemoryRepo {
    fn get_system_prop_bool<'a>(&'a self, name: &'a str) -> BoxFut<'a, bool> {
        Box::pin(async move {
            Ok(self
                .props
                .lock()
                .unwrap()
                .get(name)
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false))
        })
    }

    fn authenticate_user_by_email<'a>(&'a self, email: &'a str, password: &'a str) -> BoxFut<'a, bool> {
        Box::pin(async move {
            let users = self.users.lock().unwrap();
            Ok(users
                .iter()
                .find(|u| u["email"].as_str() == Some(email) && u["identMaster"].is_null())
                .and_then(|u| u["password"].as_str())
                .map(|stored| crate::auth::authenticate_user_sha1(password, stored))
                .unwrap_or(false))
        })
    }

    fn authenticate_user_v2<'a>(&'a self, email: &'a str, password: &'a str) -> BoxFut<'a, bool> {
        Box::pin(async move {
            let users = self.users.lock().unwrap();
            Ok(users
                .iter()
                .find(|u| u["email"].as_str() == Some(email))
                .and_then(|u| Some((u["pwdHash"].as_str()?, u["salt"].as_str()?)))
                .map(|(hash, salt)| crate::auth::hash_password_sha256(salt, password) == hash)
                .unwrap_or(false))
        })
    }

    fn fetch_all_users(&self) -> BoxFut<'_, Vec<Value>> {
        Box::pin(async move { Ok(self.users.lock().unwrap().clone()) })
    }

    fn fetch_user_by_id<'a>(&'a self, id: i64) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move {
            Ok(self
                .users
                .lock()
                .unwrap()
                .iter()
                .find(|u| u["id"].as_i64() == Some(id))
                .cloned())
        })
    }

    fn create_user<'a>(&'a self, req: UsersRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let salt = crate::auth::new_password_salt();
            let pwd_hash = req
                .password
                .as_deref()
                .map(|p| crate::auth::hash_password_sha256(&salt, p));
            let sha1 = req.password.as_deref().map(crate::auth::hash_password_sha1);
            let id = {
                let mut seq = self.seq.lock().unwrap();
                seq.user += 1;
                seq.user
            };
            let lc = Lifecycle {
                ident_revision: Some(id + 10),
                revision_effective_from: Some(now_local()),
                last_update_time: Some(now_local()),
                ..Lifecycle::active_ready(id)
            };
            let user = json_shape::user_json(
                id,
                &lc,
                req.name.as_deref(),
                req.email.as_deref(),
                req.username.as_deref(),
                pwd_hash.as_deref(),
                sha1.as_deref(),
                Some(&salt),
                Some(&now_local()),
                None,
            );
            self.users.lock().unwrap().push(user.clone());
            Ok(user)
        })
    }

    fn update_user<'a>(&'a self, req: UsersRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut users = self.users.lock().unwrap();
            let user = users
                .iter_mut()
                .find(|u| u["id"].as_i64() == Some(req.ident))
                .ok_or_else(|| anyhow::anyhow!("user not found"))?;
            if let Some(v) = req.name {
                user["name"] = json!(v);
            }
            if let Some(v) = req.email {
                user["email"] = json!(v);
            }
            if let Some(v) = req.username {
                user["username"] = json!(v);
            }
            user["updatedDate"] = json_shape::fmt_timestamp(Some(&now_local()));
            Ok(user.clone())
        })
    }

    fn change_password<'a>(&'a self, ident_user: i64, new_password: &'a str) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let salt = crate::auth::new_password_salt();
            let pwd_hash = crate::auth::hash_password_sha256(&salt, new_password);
            let sha1 = crate::auth::hash_password_sha1(new_password);
            let mut users = self.users.lock().unwrap();
            let user = users
                .iter_mut()
                .find(|u| u["id"].as_i64() == Some(ident_user))
                .ok_or_else(|| anyhow::anyhow!("user not found"))?;
            user["pwdHash"] = json!(pwd_hash);
            user["salt"] = json!(salt);
            user["password"] = json!(sha1);
            user["updatedDate"] = json_shape::fmt_timestamp(Some(&now_local()));
            Ok(user.clone())
        })
    }

    fn delete_user<'a>(&'a self, id: i64) -> BoxFut<'a, bool> {
        Box::pin(async move {
            let mut users = self.users.lock().unwrap();
            let before = users.len();
            users.retain(|u| u["id"].as_i64() != Some(id));
            Ok(users.len() < before)
        })
    }

    fn fetch_akun_keuangan_by_email<'a>(&'a self, email: &'a str) -> BoxFut<'a, Vec<Value>> {
        Box::pin(async move {
            let user_ids: Vec<i64> = {
                let users = self.users.lock().unwrap();
                users
                    .iter()
                    .filter(|u| u["email"].as_str() == Some(email))
                    .filter_map(|u| u["id"].as_i64())
                    .collect()
            };
            let akun = self.akun.lock().unwrap();
            Ok(akun
                .iter()
                .filter(|a| {
                    a["idUser"]
                        .as_i64()
                        .map(|i| user_ids.contains(&i))
                        .unwrap_or(false)
                        && a["identMaster"].is_null()
                })
                .cloned()
                .collect())
        })
    }

    fn fetch_akun_by_id<'a>(&'a self, id: i64) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move {
            Ok(self
                .akun
                .lock()
                .unwrap()
                .iter()
                .find(|a| a["id"].as_i64() == Some(id))
                .cloned())
        })
    }

    fn create_akun_keuangan<'a>(&'a self, req: AkunKeuanganRequest, email: &'a str) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let account_number =
                crate::util::create_account_number(req.account_type).map_err(|e| anyhow::anyhow!(e))?;
            let id_user = {
                let users = self.users.lock().unwrap();
                users
                    .iter()
                    .find(|u| u["email"].as_str() == Some(email))
                    .and_then(|u| u["id"].as_i64())
            };
            let id = {
                let mut seq = self.seq.lock().unwrap();
                seq.akun += 1;
                seq.akun
            };
            let lc = Lifecycle {
                ident_revision: Some(id + 1),
                revision_effective_from: Some(now_local()),
                last_update_time: Some(now_local()),
                ..Lifecycle::active_ready(id_user.unwrap_or(0))
            };
            // Konverter lama SELALU menimpa saldo jadi 0
            let akun = json_shape::akun_json(
                id,
                &lc,
                req.name.as_deref(),
                req.description.as_deref(),
                id_user,
                Some("0"),
                Some(&account_number),
                None,
                None,
            );
            self.akun.lock().unwrap().push(akun.clone());
            Ok(akun)
        })
    }

    fn update_akun_keuangan<'a>(&'a self, req: AkunKeuanganRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut akun = self.akun.lock().unwrap();
            let a = akun
                .iter_mut()
                .find(|a| a["id"].as_i64() == Some(req.ident))
                .ok_or_else(|| anyhow::anyhow!("akun not found"))?;
            if let Some(v) = req.name {
                a["name"] = json!(v);
            }
            if let Some(v) = req.description {
                a["description"] = json!(v);
            }
            if let Some(v) = req.id_user {
                a["idUser"] = json!(v);
            }
            if let Some(v) = req.saldo {
                a["saldo"] = json_shape::fmt_decimal(Some(&v.to_string()));
            }
            Ok(a.clone())
        })
    }

    fn fetch_categories<'a>(&'a self, id_akun: i64, email: &'a str) -> BoxFut<'a, Vec<Value>> {
        Box::pin(async move {
            let valid = self
                .akun
                .lock()
                .unwrap()
                .iter()
                .any(|a| a["id"].as_i64() == Some(id_akun));
            let email_ok = {
                let users = self.users.lock().unwrap();
                users.iter().any(|u| u["email"].as_str() == Some(email))
            };
            if !valid || !email_ok {
                return Ok(vec![]);
            }
            let cats = self.categories.lock().unwrap();
            Ok(cats
                .iter()
                .filter(|c| c["akunKeuangan"]["id"].as_i64() == Some(id_akun))
                .cloned()
                .collect())
        })
    }

    fn fetch_category_detail<'a>(&'a self, id_akun: i64, id_c: i64, email: &'a str) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move {
            Ok(self
                .fetch_categories(id_akun, email)
                .await?
                .into_iter()
                .find(|c| c["id"].as_i64() == Some(id_c)))
        })
    }

    fn create_category<'a>(&'a self, cat: CategoryRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let id = {
                let mut seq = self.seq.lock().unwrap();
                seq.category += 1;
                seq.category
            };
            let akun_id = cat
                .akun_keuangan
                .as_ref()
                .and_then(|v| v.get("id"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let c = json_shape::category_json(
                id,
                cat.category_name.as_deref(),
                cat.category_type.as_ref().map(|d| d.to_string()).as_deref(),
                Some(json_shape::akun_stub(akun_id)),
                Some(&now_local()),
                None,
            );
            self.categories.lock().unwrap().push(c.clone());
            Ok(c)
        })
    }

    fn update_category<'a>(&'a self, cat: CategoryRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut cats = self.categories.lock().unwrap();
            let c = cats
                .iter_mut()
                .find(|c| c["id"].as_i64() == Some(cat.id))
                .ok_or_else(|| anyhow::anyhow!("category not found"))?;
            if let Some(v) = cat.category_name {
                c["category_name"] = json!(v);
            }
            if let Some(t) = cat.category_type {
                c["category_type"] = json_shape::fmt_decimal(Some(&t.to_string()));
            }
            c["updatedDate"] = json_shape::fmt_timestamp(Some(&now_local()));
            Ok(c.clone())
        })
    }

    fn fetch_transaksi_by_akun<'a>(&'a self, id_akun: i64, email: &'a str) -> BoxFut<'a, Vec<Value>> {
        Box::pin(async move {
            let valid = self
                .akun
                .lock()
                .unwrap()
                .iter()
                .any(|a| a["id"].as_i64() == Some(id_akun));
            let email_ok = {
                let users = self.users.lock().unwrap();
                users.iter().any(|u| u["email"].as_str() == Some(email))
            };
            if !valid || !email_ok {
                return Ok(vec![]);
            }
            let tk = self.transaksi.lock().unwrap();
            Ok(tk
                .iter()
                .filter(|t| t["idAkunKeuangan"].as_i64() == Some(id_akun))
                .cloned()
                .collect())
        })
    }

    fn fetch_transaksi_detail<'a>(&'a self, id_akun: i64, id_tk: i64, email: &'a str) -> BoxFut<'a, Option<Value>> {
        Box::pin(async move {
            Ok(self
                .fetch_transaksi_by_akun(id_akun, email)
                .await?
                .into_iter()
                .find(|t| t["id"].as_i64() == Some(id_tk)))
        })
    }

    fn create_transaksi<'a>(&'a self, tk_req: TransaksiKeuanganRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let internal_ref = crate::util::generate_internal_reference(crate::util::app_now());
            let id = {
                let mut seq = self.seq.lock().unwrap();
                seq.transaksi += 1;
                seq.transaksi
            };
            let t = json_shape::transaksi_json(
                id,
                tk_req.name.as_deref(),
                tk_req.description.as_deref(),
                tk_req.id_akun_keuangan,
                None,
                tk_req.jenis.as_ref().map(|d| d.to_string()).as_deref(),
                tk_req.nominal.as_ref().map(|d| d.to_string()).as_deref(),
                Some(&now_local()),
                None,
                tk_req.transaction_date.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()).as_deref(),
                tk_req.externalreference.as_deref(),
                Some(&internal_ref),
            );
            self.transaksi.lock().unwrap().push(t.clone());
            Ok(t)
        })
    }

    fn update_transaksi<'a>(&'a self, tk_req: TransaksiKeuanganRequest) -> BoxFut<'a, Value> {
        Box::pin(async move {
            let mut tk = self.transaksi.lock().unwrap();
            let t = tk
                .iter_mut()
                .find(|t| t["id"].as_i64() == Some(tk_req.id))
                .ok_or_else(|| anyhow::anyhow!("transaksi not found"))?;
            if let Some(v) = tk_req.name {
                t["name"] = json!(v);
            }
            if let Some(v) = tk_req.description {
                t["description"] = json!(v);
            }
            if let Some(j) = tk_req.jenis {
                t["jenis"] = json_shape::fmt_decimal(Some(&j.to_string()));
            }
            if let Some(n) = tk_req.nominal {
                t["nominal"] = json_shape::fmt_decimal(Some(&n.to_string()));
            }
            if let Some(v) = tk_req.externalreference {
                t["externalreference"] = json!(v);
            }
            t["updatedDate"] = json_shape::fmt_timestamp(Some(&now_local()));
            Ok(t.clone())
        })
    }

    fn fetch_transaksi_not_in_settlement_engine(&self) -> BoxFut<'_, Vec<Value>> {
        Box::pin(async move { Ok(vec![]) })
    }

    fn fetch_event_entries<'a>(&'a self, event_id: &'a str) -> BoxFut<'a, Vec<Value>> {
        Box::pin(async move {
            Ok(self
                .event_entries
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e["eventId"].as_str() == Some(event_id))
                .cloned()
                .collect())
        })
    }
}
