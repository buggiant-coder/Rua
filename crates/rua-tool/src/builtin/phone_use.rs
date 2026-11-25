use crate::result_store::estimate_text_tokens;
use crate::{annotations::ExtendedToolAnnotations, schema::ToolSpec};
use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::phone_use::{
    DEFAULT_OBSERVATION_DEPTH, DEFAULT_OBSERVATION_NODES, MAX_OBSERVATION_DEPTH,
    MAX_OBSERVATION_NODES,
};
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_core::{Result, RuaError};

/// Model-visible budget for one observation result, in estimated tokens.
///
/// Deliberately below `DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS` so a full tree observation is
/// trimmed by this function instead of by the generic line/byte compressor.
pub const DEFAULT_OBSERVATION_RESULT_TOKENS: usize = 6_000;

pub struct PhoneStatusTool;
pub struct PhoneReleaseTool;
pub struct PhoneObserveTool;
pub struct PhoneClickTool;
pub struct PhoneListAppsTool;
pub struct PhoneCameraTool {
    pub capture: bool,
}

impl PhoneCameraTool {
    pub fn spec(capture: bool) -> ToolSpec {
        let name = if capture {
            "phone_take_photo"
        } else {
            "phone_list_cameras"
        };
        ToolSpec{name:name.into(),description:if capture {"Take one photo from an enumerated Android camera in a visible Rua preview. Requires separate camera permission and image-capable model. No silent background capture. User Stop/cancel releases camera; never retry uncertain capture automatically."}else{"List actual front/back cameras on this Android execution device; does not open or capture the camera."}.into(),parameters:if capture {serde_json::json!({"type":"object","properties":{"camera_id":{"type":"string","maxLength":100}},"required":["camera_id"],"additionalProperties":false})}else{serde_json::json!({"type":"object","properties":{},"additionalProperties":false})},annotations:if capture{ExtendedToolAnnotations::default()}else{ExtendedToolAnnotations::read_only()},namespace:"builtin".into()}
    }
}

#[async_trait]
impl Tool for PhoneCameraTool {
    fn name(&self) -> &str {
        if self.capture {
            "phone_take_photo"
        } else {
            "phone_list_cameras"
        }
    }
    fn schema(&self) -> ToolSchema {
        let s = Self::spec(self.capture);
        ToolSchema {
            name: s.name,
            description: s.description,
            parameters: s.parameters,
        }
    }
    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        crate::schema::validate_args(self.name(), &args, &self.schema().parameters)
            .map_err(|e| invalid(e.to_string()))?;
        let session = invocation_session(&ctx)?;
        if self.capture
            && ctx
                .metadata
                .get(rua_core::tool::MODEL_SUPPORTS_IMAGE_METADATA_KEY)
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        {
            return Err(invalid("Photo capture requires an image-capable model"));
        }
        let bridge =
            rua_core::phone_use::bridge().ok_or_else(|| invalid("Phone Use is not connected"))?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let operation = async {
            if self.capture {
                let camera = args["camera_id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 100)
                    .ok_or_else(|| invalid("Invalid camera id"))?;
                let image = bridge
                    .take_photo(
                        &session,
                        ctx.metadata["tool_call_id"]
                            .as_str()
                            .expect("trusted invocation"),
                        camera,
                        &cancel,
                    )
                    .await
                    .map_err(invalid)?;
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&image.data)
                    .map_err(|_| invalid("Invalid photo encoding"))?;
                if bytes.len() > 768 * 1024 || !bytes.starts_with(&[0xff, 0xd8, 0xff]) {
                    return Err(invalid("Invalid JPEG photo"));
                }
                let media = archive_phone_image(&ctx, &bytes, "phone-camera")?;
                let mut output = ToolResult::text(
                    self.name(),
                    format!(
                        "Photo captured from camera {camera}, {}x{}. Camera released.",
                        image.width, image.height
                    ),
                );
                output.content.push(rua_core::message::ContentPart::Image {
                    url: format!("data:{};base64,{}", image.mime_type, image.data),
                    media_type: Some(image.mime_type),
                    alt: Some("Phone camera photo".into()),
                });
                output.metadata.insert(
                    "display_media".into(),
                    serde_json::to_value([&media]).map_err(|e| invalid(e.to_string()))?,
                );
                output.content.push(rua_core::message::ContentPart::Text {
                    text: format!("Photo reference: {}", media.media_ref),
                });
                Ok(output)
            } else {
                let value = bridge.list_cameras(&cancel).await.map_err(invalid)?;
                Ok(ToolResult::text(
                    self.name(),
                    serde_json::to_string(&value).map_err(|e| invalid(e.to_string()))?,
                ))
            }
        };
        if let Some(mut receiver) = ctx.cancel.clone() {
            tokio::select! {biased;
                _=async{if !*receiver.borrow(){let _=receiver.changed().await;}}=>{cancel.cancel();Err(invalid("Phone camera request cancelled"))},
                result=operation=>result,
            }
        } else {
            operation.await
        }
    }
}

