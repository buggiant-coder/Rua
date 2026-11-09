use rua_agent::profile::AgentProfile;
use rua_agent::prompt::{
    build_execution_environment_prompt, build_system_prompt_from_working_dir,
    inject_multimodal_hint, parse_frontmatter, strip_frontmatter, strip_heartbeat, PromptBuilder,
    PromptVariables,
};
use rua_core::runtime_layout::RuntimeLayout;

fn profile_with_prompt(prompt: &str) -> AgentProfile {
    let mut profile = AgentProfile::new("agent-a", "Agent A");
    profile.system_prompt = prompt.to_string();
    profile
}

#[test]
fn execution_environment_prompt_explains_android_runtime_and_phone_storage() {
    let layout = RuntimeLayout::android(
        "/data/app/com.rua.mobile-test/lib/arm64",
        "/data/data/com.rua.mobile/files",
        "/data/data/com.rua.mobile/files/home",
    );
    let workspace =
        std::path::Path::new("/data/data/com.rua.mobile/files/home/.rua/workspaces/default");

    let prompt = build_execution_environment_prompt(
        &layout,
        workspace,
        "mobile-apk",
        Some("phone-device-id"),
        Some("Pixel"),
    );

    assert!(prompt.contains("## Execution environment"));
    assert!(prompt.contains("phone-device-id"));
    assert!(prompt.contains("Pixel"));
    assert!(prompt.contains("Linux Compatibility Runtime"));
    assert!(prompt.contains("PRoot Debian"));
    assert!(prompt.contains(workspace.to_string_lossy().as_ref()));
    assert!(prompt.contains("/sdcard"));
    assert!(prompt.contains("inspect /sdcard"));
    assert!(prompt.contains("Avoid `~`"));
    assert!(prompt.contains("storage permission"));
}

#[test]
fn render_template_resolves_known_variables_and_preserves_unknown_placeholders() {
    let profile = profile_with_prompt("Base");
    let vars = PromptVariables::new("agent-a", "console")
        .with_user_id("user-1")
        .with_workspace("/workspace")
        .with_tools_list("- read: read files")
        .with_memory_summary("remember this")
        .with_custom("custom_key", "custom value");

    let rendered = PromptBuilder::render_template(
        "agent={{agent_id}}\nuser={{ user_id }}\nws={{workspace}}\ntools={{tools_list}}\nmem={{memory_summary}}\ncustom={{custom_key}}\nmissing={{missing}}",
        &vars,
        &profile,
    );

    assert!(rendered.text.contains("agent=agent-a"));
    assert!(rendered.text.contains("user=user-1"));
    assert!(rendered.text.contains("ws=/workspace"));
    assert!(rendered.text.contains("tools=- read: read files"));
    assert!(rendered.text.contains("mem=remember this"));
    assert!(rendered.text.contains("custom=custom value"));
    assert!(rendered.text.contains("missing={{missing}}"));
    assert_eq!(rendered.unresolved.len(), 1);
    assert_eq!(rendered.unresolved[0].name, "missing");
}

#[test]
fn template_rendering_and_multimodal_hint_are_separate_operations() {
    let profile = profile_with_prompt("You are {{agent_id}} in {{workspace}}.");
    let vars = PromptVariables::new("agent-a", "console")
        .with_workspace("/repo")
        .with_model_info(&llm_multiplex::traits::ModelInfo {
            id: "text-only".into(),
            name: "Text Only".into(),
            context_window: None,
            max_output_tokens: None,
            supports_image: Some(false),
            supports_video: Some(false),
            supports_multimodal: Some(false),
            probe_source: Some("test".into()),
            generate_kwargs: serde_json::Map::new(),
        });

    let rendered = PromptBuilder::render_template(&profile.system_prompt, &vars, &profile);
    let prompt = inject_multimodal_hint(
        &rendered.text,
        vars.supports_image,
        vars.supports_video,
        vars.supports_multimodal,
    );

    assert!(prompt.starts_with("You are agent-a in /repo."));
    assert!(
        prompt.contains("It appears that you can only understand text content."),
        "text-only model should get multimodal fallback hint: {prompt}"
    );
}

