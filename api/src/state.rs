use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::auth::Sessions;
use crate::config::Config;
use crate::config::{BlobBackend, S3Config};
use crate::storage::{BlobStore, LocalBlobStore, S3BlobStore};

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub blobs: Arc<dyn BlobStore>,
    pub sessions: Arc<Sessions>,
    pub staging: Arc<crate::uploads::Staging>,
    pub default_quota_bytes: i64,
}

async fn open_blobs(backend: &BlobBackend) -> Result<Arc<dyn BlobStore>> {
    match backend {
        BlobBackend::Local { root } => {
            let store = LocalBlobStore::open(root)
                .await
                .with_context(|| format!("opening blob store at {}", root.display()))?;
            Ok(Arc::new(store))
        }
        BlobBackend::S3(s3) => Ok(Arc::new(open_s3(s3).await)),
    }
}

/// Explicit credentials when they are configured, otherwise whatever the environment provides, so
/// a pod with a role attached needs no keys in its config. Path style addressing because `MinIO` and
/// Garage are addressed that way, and AWS accepts it too.
async fn open_s3(cfg: &S3Config) -> S3BlobStore {
    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};

    let mut loader = aws_config::defaults(BehaviorVersion::latest());
    if let Some(region) = &cfg.region {
        loader = loader.region(Region::new(region.clone()));
    }
    if let (Some(key), Some(secret)) = (&cfg.access_key_id, &cfg.secret_access_key) {
        loader =
            loader.credentials_provider(Credentials::new(key, secret, None, None, "roxycloud"));
    }

    let mut builder = aws_sdk_s3::config::Builder::from(&loader.load().await);
    if let Some(endpoint) = &cfg.endpoint {
        builder = builder.endpoint_url(endpoint).force_path_style(true);
    }

    S3BlobStore::new(
        aws_sdk_s3::Client::from_conf(builder.build()),
        cfg.bucket.clone(),
        &cfg.prefix,
    )
}

impl AppState {
    pub async fn from_config(cfg: &Config) -> Result<Self> {
        let db = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&cfg.database_url)
            .await
            .context("connecting to Postgres")?;

        let blobs = open_blobs(&cfg.blobs).await?;
        let staging = crate::uploads::Staging::open(&cfg.upload_root)
            .await
            .with_context(|| {
                format!(
                    "opening the upload staging at {}",
                    cfg.upload_root.display()
                )
            })?;

        Ok(Self {
            db,
            blobs,
            staging: Arc::new(staging),
            sessions: Arc::new(Sessions::new(
                &cfg.jwt_secret,
                chrono::Duration::seconds(cfg.session_ttl_seconds),
            )),
            default_quota_bytes: cfg.default_quota_bytes,
        })
    }
}
