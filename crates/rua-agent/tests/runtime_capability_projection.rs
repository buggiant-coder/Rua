use super::*;

const ACCESSIBILITY_TOOLS: [&str; 8] = [
    "phone_observe",
    "phone_click",
    "phone_long_press",
    "phone_type_text",
    "phone_swipe",
    "phone_back",
    "phone_home",
    "phone_launch_app",
];

fn phone_snapshot(
    accessibility: bool,
    policy: rua_config::types::tools::ToolSelectionPolicy,
) -> EffectiveCapabilitySnapshot {
    use rua_tool::builtin::phone_use::*;
    let mut registry = rua_tool::ToolRegistry::new();
    let mut register = |tool: Arc<dyn rua_contract::Tool>, spec| {
        registry
            .register(tool, spec, rua_tool::ToolNamespace::Builtin)
            .unwrap();
    };
    register(Arc::new(PhoneStatusTool), PhoneStatusTool::spec());
    register(Arc::new(PhoneReleaseTool), PhoneReleaseTool::spec());
    register(Arc::new(PhoneObserveTool), PhoneObserveTool::spec());
    register(Arc::new(PhoneClickTool), PhoneClickTool::spec());
    register(Arc::new(PhoneListAppsTool), PhoneListAppsTool::spec());
    for capture in [false, true] {
        register(
            Arc::new(PhoneCameraTool { capture }),
            PhoneCameraTool::spec(capture),
        );
    }
    for action in [
        PhoneAction::LongPress,
        PhoneAction::TypeText,
        PhoneAction::Swipe,
        PhoneAction::Back,
        PhoneAction::Home,
        PhoneAction::LaunchApp,
    ] {
        register(
            Arc::new(PhoneActionTool(action)),
            PhoneActionTool::spec(action),
        );
    }
    let mut toolkit = AgentToolkit::new();
    crate::tool_disclosure::populate_toolkit_from_registry(&mut toolkit, &registry);
    let configuration = ChatCapabilityConfiguration {
        selection_policy: policy,
        skills: Vec::new(),
        mcp_servers: BTreeMap::new(),
        plugin_ids: Vec::new(),
        extra: BTreeMap::new(),
    };
    let excluded = toolkit
        .tool_names()
        .into_iter()
        .filter(|name| !configuration.selects_tool(name, None, toolkit.disclosure_for(name)))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut toolkit = toolkit.create_filtered(&excluded);
    let mut snapshot = EffectiveCapabilitySnapshot {
        generation: 1,
        toolkit: None,
        skill_prompt_block: None,
        skill_names: Vec::new(),
        skills: Vec::new(),
        diagnostics: Vec::new(),
        capability_states: Vec::new(),
        mcp_tool_sources: HashMap::new(),
    };
    let names = toolkit
        .tool_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    filter_phone_readiness(
        &mut toolkit,
        &mut snapshot,
        names,
        &Ok(rua_core::phone_use::PhoneStatus {
            generation: "test-host".into(),
            platform: "android".into(),
            state: "ready".into(),
            accessibility,
            camera_permission: true,
            extra: BTreeMap::new(),
        }),
    );
    snapshot.toolkit = Some(Arc::new(toolkit));
    snapshot
}

fn request_with_guidance() -> ChatRequest {
    let mut request = ChatRequest::streaming(Vec::new());
    react_agent::replace_model_call_section(
        &mut request.messages,
        Some(ModelCallSection::new(
            ModelCallSectionKind::RuntimeGuidance,
            "Keep the captured Hook continuation.",
        )),
        ModelCallSectionKind::RuntimeGuidance,
    );
    request
}

fn developer_text(request: &ChatRequest) -> String {
    request
        .messages
        .iter()
        .filter(|message| message.role == agent_types::Role::Developer)
        .map(|message| message.text_content())
        .collect::<Vec<_>>()
        .join("\n")
}

