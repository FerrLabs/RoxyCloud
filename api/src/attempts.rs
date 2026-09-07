use chrono::Duration;
use sqlx::PgPool;

use crate::error::ApiError;

/// How many wrong answers cost nothing. Someone who mistyped their password twice is not an
/// attacker, and a limiter that treats them as one gets turned off.
const FREE_ATTEMPTS: i32 = 10;

/// What the eleventh costs, and every failure after it doubles: a minute, two, four, up to an hour.
/// The escalation is what makes the difference. A flat window lets an attacker wait it out and take
/// the same number of guesses again, forever; doubling collapses a day of effort to a couple of
/// dozen attempts, while a person who genuinely forgot waits a minute the first time.
const FIRST_BLOCK_SECONDS: i64 = 60;
const LONGEST_BLOCK_SECONDS: i64 = 60 * 60;

/// How long a failure is remembered once nothing follows it. Well past the longest block on
/// purpose: the count has to outlive the block it caused, or waiting one out would hand back a
/// fresh ten and the escalation would never happen.
const MEMORY_SECONDS: f64 = 24.0 * 60.0 * 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "attempt_scope", rename_all = "lowercase")]
pub enum Scope {
    Login,
    Share,
}

impl Scope {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Share => "share",
        }
    }
}

/// Serialised per subject, because a read that decides and a write that records leave a window
/// every concurrent guess walks through together, and a burst is how guessing is actually done.
/// The lock is held across two fast statements and released before the password is checked: held
/// across argon2 instead, the same burst would exhaust the connection pool.
///
/// The counter is keyed on what is being guessed, an address or a link's fingerprint, never on
/// where the guess came from. A budget per source is a budget a botnet multiplies by the size of
/// the botnet.
///
/// An attempt that finds no block in force is evaluated and counted, and the count decides how long
/// the next block runs. Each block, once served, buys one more guess, and the blocks double.
/// Deciding on the count alone would have been simpler and wrong: the count only climbs, so nothing
/// past the tenth attempt would ever be evaluated again.
pub async fn spend(pool: &PgPool, scope: Scope, subject: &str) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{}:{subject}", scope.as_str()))
        .execute(&mut *tx)
        .await?;

    // Both comparisons happen in the database, so the deadline it wrote is the deadline it reads.
    let standing = sqlx::query_as::<_, (i32, bool, i64)>(
        "SELECT failures,
                coalesce(blocked_until > now(), false),
                coalesce(ceil(extract(epoch FROM (blocked_until - now())))::BIGINT, 0)
         FROM attempts
         WHERE scope = $1 AND subject = $2 AND last_at > now() - make_interval(secs => $3)",
    )
    .bind(scope)
    .bind(subject)
    .bind(MEMORY_SECONDS)
    .fetch_optional(&mut *tx)
    .await?;

    let recorded = match standing {
        Some((_, true, seconds)) => {
            return Err(ApiError::TooManyAttempts {
                seconds: seconds.max(1),
            });
        }
        Some((failures, _, _)) => failures.saturating_add(1),
        None => 1,
    };

    let block = f64::from(i32::try_from(blocked_for(recorded).num_seconds()).unwrap_or(i32::MAX));
    sqlx::query(
        "INSERT INTO attempts (scope, subject, failures, last_at, blocked_until)
         VALUES ($1, $2, $3, now(),
                 CASE WHEN $4 > 0 THEN now() + make_interval(secs => $4) END)
         ON CONFLICT (scope, subject) DO UPDATE
         SET failures = excluded.failures,
             last_at = excluded.last_at,
             blocked_until = excluded.blocked_until",
    )
    .bind(scope)
    .bind(subject)
    .bind(recorded)
    .bind(block)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Drops everything counted against a subject. Getting it right calls this, so nine typos followed
/// by the real password do not leave somebody one mistake away from being locked out; so does an
/// administrator, for the person an attacker has shut out of their own account.
pub async fn forget(pool: &PgPool, scope: Scope, subject: &str) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM attempts WHERE scope = $1 AND subject = $2")
        .bind(scope)
        .bind(subject)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn purge_expired(pool: &PgPool) -> Result<u64, ApiError> {
    let removed =
        sqlx::query("DELETE FROM attempts WHERE last_at < now() - make_interval(secs => $1)")
            .bind(MEMORY_SECONDS)
            .execute(pool)
            .await?;
    Ok(removed.rows_affected())
}

/// How long the subject is shut out once this attempt has been evaluated, given how many have
/// been recorded against it. The allowance counts attempts rather than failures because getting it
/// right deletes the row, so nothing accumulates against somebody who knows the answer.
fn blocked_for(recorded: i32) -> Duration {
    let over = recorded.saturating_sub(FREE_ATTEMPTS);
    let Ok(doublings) = u32::try_from(over) else {
        return Duration::zero();
    };

    let seconds = FIRST_BLOCK_SECONDS
        .saturating_mul(1_i64 << doublings.min(31))
        .min(LONGEST_BLOCK_SECONDS);
    Duration::seconds(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_person_who_mistypes_is_not_an_attacker() {
        for recorded in 1..FREE_ATTEMPTS {
            assert_eq!(
                blocked_for(recorded),
                Duration::zero(),
                "attempt {recorded} should still be free"
            );
        }
        assert_ne!(
            blocked_for(FREE_ATTEMPTS),
            Duration::zero(),
            "the allowance has to end somewhere, and this is where"
        );
    }

    #[test]
    fn the_block_doubles_with_every_failure_past_the_free_ones() {
        assert_eq!(blocked_for(FREE_ATTEMPTS), Duration::seconds(60));
        assert_eq!(blocked_for(FREE_ATTEMPTS + 1), Duration::seconds(120));
        assert_eq!(blocked_for(FREE_ATTEMPTS + 2), Duration::seconds(240));
    }

    #[test]
    fn the_block_stops_growing_at_an_hour() {
        assert_eq!(
            blocked_for(FREE_ATTEMPTS + 6),
            Duration::seconds(LONGEST_BLOCK_SECONDS)
        );
        assert_eq!(
            blocked_for(i32::MAX),
            Duration::seconds(LONGEST_BLOCK_SECONDS),
            "an overflowing shift would be a block of no time at all"
        );
    }

    #[test]
    fn a_day_of_guessing_buys_a_couple_of_dozen_tries() {
        let day = Duration::days(1);
        let mut spent = Duration::zero();
        let mut recorded = 0;

        while spent < day {
            recorded += 1;
            spent += blocked_for(recorded);
        }

        assert!(
            recorded < 40,
            "a dictionary attack got {recorded} guesses in a day"
        );
    }
}
