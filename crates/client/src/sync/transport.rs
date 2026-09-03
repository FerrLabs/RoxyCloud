use std::future::Future;
use std::path::Path;

use super::path::RelPath;
use super::snapshot::Snapshot;

pub trait Transport {
    type Error: std::error::Error + Send + Sync + 'static;

    fn snapshot(&self) -> impl Future<Output = Result<Snapshot, Self::Error>> + Send;

    fn download_to(
        &self,
        path: &RelPath,
        destination: &Path,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn upload_from(
        &self,
        path: &RelPath,
        source: &Path,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn remove(&self, path: &RelPath) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