fn availability_section(request: &ChatRequest) -> Option<String> {
    let sections = request
        .messages
        .iter()
        .filter(|message| {
            message
                .metadata
                .get(react_agent::MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
                .and_then(serde_json::Value::as_str)
                == Some(ModelCallSectionKind::CapabilityAvailability.metadata_name())
        })
        .collect::<Vec<_>>();
    assert!(sections.len() <= 1);
    sections.first().map(|message| message.text_content())
}

#[test]
fn missing_accessibility_is_explained_to_the_model_without_exposing_action_schemas() {
    let snapshot = phone_snapshot(
        false,
        rua_config::types::tools::ToolSelectionPolicy::default_enabled(),
    );
    let request = project_capability_context(
        request_with_guidance(),
        &snapshot,
        &react_agent::ToolDisclosureConfig::default(),
        Some(32_768),
    );
    let text = availability_section(&request).expect("typed capability availability section");
    for name in ACCESSIBILITY_TOOLS {
        assert!(
            text.contains(name),
            "missing availability diagnostic: {name}"
        );
        assert!(!request.tools.iter().any(|tool| tool.name == name));
    }
    assert!(text.contains("Phone Use requires the Android accessibility service"));
    assert!(text.contains("native permission settings"));
    assert!(text.contains("Agent configuration"));
    assert!(developer_text(&request).contains("Keep the captured Hook continuation."));
    assert_eq!(request.tools.len(), 5);
}

#[test]
fn current_permission_snapshot_replaces_unavailable_guidance_and_preserves_existing_guidance() {
    let mut request = request_with_guidance();
    for accessibility in [false, true, false, true] {
        let snapshot = phone_snapshot(
            accessibility,
            rua_config::types::tools::ToolSelectionPolicy::default_enabled(),
        );
        request = project_capability_context(
            request,
            &snapshot,
            &react_agent::ToolDisclosureConfig::default(),
            Some(32_768),
        );
        let text = developer_text(&request);
        assert!(availability_section(&request).is_some());
        assert_eq!(
            text.contains("Current unavailable capabilities"),
            !accessibility
        );
        assert_eq!(
            text.contains("Phone Use requires the Android accessibility service"),
            !accessibility
        );
        assert_eq!(
            text.matches("Keep the captured Hook continuation.").count(),
            1
        );
        for name in ACCESSIBILITY_TOOLS {
            assert_eq!(
                request.tools.iter().any(|tool| tool.name == name),
                accessibility,
                "{name} must follow the current permission snapshot"
            );
            assert_eq!(text.contains(name), !accessibility);
        }
    }
}

#[test]
fn phone_session_guidance_requires_explicit_release_and_follows_selected_capabilities() {
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    let mut request = project_capability_context(
        request_with_guidance(),
        &phone_snapshot(true, ToolSelectionPolicy::default_enabled()),
        &react_agent::ToolDisclosureConfig::default(),
        Some(32_768),
    );
    let text = availability_section(&request).expect("Phone Use session guidance");
    assert!(text.contains("phone_release"));
    assert!(text.contains("normal Turn completion does not release"));
    assert!(text.contains("model thinking"));
    assert!(text.contains("do not recreate"));
    assert!(developer_text(&request).contains("Keep the captured Hook continuation."));
    request = project_capability_context(
        request,
        &phone_snapshot(
            true,
            ToolSelectionPolicy::restricted(vec![
                ToolIdentity::builtin("phone_status"),
                ToolIdentity::builtin("phone_list_apps"),
                ToolIdentity::builtin("phone_list_cameras"),
            ]),
        ),
        &react_agent::ToolDisclosureConfig::default(),
        Some(32_768),
    );
    assert!(availability_section(&request).is_none());
    assert!(!developer_text(&request).contains("phone_release"));

    request = project_capability_context(
        request,
        &phone_snapshot(
            true,
            ToolSelectionPolicy::restricted(vec![ToolIdentity::builtin("phone_observe")]),
        ),
        &react_agent::ToolDisclosureConfig::default(),
        Some(32_768),
    );
    let text = availability_section(&request).expect("Phone Use session guidance");
    assert!(!text.contains("phone_release"));
    assert!(!request
        .tools
        .iter()
        .any(|tool| tool.name == "phone_release"));
    assert!(text.contains("native control"));
}

#[test]
fn unavailable_guidance_does_not_reexpose_policy_disabled_phone_tools() {
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    let mut individual = ToolSelectionPolicy::default_enabled();
    individual.set_tool_enabled(ToolIdentity::builtin("phone_observe"), false);
    let capability = ToolSelectionPolicy::DefaultEnabled {
        disabled_tools: Vec::new(),
        disabled_capabilities: vec!["phone_use".into()],
        extra: Default::default(),
    };
    for policy in [
        individual,
        capability,
        ToolSelectionPolicy::restricted(vec![ToolIdentity::builtin("phone_status")]),
    ] {
        let mut request = project_capability_context(
            request_with_guidance(),
            &phone_snapshot(false, ToolSelectionPolicy::default_enabled()),
            &react_agent::ToolDisclosureConfig::default(),
            Some(32_768),
        );
        for accessibility in [false, true] {
            request = project_capability_context(
                request,
                &phone_snapshot(accessibility, policy.clone()),
                &react_agent::ToolDisclosureConfig::default(),
                Some(32_768),
            );
            assert!(!developer_text(&request).contains("phone_observe"));
            assert!(!request
                .tools
                .iter()
                .any(|tool| tool.name == "phone_observe"));
        }
    }
}
