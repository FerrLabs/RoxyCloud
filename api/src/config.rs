use std::env::{self, VarError};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum BlobBackend {
    Local { root: PathBuf },
    S3(S3Config),
}

#[derive(Debug, Clone)]
pub struct S3Config {
    pub bucket: String,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub prefix: String,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub database_url: String,
    pub blobs: BlobBackend,
    pub upload_root: PathBuf,
    pub web_root: Option<PathBuf>,
    pub jwt_secret: String,
    pub cors_allowed_origins: Vec<String>,
    pub default_quota_bytes: i64,
    pub session_ttl_seconds: i64,
    pub blob_sweep_interval_seconds: u64,
    pub blob_grace_period_seconds: u64,
    pub oidc: Option<OidcConfig>,
    pub bootstrap_admin: Option<BootstrapAdmin>,
}

#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    /// Whether a verified address nobody has an account for becomes one. A deployment that invites
    /// people through its provider wants this; one with a fixed roster does not.
    pub create_accounts: bool,
}

#[derive(Debug, Clone)]
pub struct BootstrapAdmin {
    pub email: String,
    pub password: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error("{name} is not valid: {reason}")]
    Invalid {
        name: &'static str,
        reason: &'static str,
    },
}

const DEFAULT_QUOTA_BYTES: i64 = 10 * 1024 * 1024 * 1024;
const DEFAULT_SESSION_TTL_SECONDS: i64 = 12 * 60 * 60;
const DEFAULT_BLOB_SWEEP_INTERVAL_SECONDS: u64 = 60 * 60;
const DEFAULT_BLOB_GRACE_PERIOD_SECONDS: u64 = 24 * 60 * 60;

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let backend = blobs()?;
        Ok(Self {
            port: parse_or("PORT", 3001)?,
            database_url: required("DATABASE_URL")?,
            blobs: backend.clone(),
            upload_root: upload_root(&backend)?,
            web_root: optional("WEB_ROOT").map(PathBuf::from),
            jwt_secret: required("JWT_SECRET")?,
            cors_allowed_origins: optional("CORS_ALLOWED_ORIGINS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            default_quota_bytes: parse_or("DEFAULT_QUOTA_BYTES", DEFAULT_QUOTA_BYTES)?,
            session_ttl_seconds: parse_or("SESSION_TTL_SECONDS", DEFAULT_SESSION_TTL_SECONDS)?,
            blob_sweep_interval_seconds: parse_or(
                "BLOB_SWEEP_INTERVAL_SECONDS",
                DEFAULT_BLOB_SWEEP_INTERVAL_SECONDS,
            )?,
            blob_grace_period_seconds: parse_or(
                "BLOB_GRACE_PERIOD_SECONDS",
                DEFAULT_BLOB_GRACE_PERIOD_SECONDS,
            )?,
            oidc: oidc(),
            bootstrap_admin: bootstrap_admin(),
        })
    }
}

/// Beside the blobs by default, because that is the one directory a deployment has already had to
/// make writable. An object store deployment has no such directory, so it has to say where.
fn upload_root(blobs: &BlobBackend) -> Result<PathBuf, ConfigError> {
    if let Some(configured) = optional("UPLOAD_ROOT") {
        return Ok(PathBuf::from(configured));
    }
    match blobs {
        BlobBackend::Local { root } => Ok(root.join("uploads")),
        // Falling back to a relative path would stage uploads on the container's ephemeral disk,
        // which is the one place this design says the bytes must not live, and the deployment
        // would find out when the disk filled rather than at startup.
        BlobBackend::S3(_) => Err(ConfigError::Missing("UPLOAD_ROOT")),
    }
}

/// Local disk unless `BLOB_BACKEND=s3`. An unknown value is refused rather than quietly falling
/// back, because a deployment that meant S3 and got local disk loses every upload when the pod
/// restarts.
fn blobs() -> Result<BlobBackend, ConfigError> {
    match optional("BLOB_BACKEND").as_deref() {
        None | Some("local") => Ok(BlobBackend::Local {
            root: PathBuf::from(optional("BLOB_ROOT").unwrap_or_else(|| "./data".to_owned())),
        }),
        Some("s3") => Ok(BlobBackend::S3(S3Config {
            bucket: required("S3_BUCKET")?,
            region: optional("S3_REGION"),
            endpoint: optional("S3_ENDPOINT"),
            prefix: optional("S3_PREFIX").unwrap_or_default(),
            access_key_id: optional("S3_ACCESS_KEY_ID"),
            secret_access_key: optional("S3_SECRET_ACCESS_KEY"),
        })),
        Some(_) => Err(ConfigError::Invalid {
            name: "BLOB_BACKEND",
            reason: "expected local or s3",
        }),
    }
}

fn oidc() -> Option<OidcConfig> {
    Some(OidcConfig {
        issuer: optional("OIDC_ISSUER")?,
        client_id: optional("OIDC_CLIENT_ID")?,
        client_secret: optional("OIDC_CLIENT_SECRET")?,
        redirect_url: optional("OIDC_REDIRECT_URL")?,
        create_accounts: optional("OIDC_CREATE_ACCOUNTS")
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes")),
    })
}

fn bootstrap_admin() -> Option<BootstrapAdmin> {
    Some(BootstrapAdmin {
        email: optional("BOOTSTRAP_ADMIN_EMAIL")?,
        password: optional("BOOTSTRAP_ADMIN_PASSWORD")?,
    })
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        Ok(_) | Err(VarError::NotPresent) => Err(ConfigError::Missing(name)),
        Err(VarError::NotUnicode(_)) => Err(ConfigError::Invalid {
            name,
            reason: "not valid unicode",
        }),
    }
}

fn optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn parse_or<T: std::str::FromStr>(name: &'static str, fallback: T) -> Result<T, ConfigError> {
    match optional(name) {
        None => Ok(fallback),
        Some(raw) => raw.parse().map_err(|_| ConfigError::Invalid {
            name,
            reason: "not a number",
        }),
    }
}