#[test]
fn template_can_render_agent_identity_from_variables() {
    let profile = profile_with_prompt(
        "# Agent Identity\n\n\
         Your agent id is `{{agent_id}}`. This is your unique identifier in the multi-agent system.\n\n\
         {{system_prompt}}",
    );
    let vars = PromptVariables::new("default", "console");

    let prompt = PromptBuilder::render_template(&profile.system_prompt, &vars, &profile).text;

    assert!(prompt.starts_with("# Agent Identity"));
    assert!(prompt.contains("Your agent id is `default`."));
    assert!(prompt.contains("This is your unique identifier in the multi-agent system."));
}

#[test]
fn template_renders_custom_agent_identity_before_base_prompt() {
    let profile = profile_with_prompt(
        "# Agent Identity\n\n\
             Your agent id is `{{agent_id}}`. This is your unique identifier in the multi-agent system.\n\n\
             You are a helpful assistant.",
    );
    let vars = PromptVariables::new("abc123", "console");

    let prompt = PromptBuilder::render_template(&profile.system_prompt, &vars, &profile).text;

    assert!(prompt.starts_with("# Agent Identity"));
    assert!(prompt.contains("Your agent id is `abc123`."));
    assert!(
        prompt.find("Agent Identity").unwrap() < prompt.find("helpful assistant").unwrap(),
        "identity should be rendered before the base workspace/system prompt: {prompt}"
    );
}

#[test]
fn template_can_render_agent_identity_without_workspace_prompt_files() {
    let profile = profile_with_prompt(
        "# Agent Identity\n\n\
             Your agent id is `{{agent_id}}`. This is your unique identifier in the multi-agent system.\n\n\
        ",
    );
    let vars = PromptVariables::new("xyz789", "console");

    let prompt = PromptBuilder::render_template(&profile.system_prompt, &vars, &profile).text;

    assert!(prompt.starts_with(
        "# Agent Identity\n\nYour agent id is `xyz789`. This is your unique identifier in the multi-agent system."
    ));
}

#[test]
fn working_dir_prompt_loader_does_not_inject_agent_identity_by_itself() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("prompts")).expect("prompts dir");
    std::fs::write(
        tmp.path().join("prompts").join("AGENTS.md"),
        "You are a helpful assistant.\n",
    )
    .expect("write AGENTS");

    let prompt = build_system_prompt_from_working_dir(tmp.path(), None).expect("build prompt");

    assert_eq!(prompt, "You are a helpful assistant.");
    assert!(!prompt.contains("Agent Identity"));
    assert!(!prompt.contains("Your agent id is"));
}

#[test]
fn working_dir_prompt_loader_treats_explicit_empty_selection_as_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let prompts = tmp.path().join("prompts");
    std::fs::create_dir_all(&prompts).expect("prompts dir");
    std::fs::write(
        prompts.join("10_DEFAULT.md"),
        "# Default\n\nLoaded by discovery.\n",
    )
    .expect("write prompt");

    let prompt = build_system_prompt_from_working_dir(tmp.path(), Some(&[])).expect("build prompt");

    assert!(prompt.is_empty());
}

#[test]
fn working_dir_prompt_loader_treats_bootstrap_as_hook_owned_context() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let prompts = tmp.path().join("prompts");
    std::fs::create_dir_all(&prompts).expect("prompts dir");
    std::fs::write(prompts.join("AGENTS.md"), "regular prompt\n").expect("write AGENTS");
    std::fs::write(prompts.join("BOOTSTRAP.md"), "first turn only\n").expect("write bootstrap");

    let prompt = build_system_prompt_from_working_dir(tmp.path(), None).expect("build prompt");

    assert_eq!(prompt, "regular prompt");
    assert!(
        !prompt.contains("first turn only"),
        "BOOTSTRAP.md is owned by BootstrapContextContributor and must not become regular prompt context"
    );

    let explicit = build_system_prompt_from_working_dir(
        tmp.path(),
        Some(&[
            "BOOTSTRAP.md".to_string(),
            rua_agent::prompt::LEGACY_CONSOLE_BUILTIN_PROMPT_FILE.to_string(),
            "AGENTS.md".to_string(),
        ]),
    )
    .expect("build explicit prompt");

    assert_eq!(explicit, "regular prompt");
}