impl PhoneListAppsTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {name:"phone_list_apps".into(),description:"List up to 200 launchable apps on this Android execution device. No remote device, shell command, or arbitrary Intent input.".into(),parameters:serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),annotations:ExtendedToolAnnotations::read_only(),namespace:"builtin".into()}
    }
}
#[async_trait]
impl Tool for PhoneListAppsTool {
    fn name(&self) -> &str {
        "phone_list_apps"
    }
    fn schema(&self) -> ToolSchema {
        let s = Self::spec();
        ToolSchema {
            name: s.name,
            description: s.description,
            parameters: s.parameters,
        }
    }
    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        crate::schema::validate_args(self.name(), &args, &self.schema().parameters)
            .map_err(|e| invalid(e.to_string()))?;
        invocation_session(&ctx)?;
        let bridge =
            rua_core::phone_use::bridge().ok_or_else(|| invalid("Phone Use is not connected"))?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let operation = bridge.list_apps(&cancel);
        let result = if let Some(mut receiver) = ctx.cancel.clone() {
            tokio::select! {biased;
                _=async{if !*receiver.borrow(){let _=receiver.changed().await;}}=>return Err(invalid("Phone app listing cancelled")),
                result=operation=>result,
            }
        } else {
            operation.await
        };
        Ok(ToolResult::text(
            self.name(),
            serde_json::to_string(&result.map_err(invalid)?).map_err(|e| invalid(e.to_string()))?,
        ))
    }
}

#[derive(Clone, Copy)]
pub enum PhoneAction {
    LongPress,
    TypeText,
    Swipe,
    Back,
    Home,
    LaunchApp,
}

pub struct PhoneActionTool(pub PhoneAction);
impl PhoneActionTool {
    fn action(&self) -> &'static str {
        match self.0 {
            PhoneAction::LongPress => "long_press",
            PhoneAction::TypeText => "type_text",
            PhoneAction::Swipe => "swipe",
            PhoneAction::Back => "back",
            PhoneAction::Home => "home",
            PhoneAction::LaunchApp => "launch_app",
        }
    }
    pub fn spec(action: PhoneAction) -> ToolSpec {
        let tool = Self(action);
        let mut properties = serde_json::json!({"observation_id":{"type":"string"},"element_token":{"type":"string"}});
        let mut required = vec!["observation_id", "element_token"];
        match action {
            PhoneAction::Back | PhoneAction::Home | PhoneAction::LaunchApp => {
                properties = serde_json::json!({"observation_id":{"type":"string"}});
                required = vec!["observation_id"];
                if matches!(action, PhoneAction::LaunchApp) {
                    properties["package_name"] =
                        serde_json::json!({"type":"string","maxLength":200});
                    required.push("package_name");
                }
            }
            PhoneAction::LongPress => {}
            PhoneAction::TypeText => {
                properties["text"] = serde_json::json!({"type":"string","maxLength":2000});
                required.push("text");
            }
            PhoneAction::Swipe => {
                properties = serde_json::json!({"observation_id":{"type":"string"},"x":{"type":"number","minimum":0},"y":{"type":"number","minimum":0},"end_x":{"type":"number","minimum":0},"end_y":{"type":"number","minimum":0},"duration_ms":{"type":"integer","minimum":50,"maximum":1000,"default":400}});
                required = vec!["observation_id", "x", "y", "end_x", "end_y"];
            }
        }
        ToolSpec { name:tool.name().into(),description:"Perform one bounded action on a freshly observed Android window. Never target Rua controls or system permission surfaces. Submitted effects may be unknown; inspect the returned observation, never replay after cancellation. The floating Stop control ends this Phone Use session and cancels its Turn; do not retry Phone Use in that Turn. Release the session with phone_release when finished.".into(),parameters:serde_json::json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),annotations:ExtendedToolAnnotations::default(),namespace:"builtin".into() }
    }
}

