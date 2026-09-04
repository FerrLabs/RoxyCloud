use std::env::{self, VarError};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub database_url: String,
    pub blob_root: PathBuf,
    pub web_root: Option<PathBuf>,
    pub jwt_secret: String,
    pub cors_allowed_origins: Vec<String>,
    pub default_quota_bytes: i64,
    pub session_ttl_seconds: i64,
    pub blob_sweep_interval_seconds: u64,
    pub blob_grace_period_seconds: u64,
    pub bootstrap_admin: Option<BootstrapAdmin>,
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
        Ok(Self {
            port: parse_or("PORT", 3001)?,
            database_url: required("DATABASE_URL")?,
            blob_root: PathBuf::from(optional("BLOB_ROOT").unwrap_or_else(|| "./data".to_owned())),
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
            bootstrap_admin: bootstrap_admin(),
        })
    }
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
