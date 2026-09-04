use anyhow::{Context, Result};
use std::time::Duration;

use roxycloud_api::{build_router, config::Config, state::AppState, sweeper, users};
use roxycloud_core::role::Role;
use roxycloud_core::user::Email;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cfg = Config::from_env().context("loading configuration")?;
    let state = AppState::from_config(&cfg).await?;

    sqlx::migrate!("./migrations")
        .run(&state.db)
        .await
        .context("running database migrations")?;

    bootstrap_admin(&state, &cfg).await?;

    if cfg.blob_sweep_interval_seconds > 0 {
        sweeper::spawn(
            state.clone(),
            Duration::from_secs(cfg.blob_sweep_interval_seconds),
            Duration::from_secs(cfg.blob_grace_period_seconds),
        );
        info!(
            every = cfg.blob_sweep_interval_seconds,
            grace = cfg.blob_grace_period_seconds,
            "collecting orphaned blobs in the background"
        );
    }

    let app = build_router(state, &cfg.cors_allowed_origins, cfg.web_root.as_deref());
    let bind = format!("0.0.0.0:{}", cfg.port);
    info!(%bind, "starting RoxyCloud API");

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    axum::serve(listener, app).await.context("serving")?;
    Ok(())
}

async fn bootstrap_admin(state: &AppState, cfg: &Config) -> Result<()> {
    let Some(admin) = &cfg.bootstrap_admin else {
        if users::count(&state.db).await? == 0 {
            warn!(
                "no accounts exist; set BOOTSTRAP_ADMIN_EMAIL and BOOTSTRAP_ADMIN_PASSWORD to create the first one"
            );
        }
        return Ok(());
    };

    if users::count(&state.db).await? > 0 {
        info!("accounts already exist, skipping bootstrap");
        return Ok(());
    }

    let email: Email = admin.email.parse().context("BOOTSTRAP_ADMIN_EMAIL")?;
    let mut tx = state.db.begin().await?;
    let user = users::create(
        &mut tx,
        &email,
        "Administrator",
        &admin.password,
        Role::Admin,
    )
    .await
    .context("creating the bootstrap administrator")?;
    tx.commit().await?;

    info!(%email, id = %user.id, "bootstrap administrator created");
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false).json())
        .init();
}
