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

        if written_within(&state.blobs.path_for(hash), grace).await {
            tx.commit().await?;
            continue;
        }

        state.blobs.remove(hash).await?;
        tx.commit().await?;

        collected.blobs += 1;
        collected.bytes += size;
    }

    Ok(collected)
}

async fn orphaned(pool: &PgPool, grace: Duration) -> Result<Vec<BlobHash>, ApiError> {
    sqlx::query_scalar::<_, BlobHash>(
        "SELECT hash FROM blobs
         WHERE ref_count = 0 AND unreferenced_since < now() - $1::INTERVAL",
    )
    .bind(interval_of(grace))
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

async fn written_within(path: &std::path::Path, grace: Duration) -> bool {
    tokio::fs::metadata(path)
        .await
        .and_then(|meta| meta.modified())
        .is_ok_and(|at| at.elapsed().is_ok_and(|since| since < grace))
}

fn interval_of(grace: Duration) -> sqlx::postgres::types::PgInterval {
    sqlx::postgres::types::PgInterval {
        months: 0,
        days: 0,
        microseconds: i64::try_from(grace.as_micros()).unwrap_or(i64::MAX),
    }
}
