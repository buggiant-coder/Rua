use rua_plugin_runtime::PluginSkillRegistry;

#[test]
fn register_and_list() {
    let reg = PluginSkillRegistry::new();
    let id = reg.register("p1", "greeting", "Say hello", "body").unwrap();
    assert_eq!(id, "plugin/p1/greeting");

    let all = reg.list_all();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "plugin/p1/greeting");
}

#[test]
fn register_duplicate_rejected() {
    let reg = PluginSkillRegistry::new();
    reg.register("p1", "greeting", "Say hello", "body").unwrap();
    let err = reg
        .register("p1", "greeting", "Different", "body2")
        .unwrap_err();
    assert!(err.to_string().contains("already registered"));
}

#[test]
fn different_plugins_different_skills() {
    let reg = PluginSkillRegistry::new();
    reg.register("p1", "greeting", "Hello from p1", "body1")
        .unwrap();
    reg.register("p2", "greeting", "Hello from p2", "body2")
        .unwrap();

    assert_eq!(reg.list_all().len(), 2);
    assert!(reg.get("plugin/p1/greeting").is_some());
    assert!(reg.get("plugin/p2/greeting").is_some());
}

#[test]
fn unregister_plugin_removes_all() {
    let reg = PluginSkillRegistry::new();
    reg.register("p1", "s1", "Skill 1", "body").unwrap();
    reg.register("p1", "s2", "Skill 2", "body").unwrap();
    reg.register("p2", "s3", "Skill 3", "body").unwrap();

    let removed = reg.unregister_plugin("p1");
    assert_eq!(removed.len(), 2);
    assert_eq!(reg.list_all().len(), 1);
    assert!(reg.get("plugin/p2/s3").is_some());
}

#[test]
fn unregister_nonexistent_plugin() {
    let reg = PluginSkillRegistry::new();
    let removed = reg.unregister_plugin("ghost");
    assert!(removed.is_empty());
}

#[test]
fn list_for_plugin() {
    let reg = PluginSkillRegistry::new();
    reg.register("p1", "s1", "Skill 1", "body").unwrap();
    reg.register("p1", "s2", "Skill 2", "body").unwrap();
    reg.register("p2", "s3", "Skill 3", "body").unwrap();

    let p1_skills = reg.list_for_plugin("p1");
    assert_eq!(p1_skills.len(), 2);

    let p2_skills = reg.list_for_plugin("p2");
    assert_eq!(p2_skills.len(), 1);
}

#[test]
fn get_missing() {
    let reg = PluginSkillRegistry::new();
    assert!(reg.get("plugin/p1/missing").is_none());
}

#[test]
fn is_plugin_skill_check() {
    assert!(PluginSkillRegistry::is_plugin_skill("plugin/p1/greeting"));
    assert!(!PluginSkillRegistry::is_plugin_skill("my-skill"));
    assert!(!PluginSkillRegistry::is_plugin_skill(""));
}

#[test]
fn resolve_parses_correctly() {
    let (pid, name) = PluginSkillRegistry::resolve("plugin/p1/greeting").unwrap();
    assert_eq!(pid, "p1");
    assert_eq!(name, "greeting");
    assert!(PluginSkillRegistry::resolve("not-plugin").is_none());
}

#[test]
fn register_list_unregister_cycle() {
    let reg = PluginSkillRegistry::new();
    reg.register(
        "hello_world",
        "greeting",
        "Greet the user",
        "---\nname: greeting\n---\nHello!",
    )
    .unwrap();

    let all = reg.list_all();
    assert!(all.iter().any(|s| s.name == "plugin/hello_world/greeting"));

    reg.unregister_plugin("hello_world");
    assert!(reg.list_all().is_empty());
}
