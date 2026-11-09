use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::client::HubClient;
use crate::sources::{
    install_extension_router, lobehub, modelscope, skillhub, skills_sh, skillsmp, ExtensionRouter,
};
use crate::types::{BundleSource, HubError, RawBundlePayload};

#[derive(Default, Debug)]
pub struct RuaExtensionRouter;

#[async_trait]
impl ExtensionRouter for RuaExtensionRouter {
    async fn fetch(
        &self,
        client: &HubClient,
        source: &BundleSource,
        requested_version: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<Option<RawBundlePayload>, HubError> {
        match source {
            BundleSource::LobeHub(url) => Ok(Some(
                lobehub::fetch_bundle(client, url, requested_version, cancel).await?,
            )),
            BundleSource::ModelScope(url) => Ok(Some(
                modelscope::fetch_bundle(client, url, requested_version, cancel).await?,
            )),
            BundleSource::SkillsMp(url) => Ok(Some(
                skillsmp::fetch_bundle(client, url, requested_version, cancel).await?,
            )),
            BundleSource::SkillsSh(url) => Ok(Some(
                skills_sh::fetch_bundle(client, url, requested_version, cancel).await?,
            )),
            BundleSource::SkillHub(url) => Ok(Some(
                skillhub::fetch_bundle(client, url, requested_version, cancel).await?,
            )),
            BundleSource::Aliyun(_) => Ok(None),
            _ => Ok(None),
        }
    }
}

pub fn extension_router() -> Arc<dyn ExtensionRouter> {
    Arc::new(RuaExtensionRouter)
}

pub fn install_default_router() -> Arc<dyn ExtensionRouter> {
    let router = extension_router();
    install_extension_router(router.clone());
    router
}
