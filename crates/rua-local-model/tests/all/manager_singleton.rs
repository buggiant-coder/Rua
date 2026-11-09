use std::sync::Arc;

use rua_local_model::LocalModelManager;

#[test]
fn instance_returns_same_arc() {
    let a = LocalModelManager::init_instance(std::env::temp_dir(), true);
    let b = LocalModelManager::instance();
    assert!(Arc::ptr_eq(&a, &b));

    let c = LocalModelManager::instance();
    assert!(Arc::ptr_eq(&b, &c));
}

#[test]
fn init_instance_ignores_second_call() {
    let first = LocalModelManager::init_instance(std::env::temp_dir(), true);
    let second = LocalModelManager::init_instance(std::env::temp_dir().join("other"), false);
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn new_for_test_creates_independent_instance() {
    let a = LocalModelManager::new_for_test(std::env::temp_dir());
    let b = LocalModelManager::new_for_test(std::env::temp_dir());
    let arc_a = Arc::new(a);
    let arc_b = Arc::new(b);
    assert!(!Arc::ptr_eq(&arc_a, &arc_b));
}
