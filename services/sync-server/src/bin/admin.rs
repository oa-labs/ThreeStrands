use anyhow::{bail, Context};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let [action, email] = arguments.as_slice() else {
        bail!("usage: admin <grant-sync|revoke-sync> <verified-google-email>");
    };
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let pool = sqlx::PgPool::connect(&database_url).await?;
    let user_id = sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM users WHERE lower(email)=lower($1)")
        .bind(email).fetch_optional(&pool).await?.context("account not found")?;
    match action.as_str() {
        "grant-sync" => {
            sqlx::query("INSERT INTO entitlements(user_id,feature,source) VALUES($1,'sync','beta') ON CONFLICT(user_id,feature) DO UPDATE SET source='beta',expires_at=NULL")
                .bind(user_id).execute(&pool).await?;
            println!("granted sync beta entitlement to {email}");
        }
        "revoke-sync" => {
            sqlx::query("DELETE FROM entitlements WHERE user_id=$1 AND feature='sync'")
                .bind(user_id).execute(&pool).await?;
            println!("revoked sync entitlement from {email}");
        }
        _ => bail!("unknown action: {action}"),
    }
    Ok(())
}