#[async_trait]
impl Tool for PhoneActionTool {
    fn name(&self) -> &str {
        match self.0 {
            PhoneAction::LongPress => "phone_long_press",
            PhoneAction::TypeText => "phone_type_text",
            PhoneAction::Swipe => "phone_swipe",
            PhoneAction::Back => "phone_back",
            PhoneAction::Home => "phone_home",
            PhoneAction::LaunchApp => "phone_launch_app",
        }
    }
    fn schema(&self) -> ToolSchema {
        let s = Self::spec(self.0);
        ToolSchema {
            name: s.name,
            description: s.description,
            parameters: s.parameters,
        }
    }
    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        let schema = self.schema();
        crate::schema::validate_args(self.name(), &args, &schema.parameters)
            .map_err(|e| invalid(e.to_string()))?;
        if matches!(self.0, PhoneAction::TypeText)
            && args["text"]
                .as_str()
                .is_none_or(|s| s.encode_utf16().count() > 2000)
        {
            return Err(invalid("Text exceeds 2000 characters"));
        }
        if matches!(self.0, PhoneAction::Swipe) {
            for key in ["x", "y", "end_x", "end_y"] {
                if args[key].as_f64().is_none_or(|v| !v.is_finite() || v < 0.0) {
                    return Err(invalid("Invalid swipe coordinates"));
                }
            }
            if args
                .get("duration_ms")
                .is_some_and(|v| v.as_u64().is_none_or(|v| !(50..=1000).contains(&v)))
            {
                return Err(invalid("Invalid swipe duration"));
            }
        }
        if matches!(self.0, PhoneAction::LaunchApp)
            && args["package_name"].as_str().is_none_or(|s| {
                s.is_empty()
                    || s.len() > 200
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
            })
        {
            return Err(invalid("Invalid launchable package name"));
        }
        let observation = args["observation_id"]
            .as_str()
            .ok_or_else(|| invalid("Missing observation"))?;
        uuid::Uuid::parse_str(observation).map_err(|_| invalid("Invalid observation"))?;
        let session = invocation_session(&ctx)?;
        let operation = ctx.metadata["tool_call_id"]
            .as_str()
            .expect("validated invocation");
        let bridge =
            rua_core::phone_use::bridge().ok_or_else(|| invalid("Phone Use is not connected"))?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let request = bridge.interact(self.action(), &session, operation, args, &cancel);
        let result = if let Some(mut receiver) = ctx.cancel.clone() {
            tokio::select! {biased;
                _=async {if !*receiver.borrow(){let _=receiver.changed().await;}}=>{cancel.cancel();return Err(invalid("Phone action cancelled; submitted outcome may be unknown"));},
                result=request=>result,
            }
        } else {
            request.await
        };
        action_result(
            self.name(),
            result.map_err(invalid)?,
            observation_budget(&ctx),
        )
    }
}

impl PhoneClickTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "phone_click".into(),
            description: "Execute one bounded click on the observed Android screen. Requires a fresh Chat-owned observation_id and exactly one node token or x/y pair. Never target Rua termination/approval controls or system authorization surfaces. Submitted is not proof of app effect; inspect the returned observation. Floating Stop ends the Phone Use session and cancels its Turn; do not retry in that Turn or retry an unknown outcome. Call phone_release when finished.".into(),
            parameters: serde_json::json!({"type":"object","properties":{"observation_id":{"type":"string"},"element_token":{"type":"string"},"x":{"type":"number"},"y":{"type":"number"}},"required":["observation_id"],"additionalProperties":false}),
            annotations: ExtendedToolAnnotations::default(), namespace:"builtin".into(),
        }
    }
}

fn invocation_session(ctx: &ToolContext) -> Result<rua_core::phone_use::PhoneSessionContext> {
    let session =
        rua_core::phone_use::PhoneSessionContext::from_metadata(&ctx.agent_id, &ctx.metadata)
            .map_err(invalid)?;
    if ctx
        .metadata
        .get("tool_call_id")
        .and_then(serde_json::Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(invalid("Missing trusted invocation tool_call_id"));
    }
    Ok(session)
}

