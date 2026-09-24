use std::path::Path;

use roxycloud_core::node::Node;
use roxycloud_core::version::Version;
use uuid::Uuid;

use crate::remote::{Remote, RemoteError, check};
use crate::transfer::save;

impl Remote {
    pub async fn list_versions(&self, path: &str) -> Result<Vec<Version>, RemoteError> {
        let url = self.endpoint("versions", path)?;
        let response = self
            .http()
            .get(&url)
            .bearer_auth(self.token())
            .send()
            .await?;
        check(response.status(), path)?;
        Ok(response.json().await?)
    }

    pub async fn download_version(
        &self,
        path: &str,
        id: Uuid,
        destination: &Path,
    ) -> Result<(), RemoteError> {
        let url = self.endpoint(&format!("version/{id}"), path)?;
        let response = self
            .http()
            .get(&url)
            .bearer_auth(self.token())
            .send()
            .await?;
        check(response.status(), path)?;
        save(response, destination).await
    }

    pub async fn restore_version(&self, path: &str, id: Uuid) -> Result<Node, RemoteError> {
        let url = self.endpoint(&format!("version/{id}"), path)?;
        let response = self
            .http()
            .post(&url)
            .bearer_auth(self.token())
            .send()
            .await?;
        check(response.status(), path)?;
        Ok(response.json().await?)
    }
}
