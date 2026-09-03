use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::auth::Sessions;
use crate::config::Config;
use crate::storage::LocalBlobStore;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub blobs: Arc<LocalBlobStore>,
    pub sessions: Arc<Sessions>,
    pub default_quota_bytes: i64,
}

impl AppState {
    pub async fn from_config(cfg: &Config) -> Result<Self> {
        let db = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&cfg.database_url)
            .await
            .context("connecting to Postgres")?;

        let blobs = LocalBlobStore::open(&cfg.blob_root)
            .await
            .with_context(|| format!("opening blob store at {}", cfg.blob_root.display()))?;

        Ok(Self {
            db,
            blobs: Arc::new(blobs),
            sessions: Arc::new(Sessions::new(
                &cfg.jwt_secret,
                chrono::Duration::seconds(cfg.session_ttl_seconds),
            )),
            default_quota_bytes: cfg.default_quota_bytes,
        })
    }
}