impl PhoneReleaseTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "phone_release".into(),
            description: "Release this Chat's persistent Phone Use session and hide its operation indicator when finished using the phone. Safe to repeat. Does not cancel the conversation or reverse actions. Status and app/camera listings do not start a session.".into(),
            parameters: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            annotations: ExtendedToolAnnotations { idempotent: true, ..Default::default() },
            namespace: "builtin".into(),
        }
    }
}

#[async_trait]
impl Tool for PhoneReleaseTool {
    fn name(&self) -> &str {
        "phone_release"
    }
    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }
    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        crate::schema::validate_args(self.name(), &args, &self.schema().parameters)
            .map_err(|e| invalid(e.to_string()))?;
        let session = invocation_session(&ctx)?;
        let bridge =
            rua_core::phone_use::bridge().ok_or_else(|| invalid("Phone Use is not connected"))?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let value = bridge.release(&session, &cancel).await.map_err(invalid)?;
        Ok(ToolResult::text(
            self.name(),
            serde_json::to_string(&value).map_err(|e| invalid(e.to_string()))?,
        ))
    }
}

#[async_trait]
impl Tool for PhoneClickTool {
    fn name(&self) -> &str {
        "phone_click"
    }
    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }
    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        let object = args
            .as_object()
            .ok_or_else(|| invalid("Expected click options"))?;
        if object
            .keys()
            .any(|k| !matches!(k.as_str(), "observation_id" | "element_token" | "x" | "y"))
        {
            return Err(invalid("Unknown click option"));
        }
        let observation = object
            .get("observation_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("Missing observation"))?;
        uuid::Uuid::parse_str(observation).map_err(|_| invalid("Invalid observation"))?;
        let has_token = object
            .get("element_token")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| !s.is_empty());
        let has_coordinates = object.contains_key("x") || object.contains_key("y");
        if has_token == has_coordinates
            || (has_coordinates
                && ["x", "y"].iter().any(|k| {
                    object
                        .get(*k)
                        .and_then(serde_json::Value::as_f64)
                        .is_none_or(|v| !v.is_finite() || v < 0.0)
                }))
        {
            return Err(invalid("Provide exactly one element_token or x/y pair"));
        }
        let session = invocation_session(&ctx)?;
        let operation_id = ctx.metadata["tool_call_id"]
            .as_str()
            .expect("validated invocation");
        let bridge =
            rua_core::phone_use::bridge().ok_or_else(|| invalid("Phone Use is not connected"))?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let request = bridge.interact("click", &session, operation_id, args, &cancel);
        let result = if let Some(mut receiver) = ctx.cancel.clone() {
            tokio::select! {biased;
                _=async {if !*receiver.borrow() {let _=receiver.changed().await;}} => {cancel.cancel(); return Err(invalid("Phone click cancelled; submitted outcome may be unknown"));},
                result=request=>result,
            }
        } else {
            request.await
        };
        action_result(
            self.name(),
            result.map_err(invalid)?,
            observation_budget(&ctx),
        )
    }
}

impl PhoneObserveTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "phone_observe".into(),
            description: "Observe the Android execution device's foreground App/window and bounded accessibility tree. Returns Chat-owned node tokens and an optional screenshot for image-capable models on Android 11+. Only readable or actionable nodes are listed; layout containers are traversed but never counted, so raise max_nodes when a page reports truncated and you still need to reach deeper controls. Input coordinates use the observation's full display dimensions, not a resized screenshot. Requires user-enabled accessibility; no phone input.".into(),
            parameters: serde_json::json!({"type":"object","properties":{"max_nodes":{"type":"integer","minimum":1,"maximum":MAX_OBSERVATION_NODES,"default":DEFAULT_OBSERVATION_NODES},"max_depth":{"type":"integer","minimum":1,"maximum":MAX_OBSERVATION_DEPTH,"default":DEFAULT_OBSERVATION_DEPTH},"include_screenshot":{"type":"boolean","default":false}},"additionalProperties":false}),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }
}