#[test]
fn working_dir_prompt_loader_treats_agent_memory_as_reserved_builtin_context() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let prompts = tmp.path().join("prompts");
    std::fs::create_dir_all(&prompts).expect("prompts dir");
    std::fs::write(
        prompts.join("10_AGENT_SAFETY.md"),
        "# Safety\n\nsafety prompt\n",
    )
    .expect("write safety");
    std::fs::write(
        prompts.join("AGENT_MEMORY.md"),
        "user-visible memory prompt should be ignored\n",
    )
    .expect("write old memory prompt");

    let prompt = build_system_prompt_from_working_dir(tmp.path(), None).expect("build prompt");
    assert!(prompt.contains("safety prompt"));
    assert!(
        !prompt.contains("user-visible memory prompt should be ignored"),
        "AGENT_MEMORY.md is reserved for the mandatory built-in protocol and must not be loaded from workspace prompts"
    );

    let explicit = build_system_prompt_from_working_dir(
        tmp.path(),
        Some(&[
            "AGENT_MEMORY.md".to_string(),
            "10_AGENT_SAFETY.md".to_string(),
        ]),
    )
    .expect("build explicit prompt");
    assert_eq!(explicit, "# Safety\n\nsafety prompt");
}

#[test]
fn multimodal_hint_is_added_only_for_known_text_only_models() {
    let prompt = "You are a helpful assistant.";

    assert_eq!(
        inject_multimodal_hint(prompt, Some(true), Some(false), Some(true)),
        prompt,
        "vision-capable models must not receive the text-only fallback hint"
    );
    assert_eq!(
        inject_multimodal_hint(prompt, None, None, None),
        prompt,
        "unknown capabilities should preserve the base prompt"
    );

    let text_only = inject_multimodal_hint(prompt, Some(false), Some(false), Some(false));
    assert!(text_only.starts_with(prompt));
    assert!(
        text_only.contains("It appears that you can only understand text content."),
        "known text-only models should get the fallback hint: {text_only}"
    );
}

#[test]
fn heartbeat_sections_are_removed_without_touching_other_prompt_text() {
    assert_eq!(
        strip_heartbeat("no heartbeat markers here"),
        "no heartbeat markers here"
    );
    assert_eq!(
        strip_heartbeat(
            "before<!-- heartbeat:start -->heartbeat content\nmultiline<!-- heartbeat:end -->after"
        ),
        "beforeafter"
    );
    assert_eq!(
        strip_heartbeat(
            "a<!-- heartbeat:start -->x<!-- heartbeat:end -->b<!-- heartbeat:start -->y<!-- heartbeat:end -->c"
        ),
        "abc"
    );
    assert_eq!(
        strip_heartbeat("before<!-- heartbeat:start -->unfinished"),
        "before<!-- heartbeat:start -->unfinished",
        "unterminated heartbeat blocks are left intact for debuggability"
    );
}

#[test]
fn frontmatter_parser_strips_metadata_and_preserves_body() {
    let content = "---\nname: code\ngroup: style\nauto_detect: true\npriority: 7\ntriggers:\n  - code\n  - implement\n---\nBody text\n";

    assert_eq!(strip_frontmatter(content), "Body text\n");
    let (frontmatter, body) = parse_frontmatter(content);
    let frontmatter = frontmatter.expect("frontmatter");

    assert_eq!(body, "Body text\n");
    assert_eq!(frontmatter["name"], "code");
    assert_eq!(frontmatter["group"], "style");
    assert_eq!(frontmatter["auto_detect"], true);
    assert_eq!(frontmatter["priority"], 7);
    assert_eq!(
        frontmatter["triggers"],
        serde_json::json!(["code", "implement"])
    );
}

#[test]
fn system_prompt_from_working_dir_loads_sorted_markdown_strips_frontmatter_and_heartbeat() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let prompts = tmp.path().join("prompts");
    std::fs::create_dir_all(&prompts).expect("prompts dir");
    std::fs::write(
        prompts.join("B.md"),
        "---\nname: ignored\n---\nB body\n<!-- heartbeat:start -->dynamic<!-- heartbeat:end -->\n",
    )
    .expect("write B");
    std::fs::write(prompts.join("A.md"), "A body\n").expect("write A");
    std::fs::write(prompts.join("ignored.txt"), "ignore").expect("write txt");

    let prompt = build_system_prompt_from_working_dir(tmp.path(), None).expect("build prompt");

    assert_eq!(prompt, "A body\n\nB body");

    let explicit = build_system_prompt_from_working_dir(
        tmp.path(),
        Some(&["B.md".to_string(), "missing.md".to_string()]),
    )
    .expect("build explicit prompt");
    assert_eq!(explicit, "B body");
}
