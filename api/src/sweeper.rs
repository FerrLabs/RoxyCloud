use std::time::Duration;

use sqlx::PgPool;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{error, info};

use crate::error::ApiError;
use crate::state::AppState;
use roxycloud_core::blob::BlobHash;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Collected {
    pub blobs: u64,
    pub bytes: i64,
}

pub fn spawn(state: AppState, every: Duration, grace: Duration) {
    tokio::spawn(async move {
        let mut ticks = interval(every);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            ticks.tick().await;
            match crate::dav::locks::purge_expired(&state.db).await {
                Ok(expired) if expired > 0 => info!(locks = expired, "removed expired locks"),
                Ok(_) => {}
                Err(error) => error!(%error, "purging expired locks failed"),
            }
            match crate::attempts::purge_expired(&state.db).await {
                Ok(expired) if expired > 0 => info!(attempts = expired, "removed expired attempts"),
                Ok(_) => {}
                Err(error) => error!(%error, "purging expired attempts failed"),
            }
            match sweep_uploads(&state).await {
                Ok(cleared) if cleared > 0 => info!(uploads = cleared, "dropped expired uploads"),
                Ok(_) => {}
                Err(error) => error!(%error, "dropping expired uploads failed"),
            }
            match state.blobs.sweep_staged(grace).await {
                Ok(cleared) if cleared > 0 => info!(staged = cleared, "cleared stale uploads"),
                Ok(_) => {}
                Err(error) => error!(%error, "clearing stale uploads failed"),
            }
            match sweep(&state, grace).await {
                Ok(collected) if collected.blobs > 0 => {
                    info!(
                        blobs = collected.blobs,
                        bytes = collected.bytes,
                        "collected orphaned blobs"
                    );
                }
                Ok(_) => {}
                Err(error) => error!(%error, "sweeping orphaned blobs failed"),
            }
        }
    });
}

/// Expired sessions go, and then the staging directory is reconciled against the sessions that
/// remain. Deleting the row and deleting the bytes cannot be made one operation, so the second is
/// driven by what the first left rather than by a list carried between them.
async fn sweep_uploads(state: &AppState) -> Result<u64, ApiError> {
    let expired = crate::uploads::purge_expired(&state.db).await?.len();
    let live = crate::uploads::live_staged(&state.db).await?;
    state.staging.sweep(&live).await?;
    Ok(expired as u64)
}

pub async fn sweep(state: &AppState, grace: Duration) -> Result<Collected, ApiError> {
    let mut collected = Collected::default();

    for hash in orphaned(&state.db, grace).await? {
        let mut tx = state.db.begin().await?;
        let claimed = sqlx::query_scalar::<_, i64>(
            "DELETE FROM blobs
             WHERE hash = $1 AND ref_count = 0 AND unreferenced_since < now() - $2::INTERVAL
             RETURNING size",
        )
        .bind(hash)
        .bind(interval_of(grace))
        .fetch_optional(&mut *tx)
        .await?;

        let Some(size) = claimed else {
            tx.rollback().await?;
            continue;
        };

        if state.blobs.written_within(hash, grace).await {
            tx.commit().await?;
            continue;
        }

        if let Err(error) = state.blobs.remove(hash).await {
            error!(%error, %hash, "could not remove the bytes, keeping the row");
            tx.rollback().await?;
            continue;
        }
        tx.commit().await?;

        collected.blobs += 1;
        collected.bytes += size;
    }

    Ok(collected)
}

async fn orphaned(pool: &PgPool, grace: Duration) -> Result<Vec<BlobHash>, ApiError> {
    sqlx::query_scalar::<_, BlobHash>(
        "SELECT hash FROM blobs
         WHERE ref_count = 0 AND unreferenced_since < now() - $1::INTERVAL
         LIMIT 10000",
    )
    .bind(interval_of(grace))
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

fn interval_of(grace: Duration) -> sqlx::postgres::types::PgInterval {
    sqlx::postgres::types::PgInterval {
        months: 0,
        days: 0,
        microseconds: i64::try_from(grace.as_micros()).unwrap_or(i64::MAX),
    }
}
