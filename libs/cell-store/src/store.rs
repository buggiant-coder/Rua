use std::path::Path;

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};

use crate::error::CellStoreError;

#[async_trait]
pub trait CellStore: Send + Sync {
    async fn read<V>(&self, file: &Path, json_path: &str) -> Result<Option<V>, CellStoreError>
    where
        V: DeserializeOwned + Send + 'static;

    async fn write<V>(&self, file: &Path, json_path: &str, value: V) -> Result<(), CellStoreError>
    where
        V: Serialize + Send + 'static;

    async fn update<V, F>(
        &self,
        file: &Path,
        json_path: &str,
        mutator: F,
    ) -> Result<V, CellStoreError>
    where
        V: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(Option<V>) -> V + Send;

    async fn delete(&self, file: &Path, json_path: &str) -> Result<(), CellStoreError>;
}
