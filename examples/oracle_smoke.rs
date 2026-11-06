//! Optional legacy connection probe. Supply every connection value explicitly.
use akunkeuangan_service::db::{oracle_impl::OracleRepo, Repository};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let connect = std::env::var("ORACLE_CONNECT")?;
    let user = std::env::var("ORACLE_USER")?;
    let password = std::env::var("ORACLE_PASSWORD")?;
    let repo = OracleRepo::connect(&connect, &user, &password, &connect, &user, &password)?;
    repo.get_system_prop_bool("is.test").await?;
    println!("Oracle connection ready");
    Ok(())
}
