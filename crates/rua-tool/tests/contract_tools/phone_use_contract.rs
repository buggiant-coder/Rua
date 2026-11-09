use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::builtin::phone_use::PhoneClickTool;
use rua_tool::builtin::phone_use::PhoneObserveTool;
use rua_tool::builtin::phone_use::PhoneStatusTool;
use rua_tool::builtin::phone_use::{PhoneAction, PhoneActionTool};
use rua_tool::{register_all_builtin_tools, MockCommandExecutor, ToolRegistry};
use serde_json::json;
use std::sync::Arc;

#[test]
fn phone_status_is_not_registered_on_non_android_hosts() {
    if cfg!(target_os = "android") {
        return;
    }
    let mut registry = ToolRegistry::new();
    register_all_builtin_tools(&mut registry, Arc::new(MockCommandExecutor::new()), None).unwrap();
    assert!(registry.get("phone_status").is_none());
}

#[tokio::test]
async fn observation_requires_trusted_chat_and_bounded_options() {
    let tool = PhoneObserveTool;
    assert!(tool
        .call(json!({}), ToolContext::default())
        .await
        .unwrap_err()
        .to_string()
        .contains("trusted ChatAddress"));
    for args in [
        json!({"owner":"other"}),
        json!({"device_id":"remote"}),
        json!({"method":"hello"}),
    ] {
        assert!(tool
            .call(args, ToolContext::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("Unknown observation option"));
    }
    assert_eq!(
        tool.schema().parameters["properties"]["max_nodes"]["maximum"],
        40
    );
    assert_eq!(
        tool.schema().parameters["properties"]["max_depth"]["maximum"],
        12
    );
}

#[tokio::test]
async fn click_refuses_conflicting_locators_and_forged_identity() {
    let tool = PhoneClickTool;
    let id = uuid::Uuid::new_v4().to_string();
    for args in [
        json!({"observation_id":id,"owner":"other","x":1,"y":2}),
        json!({"observation_id":id,"element_token":"node","x":1,"y":2}),
        json!({"observation_id":id,"x":1}),
        json!({"observation_id":id,"x":-1,"y":2}),
    ] {
        assert!(tool.call(args, ToolContext::default()).await.is_err());
    }
    assert!(tool
        .call(
            json!({"observation_id":id,"x":1,"y":2}),
            ToolContext::default()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("trusted ChatAddress"));
    assert!(!PhoneClickTool::spec().annotations.read_only);
}

#[tokio::test]
async fn phone_status_cannot_select_remote_device_or_inject_identity() {
    let tool = PhoneStatusTool;
    for args in [
        json!({"device_id":"remote"}),
        json!({"agent_id":"other"}),
        json!({"method":"hello"}),
    ] {
        let result = tool.call(args, ToolContext::default()).await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("takes no arguments"));
    }
    assert_eq!(tool.schema().name, "phone_status");
}

#[tokio::test]
async fn input_and_swipe_limits_are_enforced_before_dispatch() {
    let id = uuid::Uuid::new_v4().to_string();
    let text = PhoneActionTool(PhoneAction::TypeText);
    assert!(text
        .call(
            json!({"observation_id":id,"element_token":"x","text":"测".repeat(2001)}),
            ToolContext::default()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("exceeds"));
    let swipe = PhoneActionTool(PhoneAction::Swipe);
    assert!(swipe
        .call(
            json!({"observation_id":id,"x":1,"y":1,"end_x":1,"end_y":2,"duration_ms":1001}),
            ToolContext::default()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("duration"));
    assert!(
        !PhoneActionTool::spec(PhoneAction::LongPress)
            .annotations
            .read_only
    );
}

#[tokio::test]
async fn navigation_rejects_arbitrary_intents_and_requires_new_observation() {
    let launch = PhoneActionTool(PhoneAction::LaunchApp);
    let id = uuid::Uuid::new_v4().to_string();
    for package in ["intent://settings", "com.test; sh", "", "../app"] {
        assert!(launch
            .call(
                json!({"observation_id":id,"package_name":package}),
                ToolContext::default()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("package name"));
    }
    for action in [PhoneAction::Back, PhoneAction::Home] {
        let tool = PhoneActionTool(action);
        assert!(tool.call(json!({}), ToolContext::default()).await.is_err());
        assert_eq!(
            tool.schema().parameters["required"],
            json!(["observation_id"])
        );
    }
}

#[test]
fn screenshot_projection_is_image_content_not_base64_json_text() {
    use rua_core::message::ContentPart;
    use rua_core::phone_use::PhoneObservation;
    let id = uuid::Uuid::new_v4();
    let observation:PhoneObservation=serde_json::from_value(json!({
        "observation_id":id,"service_generation":uuid::Uuid::new_v4(),"package_name":"fixture","window_id":1,"width":1080,"height":2400,"rotation":0,"truncated":false,
        "nodes":[{"token":format!("{id}:0"),"path":[],"text":"","description":"","view_id":"","class_name":"View","bounds":[0,0,20,20],"clickable":false,"editable":false,"scrollable":false,"enabled":true,"visible":true,"password":false}],
        "screenshot":{"mime_type":"image/jpeg","width":576,"height":1280,"data":"/9j/2Q=="}
    })).unwrap();
    let result = rua_tool::builtin::phone_use::observation_result(observation).unwrap();
    assert!(result.content.iter().any(
        |p| matches!(p,ContentPart::Image{url,..} if url.starts_with("data:image/jpeg;base64,"))
    ));
    assert!(result
        .content
        .iter()
        .filter_map(|p| if let ContentPart::Text { text } = p {
            Some(text)
        } else {
            None
        })
        .all(|text| !text.contains("/9j/2Q==")));
}

#[test]
fn camera_capture_is_independently_guarded_and_not_read_only() {
    use rua_tool::builtin::phone_use::PhoneCameraTool;
    assert!(PhoneCameraTool::spec(false).annotations.read_only);
    assert!(!PhoneCameraTool::spec(true).annotations.read_only);
    assert_eq!(
        PhoneCameraTool::spec(true).parameters["required"],
        json!(["camera_id"])
    );
    assert_eq!(
        PhoneCameraTool::spec(true).parameters["additionalProperties"],
        false
    );
}
