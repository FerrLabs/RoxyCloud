use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core,
};

pub const MIN_PASSWORD_LEN: usize = 12;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WeakPassword {
    #[error("password must be at least {MIN_PASSWORD_LEN} characters")]
    TooShort,
}

#[derive(Debug, thiserror::Error)]
#[error("hashing the password failed")]
pub struct HashFailed;

pub fn hash(password: &str) -> Result<String, HashFailed> {
    let salt = SaltString::generate(&mut rand_core::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| HashFailed)
}

#[must_use]
pub fn verify(password: &str, encoded: &str) -> bool {
    PasswordHash::new(encoded).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

static DECOY: OnceLock<String> = OnceLock::new();

pub fn verify_decoy(password: &str) {
    let decoy = DECOY.get_or_init(|| hash("a decoy that matches nothing").unwrap_or_default());
    let _ = verify(password, decoy);
}

pub fn check_strength(password: &str) -> Result<(), WeakPassword> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(WeakPassword::TooShort);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_verifies_against_its_own_password() {
        let encoded = hash("correct horse battery").expect("hashes");
        assert!(verify("correct horse battery", &encoded));
    }

    #[test]
    fn a_wrong_password_does_not_verify() {
        let encoded = hash("correct horse battery").expect("hashes");
        assert!(!verify("correct horse batterz", &encoded));
        assert!(!verify("", &encoded));
    }

    #[test]
    fn the_same_password_hashes_differently_every_time() {
        let first = hash("correct horse battery").expect("hashes");
        let second = hash("correct horse battery").expect("hashes");
        assert_ne!(first, second, "salt must be random");
        assert!(verify("correct horse battery", &first));
        assert!(verify("correct horse battery", &second));
    }

    #[test]
    fn a_corrupt_stored_hash_rejects_instead_of_panicking() {
        assert!(!verify("anything", ""));
        assert!(!verify("anything", "not-a-phc-string"));
        assert!(!verify("anything", "$argon2id$v=19$m=1,t=1,p=1$aaaa"));
    }

    #[test]
    fn the_hash_is_argon2id_not_a_weaker_default() {
        let encoded = hash("correct horse battery").expect("hashes");
        assert!(encoded.starts_with("$argon2id$"), "{encoded}");
    }

    #[test]
    fn the_decoy_does_real_work_so_an_unknown_account_costs_the_same_as_a_known_one() {
        verify_decoy("anything");
        let decoy = DECOY.get().expect("initialised by the call above");
        assert!(decoy.starts_with("$argon2id$"), "{decoy}");
        assert!(!verify("anything", decoy));
    }

    #[test]
    fn short_passwords_are_refused_counting_characters_not_bytes() {
        assert_eq!(check_strength("short"), Err(WeakPassword::TooShort));
        assert_eq!(
            check_strength(&"é".repeat(MIN_PASSWORD_LEN - 1)),
            Err(WeakPassword::TooShort)
        );
        assert!(check_strength(&"é".repeat(MIN_PASSWORD_LEN)).is_ok());
    }
}
