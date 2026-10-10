use sqlx::{PgPool, Postgres, Transaction};
use stashden_core::blob::BlobHash;
use stashden_core::name::{MAX_NAME_LEN, NodeName};
use stashden_core::user::User;
use uuid::Uuid;

use crate::error::ApiError;
use crate::{db, trash, users, versions};

const MAX_ATTEMPTS: u32 = 1000;

pub async fn remove(
    pool: &PgPool,
    id: Uuid,
    hand_over_to: Option<Uuid>,
    default_quota_bytes: i64,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;

    let mut locked = vec![id];
    locked.extend(hand_over_to);
    locked.sort();
    for owner in locked {
        db::lock_owner(&mut tx, owner).await?;
    }

    let leaving = users::by_id_in(&mut tx, id).await?;
    trash::empty(&mut tx, id).await?;

    if let Some(recipient) = hand_over_to {
        let recipient = users::by_id_in(&mut tx, recipient).await?;
        if !recipient.is_active() || !recipient.may_write() {
            return Err(ApiError::WrongKind {
                expected: "active account that can write to hand the files to",
            });
        }
        hand_over(&mut tx, &leaving, &recipient, default_quota_bytes).await?;
    }

    let held = held_blobs(&mut tx, id).await?;
    sqlx::query("DELETE FROM grants WHERE grantee_email = $1")
        .bind(leaving.email.as_str())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for hash in held {
        db::release_blob(&mut tx, hash).await?;
    }

    tx.commit().await?;
    Ok(())
}

async fn hand_over(
    tx: &mut Transaction<'_, Postgres>,
    leaving: &User,
    recipient: &User,
    default_quota_bytes: i64,
) -> Result<(), ApiError> {
    let Some(old_root) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM nodes WHERE owner_id = $1 AND parent_id IS NULL",
    )
    .bind(leaving.id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(());
    };

    let root = db::ensure_root(tx, recipient.id, default_quota_bytes).await?;
    let name = free_name(tx, &root.id, &folder_name(leaving)).await?;
    let folder =
        db::create_directories(tx, recipient.id, &root, std::slice::from_ref(&name)).await?;

    sqlx::query("UPDATE nodes SET parent_id = $2 WHERE parent_id = $1")
        .bind(old_root)
        .bind(folder.id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE nodes SET owner_id = $2 WHERE owner_id = $1 AND id <> $3")
        .bind(leaving.id)
        .bind(recipient.id)
        .bind(old_root)
        .execute(&mut **tx)
        .await?;

    let used = sqlx::query_scalar::<_, i64>("SELECT bytes_used FROM quotas WHERE owner_id = $1")
        .bind(leaving.id)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(0);
    db::charge_quota(tx, recipient.id, used).await
}

const NUMBERED_SUFFIX: &str = " 1000";
const MAX_FOLDER_NAME_BYTES: usize = MAX_NAME_LEN - NUMBERED_SUFFIX.len();

fn folder_name(leaving: &User) -> String {
    fitted(&format!(
        "{} ({})",
        leaving.display_name.trim(),
        leaving.email
    ))
}

fn fitted(who: &str) -> String {
    let cleaned: String = who
        .chars()
        .map(|character| {
            if character == '/' || character == '\\' || character.is_control() {
                '-'
            } else {
                character
            }
        })
        .collect();
    let mut end = cleaned.len().min(MAX_FOLDER_NAME_BYTES);
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    cleaned[..end].to_owned()
}

async fn free_name(
    tx: &mut Transaction<'_, Postgres>,
    parent: &Uuid,
    wanted: &str,
) -> Result<NodeName, ApiError> {
    for attempt in 1..=MAX_ATTEMPTS {
        let candidate = if attempt == 1 {
            wanted.to_owned()
        } else {
            format!("{wanted} {attempt}")
        };
        let name: NodeName = candidate.parse()?;
        let taken = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM nodes WHERE parent_id = $1 AND name = $2 AND deleted_at IS NULL)",
        )
        .bind(parent)
        .bind(name.as_str())
        .fetch_one(&mut **tx)
        .await?;
        if !taken {
            return Ok(name);
        }
    }
    Err(ApiError::Conflict(wanted.to_owned()))
}

async fn held_blobs(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
) -> Result<Vec<BlobHash>, ApiError> {
    let nodes = sqlx::query_as::<_, (Uuid, Option<BlobHash>)>(
        "SELECT id, blob_hash FROM nodes WHERE owner_id = $1",
    )
    .bind(owner_id)
    .fetch_all(&mut **tx)
    .await?;
    let ids: Vec<Uuid> = nodes.iter().map(|(id, _)| *id).collect();
    let mut blobs: Vec<BlobHash> = nodes.into_iter().filter_map(|(_, hash)| hash).collect();
    blobs.extend(versions::blobs_under(tx, &ids).await?);
    Ok(blobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_is_not_too_long_is_kept_with_separators_made_harmless() {
        assert_eq!(
            fitted("Ada / Lovelace (ada@example.com)"),
            "Ada - Lovelace (ada@example.com)"
        );
    }

    #[test]
    fn a_long_name_is_cut_by_bytes_on_a_character_boundary() {
        let cjk = format!("{} (ada@example.com)", "\u{8a9e}".repeat(100));

        let name = fitted(&cjk);

        assert!(name.len() <= MAX_FOLDER_NAME_BYTES, "{} bytes", name.len());
        assert!(name.starts_with('\u{8a9e}'));
        assert!(
            format!("{name} 1000").parse::<NodeName>().is_ok(),
            "room is left for the number that makes a taken name free"
        );
    }
}
