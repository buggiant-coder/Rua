use rua_plugin_runtime::{HookError, HookPoint, HookRegistry, HookSpec};

fn make_spec(point: HookPoint, name: &str, priority: i32) -> HookSpec {
    HookSpec {
        hook_point: point,
        hook_name: name.to_string(),
        callback: format!("on_{name}"),
        priority,
    }
}

#[tokio::test]
async fn register_returns_stable_registration_id() {
    let registry = HookRegistry::new();

    let id = registry
        .register("p1", make_spec(HookPoint::Startup, "init", 100))
        .await
        .unwrap();

    assert!(id.starts_with("hook-"));
}

#[tokio::test]
async fn register_duplicate_hook_name_for_same_plugin_and_point_is_rejected() {
    let registry = HookRegistry::new();
    registry
        .register("p1", make_spec(HookPoint::Startup, "init", 100))
        .await
        .unwrap();

    let err = registry
        .register("p1", make_spec(HookPoint::Startup, "init", 50))
        .await
        .unwrap_err();

    assert!(matches!(err, HookError::AlreadyRegistered { .. }));
}

#[tokio::test]
async fn get_for_point_sorts_hooks_by_priority() {
    let registry = HookRegistry::new();
    registry
        .register("p-low", make_spec(HookPoint::BeforeToolCall, "low", 200))
        .await
        .unwrap();
    registry
        .register("p-high", make_spec(HookPoint::BeforeToolCall, "high", 10))
        .await
        .unwrap();

    let entries = registry.get_for_point(HookPoint::BeforeToolCall).await;

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].spec.hook_name, "high");
    assert_eq!(entries[1].spec.hook_name, "low");
}

#[tokio::test]
async fn unregister_by_id_removes_only_matching_hook() {
    let registry = HookRegistry::new();
    let remove_id = registry
        .register("p1", make_spec(HookPoint::Startup, "remove-me", 100))
        .await
        .unwrap();
    registry
        .register("p1", make_spec(HookPoint::Startup, "keep-me", 100))
        .await
        .unwrap();

    assert!(registry.unregister(&remove_id).await);

    let remaining = registry.get_for_point(HookPoint::Startup).await;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].spec.hook_name, "keep-me");
}

#[tokio::test]
async fn unregister_unknown_id_returns_false() {
    let registry = HookRegistry::new();

    assert!(!registry.unregister("hook-999").await);
}

#[tokio::test]
async fn remove_by_plugin_clears_hooks_across_all_points() {
    let registry = HookRegistry::new();
    registry
        .register("p1", make_spec(HookPoint::Startup, "a", 10))
        .await
        .unwrap();
    registry
        .register("p1", make_spec(HookPoint::Shutdown, "b", 20))
        .await
        .unwrap();
    registry
        .register("p2", make_spec(HookPoint::Startup, "c", 30))
        .await
        .unwrap();

    registry.remove_by_plugin("p1").await;

    assert!(registry.list_for_plugin("p1").await.is_empty());
    assert_eq!(registry.list_for_plugin("p2").await.len(), 1);
}

#[tokio::test]
async fn list_for_plugin_returns_hooks_from_multiple_points() {
    let registry = HookRegistry::new();
    registry
        .register("p1", make_spec(HookPoint::Startup, "a", 10))
        .await
        .unwrap();
    registry
        .register("p1", make_spec(HookPoint::BeforeToolCall, "b", 20))
        .await
        .unwrap();
    registry
        .register("p2", make_spec(HookPoint::Startup, "c", 30))
        .await
        .unwrap();

    let p1_hooks = registry.list_for_plugin("p1").await;

    assert_eq!(p1_hooks.len(), 2);
    assert!(p1_hooks.iter().any(|hook| hook.spec.hook_name == "a"));
    assert!(p1_hooks.iter().any(|hook| hook.spec.hook_name == "b"));
}
