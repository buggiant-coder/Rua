use std::sync::Arc;

use rua_skills_hub::HubClient;
use rua_skills_market::provider::MarketProvider;
use rua_skills_market::providers::ModelScopeProvider;

#[tokio::test]
async fn modelscope_is_available_without_auth() {
    let provider = ModelScopeProvider::new(Arc::new(HubClient::new().expect("client")));
    let (available, reason) = provider.available();
    assert!(available);
    assert_eq!(reason, None);
    assert_eq!(provider.key(), "modelscope");
    assert_eq!(provider.label(), "ModelScope");
}

#[tokio::test]
async fn modelscope_registered_in_default_registry() {
    let registry =
        rua_skills_market::build_default_registry(Arc::new(HubClient::new().expect("client")));
    assert!(registry.contains("modelscope"));

    let infos = rua_skills_market::list_providers(&registry);
    let ms = infos
        .iter()
        .find(|p| p.key == "modelscope")
        .expect("modelscope listed");
    assert_eq!(ms.label, "ModelScope");
    assert!(ms.available);
    assert_eq!(ms.reason, None);
}
