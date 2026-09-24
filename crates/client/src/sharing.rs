use roxycloud_core::grant::{Given, NewGrant};
use roxycloud_core::share::{Minted, NewShare, Share};
use uuid::Uuid;

use crate::remote::{Remote, RemoteError, answered, check};

impl Remote {
    pub async fn list_shares(&self) -> Result<Vec<Share>, RemoteError> {
        let response = self
            .http()
            .get(format!("{}/v1/shares", self.base()))
            .bearer_auth(self.token())
            .send()
            .await?;
        check(response.status(), "the shares")?;
        Ok(response.json().await?)
    }

    pub async fn share(&self, request: &NewShare) -> Result<Minted, RemoteError> {
        let response = self
            .http()
            .post(format!("{}/v1/shares", self.base()))
            .bearer_auth(self.token())
            .json(request)
            .send()
            .await?;
        Ok(answered(response, &request.path).await?.json().await?)
    }

    pub async fn revoke_share(&self, id: Uuid) -> Result<(), RemoteError> {
        let response = self
            .http()
            .delete(format!("{}/v1/shares/{id}", self.base()))
            .bearer_auth(self.token())
            .send()
            .await?;
        answered(response, &id.to_string()).await?;
        Ok(())
    }

    pub async fn list_grants(&self) -> Result<Vec<Given>, RemoteError> {
        let response = self
            .http()
            .get(format!("{}/v1/grants", self.base()))
            .bearer_auth(self.token())
            .send()
            .await?;
        check(response.status(), "what this account shares")?;
        Ok(response.json().await?)
    }

    pub async fn grant(&self, request: &NewGrant) -> Result<Given, RemoteError> {
        let response = self
            .http()
            .post(format!("{}/v1/grants", self.base()))
            .bearer_auth(self.token())
            .json(request)
            .send()
            .await?;
        Ok(answered(response, &request.path).await?.json().await?)
    }

    pub async fn withdraw_grant(&self, id: Uuid) -> Result<(), RemoteError> {
        let response = self
            .http()
            .delete(format!("{}/v1/grants/{id}", self.base()))
            .bearer_auth(self.token())
            .send()
            .await?;
        answered(response, &id.to_string()).await?;
        Ok(())
    }
}