#[async_trait]
impl Tool for PhoneObserveTool {
    fn name(&self) -> &str {
        "phone_observe"
    }
    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }
    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        let object = args
            .as_object()
            .ok_or_else(|| invalid("Expected observation options"))?;
        if object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "max_nodes" | "max_depth" | "include_screenshot"
            )
        }) {
            return Err(invalid("Unknown observation option"));
        }
        let limit = |key: &str, default: u32, max: u32| -> Result<u32> {
            match object.get(key) {
                None => Ok(default),
                Some(v) => v
                    .as_u64()
                    .filter(|v| (1..=u64::from(max)).contains(v))
                    .map(|v| v as u32)
                    .ok_or_else(|| invalid(format!("{key} must be an integer in 1..={max}"))),
            }
        };
        let session = invocation_session(&ctx)?;
        let capture = match object.get("include_screenshot") {
            None => false,
            Some(v) => v
                .as_bool()
                .ok_or_else(|| invalid("include_screenshot must be boolean"))?,
        };
        if capture
            && ctx
                .metadata
                .get(rua_core::tool::MODEL_SUPPORTS_IMAGE_METADATA_KEY)
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        {
            return Err(invalid("Screenshot requires an image-capable model; request a text-only observation instead"));
        }
        let bridge =
            rua_core::phone_use::bridge().ok_or_else(|| invalid("Phone Use is not connected"))?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let max_nodes = limit(
            "max_nodes",
            DEFAULT_OBSERVATION_NODES,
            MAX_OBSERVATION_NODES,
        )?;
        let max_depth = limit(
            "max_depth",
            DEFAULT_OBSERVATION_DEPTH,
            MAX_OBSERVATION_DEPTH,
        )?;
        let result = async {
            if capture {
                bridge
                    .screenshot(
                        &session,
                        ctx.metadata["tool_call_id"]
                            .as_str()
                            .expect("trusted invocation"),
                        max_nodes,
                        max_depth,
                        &cancel,
                    )
                    .await
            } else {
                bridge
                    .observe(
                        &session,
                        ctx.metadata["tool_call_id"]
                            .as_str()
                            .expect("trusted invocation"),
                        max_nodes,
                        max_depth,
                        &cancel,
                    )
                    .await
            }
        };
        let result = if let Some(mut receiver) = ctx.cancel.clone() {
            tokio::select! {
                biased;
                _ = async { if !*receiver.borrow() { let _ = receiver.changed().await; } } => return Err(invalid("Phone observation cancelled")),
                result = result => result,
            }
        } else {
            result.await
        };
        let observation = result.map_err(invalid)?;
        let media = if let Some(image) = &observation.screenshot {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .map_err(|_| invalid("Invalid screenshot encoding"))?;
            Some(archive_phone_image(&ctx, &bytes, "phone-screenshot")?)
        } else {
            None
        };
        let mut output = observation_result(observation, observation_budget(&ctx))?;
        if let Some(media) = media {
            output.metadata.insert(
                "display_media".into(),
                serde_json::to_value([&media]).map_err(|e| invalid(e.to_string()))?,
            );
            output.content.push(rua_core::message::ContentPart::Text {
                text: format!("Screenshot reference: {}", media.media_ref),
            });
        }
        Ok(output)
    }
}

pub fn observation_result(
    mut observation: rua_core::phone_use::PhoneObservation,
    max_tokens: usize,
) -> Result<ToolResult> {
    fit_observation_to_model_budget(&mut observation, max_tokens);
    let image = observation.screenshot.take();
    let mut output = ToolResult::text(
        "phone_observe",
        serde_json::to_string(&observation).map_err(|e| invalid(e.to_string()))?,
    );
    if let Some(image) = image {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|_| invalid("Invalid screenshot encoding"))?;
        if bytes.len() > 768 * 1024 || !bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            return Err(invalid("Invalid JPEG screenshot"));
        }
        output.content.push(rua_core::message::ContentPart::Text {
            text: format!(
                "Screenshot {}x{}; input coordinates use observation {}x{} pixels.",
                image.width, image.height, observation.width, observation.height
            ),
        });
        output.content.push(rua_core::message::ContentPart::Image {
            url: format!("data:{};base64,{}", image.mime_type, image.data),
            media_type: Some(image.mime_type),
            alt: Some("Phone observation".into()),
        });
    }
    Ok(output)
}

/// Model-visible budget for one observation, tied to the Agent's configured tool-output limit.
///
/// The generic compressor still runs afterwards, so staying under the configured limit is what
/// keeps a single-line JSON body from being byte-cut mid-object.
fn observation_budget(ctx: &ToolContext) -> usize {
    ctx.execution_output_max_tokens
        .min(DEFAULT_OBSERVATION_RESULT_TOKENS)
}

