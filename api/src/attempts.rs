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

/// One statement, because two would not hold: a read that decides and a write that records leave a
/// window every concurrent guess walks through together, and a burst is exactly how guessing is
/// done. The row lock serialises them instead, so each caller gets its own number and only the
/// first ten are handed on to argon2.
///
/// The counter is keyed on what is being guessed, an address or a link's fingerprint, never on
/// where the guess came from. A budget per source is a budget a botnet multiplies by the size of
/// the botnet.
///
/// An attempt made while blocked is recorded like any other, so sustained hammering holds the
/// lockout open rather than letting it lapse and hand back a fresh ten.
pub async fn spend(pool: &PgPool, scope: Scope, subject: &str) -> Result<(), ApiError> {
    let recorded = sqlx::query_scalar::<_, i32>(
        "INSERT INTO attempts (scope, subject, failures, last_at)
         VALUES ($1, $2, 1, now())
         ON CONFLICT (scope, subject) DO UPDATE
         SET failures = CASE
                 WHEN attempts.last_at < now() - make_interval(secs => $3) THEN 1
                 ELSE attempts.failures + 1
             END,
             last_at = now()
         RETURNING failures",
    )
    .bind(scope)
    .bind(subject)
    .bind(MEMORY_SECONDS)
    .fetch_one(pool)
    .await?;

    // The statement above set `last_at` to the database's `now()`, so what is left of the block is
    // the whole of it. Nothing here weighs the process clock against the database's.
    let seconds = blocked_for(recorded).num_seconds();
    if seconds > 0 {
        return Err(ApiError::TooManyAttempts { seconds });
    }
    Ok(())
}

/// Getting it right clears the count, so nine typos followed by the real password do not leave
/// somebody one mistake away from being locked out of their own account.
pub async fn succeeded(pool: &PgPool, scope: Scope, subject: &str) -> Result<(), ApiError> {
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

/// How long the subject is shut out, given how many attempts have been recorded against it. The
/// allowance counts attempts rather than failures because getting it right deletes the row, so
/// nothing accumulates against somebody who knows the answer.
fn blocked_for(recorded: i32) -> Duration {
    let over = recorded.saturating_sub(FREE_ATTEMPTS + 1);
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
        for recorded in 1..=FREE_ATTEMPTS {
            assert_eq!(
                blocked_for(recorded),
                Duration::zero(),
                "attempt {recorded} should still be free"
            );
        }
        assert_ne!(
            blocked_for(FREE_ATTEMPTS + 1),
            Duration::zero(),
            "the allowance has to end somewhere, and this is where"
        );
    }

    #[test]
    fn the_block_doubles_with_every_failure_past_the_free_ones() {
        assert_eq!(blocked_for(FREE_ATTEMPTS + 1), Duration::seconds(60));
        assert_eq!(blocked_for(FREE_ATTEMPTS + 2), Duration::seconds(120));
        assert_eq!(blocked_for(FREE_ATTEMPTS + 3), Duration::seconds(240));
    }

    #[test]
    fn the_block_stops_growing_at_an_hour() {
        assert_eq!(
            blocked_for(FREE_ATTEMPTS + 7),
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
