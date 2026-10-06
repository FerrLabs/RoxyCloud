use keyring::{Entry, Error};
use serde::{Deserialize, Serialize};

const SERVICE: &str = "Stashden";
const ACCOUNT: &str = "desktop";

#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub server: String,
    pub email: String,
    pub secret: String,
}

fn entry() -> Result<Entry, String> {
    Entry::new(SERVICE, ACCOUNT)
        .map_err(|error| format!("the system keychain is unavailable: {error}"))
}

pub async fn keep(credentials: Credentials) -> Result<(), String> {
    blocking(move || {
        let encoded = serde_json::to_string(&credentials).map_err(|error| error.to_string())?;
        entry()?
            .set_password(&encoded)
            .map_err(|error| format!("the system keychain refused the app password: {error}"))
    })
    .await
}

pub async fn kept() -> Result<Option<Credentials>, String> {
    blocking(|| match entry()?.get_password() {
        Ok(encoded) => serde_json::from_str(&encoded)
            .map(Some)
            .map_err(|error| format!("the keychain entry is not one this app wrote: {error}")),
        Err(Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("the system keychain could not be read: {error}")),
    })
    .await
}

pub async fn forget() -> Result<(), String> {
    blocking(|| match entry()?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(error) => Err(format!(
            "the system keychain kept the app password: {error}"
        )),
    })
    .await
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}