/// Serializes an action result, trimming its embedded post-action observation if needed.
///
/// Action responses embed a fresh observation, so they hit the same generic-compressor hazard
/// as `phone_observe` itself: a byte-cut single-line JSON body.
pub fn action_result(
    name: &str,
    mut value: serde_json::Value,
    max_tokens: usize,
) -> Result<ToolResult> {
    if let Some(observation) = value.get_mut("observation") {
        let mut parsed: rua_core::phone_use::PhoneObservation =
            serde_json::from_value(observation.clone()).map_err(|e| invalid(e.to_string()))?;
        fit_observation_to_model_budget(&mut parsed, max_tokens);
        *observation = serde_json::to_value(parsed).map_err(|e| invalid(e.to_string()))?;
    }
    Ok(ToolResult::text(
        name,
        serde_json::to_string(&value).map_err(|e| invalid(e.to_string()))?,
    ))
}

/// The generic result compressor cuts a single-line JSON body wherever the byte budget runs
/// out, which would hand the model malformed JSON. Observation payloads therefore trim their
/// own trailing nodes first (ADR-0010: tool-owned semantic pagination comes before the
/// execution-time token limit). Tokens are estimated the same way as the compressor.
pub fn fit_observation_to_model_budget(
    observation: &mut rua_core::phone_use::PhoneObservation,
    max_tokens: usize,
) {
    // Fast path: a payload that already fits is left byte-for-byte untouched.
    if serde_json::to_string(&*observation).is_ok_and(|json| {
        estimate_text_tokens(&json, rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR) <= max_tokens
    }) {
        return;
    }
    let original = std::mem::take(&mut observation.nodes);
    let was_truncated = observation.truncated;
    let size = |observation: &mut rua_core::phone_use::PhoneObservation, keep: usize| {
        observation.nodes = original[..keep].to_vec();
        serde_json::to_string(observation)
            .map(|json| estimate_text_tokens(&json, rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR))
            .unwrap_or(usize::MAX)
    };
    // Payload size is monotonic in the retained count, so binary search for the largest fit
    // instead of re-serializing once per dropped node.
    let (mut low, mut high) = (0usize, original.len());
    let mut keep = 0usize;
    while low <= high {
        let middle = (low + high) / 2;
        if size(observation, middle) <= max_tokens {
            keep = middle;
            low = middle + 1;
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    observation.nodes = original[..keep].to_vec();
    // Never clear a truncation flag the APK already set.
    observation.truncated = was_truncated || keep < original.len();
}

impl PhoneStatusTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "phone_status".into(),
            description: "Check this Android execution device's private Phone Use connection and separate accessibility/camera permissions. Does not operate the phone or grant permissions.".into(),
            parameters: serde_json::json!({"type":"object","properties":{}}),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }
}

#[async_trait]
impl Tool for PhoneStatusTool {
    fn name(&self) -> &str {
        "phone_status"
    }
    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }
    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        if args.as_object().is_none_or(|args| !args.is_empty()) {
            return Err(invalid("phone_status takes no arguments"));
        }
        let bridge = rua_core::phone_use::bridge().ok_or_else(|| {
            invalid("Phone Use is not connected on this Android execution device")
        })?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let operation = bridge.status(&cancel);
        let result = if let Some(mut receiver) = ctx.cancel.clone() {
            tokio::select! {
                biased;
                _ = async { if !*receiver.borrow() { let _ = receiver.changed().await; } } => {
                    cancel.cancel();
                    return Err(invalid("Phone Use request cancelled"));
                },
                result = operation => result,
            }
        } else {
            operation.await
        };
        let status = result.map_err(invalid)?;
        Ok(ToolResult::text(
            self.name(),
            serde_json::to_string(&status).map_err(|e| invalid(e.to_string()))?,
        ))
    }
}

fn invalid(message: impl Into<String>) -> RuaError {
    RuaError::Validation {
        message: message.into(),
    }
}

fn archive_phone_image(
    ctx: &ToolContext,
    bytes: &[u8],
    source: &str,
) -> Result<rua_session::display_media::DisplayMedia> {
    let storage = ctx
        .conversation_store_dir
        .as_ref()
        .ok_or_else(|| invalid("Missing Chat media storage root"))?;
    let device = ctx
        .metadata
        .get("execution_device_id")
        .or_else(|| ctx.metadata.get("device_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("Missing execution device identity for phone media"))?;
    rua_session::display_media::DisplayMediaArchive::store_image_bytes(
        storage,
        bytes,
        "image/jpeg",
        device,
        source,
    )
    .map_err(|e| invalid(format!("Phone image could not be stored: {e}")))
}
