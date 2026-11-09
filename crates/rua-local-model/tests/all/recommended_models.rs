use rua_local_model::{get_recommended_models, LocalModelManager};

#[test]
fn below_4gb_returns_empty() {
    assert!(get_recommended_models(0).is_empty());
    assert!(get_recommended_models(2).is_empty());
    assert!(get_recommended_models(3).is_empty());
}

#[test]
fn tier_4gb_returns_2b_models() {
    let models = get_recommended_models(4);
    assert_eq!(models.len(), 2);
    assert!(models[0].id.contains("2B-Q4_K_M"));
    assert!(models[1].id.contains("2B-Q8_0"));
    for m in &models {
        assert_eq!(m.source, "modelscope");
        assert!(!m.downloaded);
    }
}

#[test]
fn tier_8gb_returns_2b_models() {
    let models = get_recommended_models(8);
    assert_eq!(models.len(), 2);
    assert!(models[0].name.contains("2B"));
}

#[test]
fn tier_16gb_returns_4b_models() {
    let models = get_recommended_models(16);
    assert_eq!(models.len(), 2);
    assert!(models[0].id.contains("4B-Q4_K_M"));
    assert!(models[1].id.contains("4B-Q8_0"));
}

#[test]
fn tier_32gb_returns_9b_models() {
    let models = get_recommended_models(32);
    assert_eq!(models.len(), 2);
    assert!(models[0].id.contains("9B-Q4_K_M"));
    assert!(models[1].id.contains("9B-Q8_0"));
}

#[test]
fn tier_boundary_9gb_returns_4b() {
    let models = get_recommended_models(9);
    assert_eq!(models.len(), 2);
    assert!(models[0].id.contains("4B"));
}

#[test]
fn tier_boundary_17gb_returns_9b() {
    let models = get_recommended_models(17);
    assert_eq!(models.len(), 2);
    assert!(models[0].id.contains("9B"));
}

#[test]
fn manager_get_recommended_models_sets_downloaded_false() {
    let mgr = LocalModelManager::new_for_test(std::env::temp_dir());
    let models = mgr.get_recommended_models(8);
    assert_eq!(models.len(), 2);
    for m in &models {
        assert!(!m.downloaded);
    }
}

#[test]
fn manager_get_recommended_models_marks_existing_gguf_as_downloaded() {
    let dir = tempfile::tempdir().unwrap();
    let first_model = get_recommended_models(8)
        .into_iter()
        .next()
        .expect("8gb tier should recommend a model");
    let safe_name = first_model.id.replace(['/', ':'], "_");
    let model_dir = dir.path().join("models");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::write(model_dir.join(format!("{safe_name}.gguf")), b"fake").unwrap();

    let mgr = LocalModelManager::new_for_test(dir.path());
    let models = mgr.get_recommended_models(8);

    assert!(
        models
            .iter()
            .any(|m| m.id == first_model.id && m.downloaded),
        "expected {} to be marked downloaded: {:?}",
        first_model.id,
        models
    );
    assert!(
        models
            .iter()
            .filter(|m| m.id != first_model.id)
            .all(|m| !m.downloaded),
        "only the matching local gguf should be marked downloaded: {models:?}"
    );
}

#[test]
fn manager_get_recommended_models_ignores_staging_download_files() {
    let dir = tempfile::tempdir().unwrap();
    let first_model = get_recommended_models(8)
        .into_iter()
        .next()
        .expect("8gb tier should recommend a model");
    let safe_name = first_model.id.replace(['/', ':'], "_");
    let model_dir = dir.path().join("models");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::write(
        model_dir.join(format!(".{safe_name}.gguf.downloading")),
        b"partial",
    )
    .unwrap();

    let mgr = LocalModelManager::new_for_test(dir.path());
    let models = mgr.get_recommended_models(8);

    assert!(
        models
            .iter()
            .filter(|m| m.id == first_model.id)
            .all(|m| !m.downloaded),
        "temporary download files must not mark a model as downloaded: {models:?}"
    );
}
