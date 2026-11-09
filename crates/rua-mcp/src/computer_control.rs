//! One bounded input sequence owns the physical desktop. No task scheduler.
use crate::call_scope::CallScope;
use mcp_manager::error::{McpError, McpResult};
use mcp_manager::types::{McpCallContext, McpServerId, McpToolInfo};
use serde_json::{json, Value};
use tokio::sync::{Mutex, MutexGuard};

pub const INTERACT: &str = "interact";
const SERVER: &str = super::bundled_computer::SERVER_ID;

#[derive(Default)]
struct ControlState {
    uncertain: std::collections::BTreeSet<String>,
}
static CONTROL: Mutex<ControlState> = Mutex::const_new(ControlState {
    uncertain: std::collections::BTreeSet::new(),
});
type RecoveryState = std::collections::BTreeMap<String, Option<String>>;
static RECOVERY: std::sync::LazyLock<tokio::sync::watch::Sender<RecoveryState>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::Sender::new(RecoveryState::new()));

pub fn recovery_requests() -> tokio::sync::watch::Receiver<RecoveryState> {
    RECOVERY.subscribe()
}

/// Only the physical desktop owner may clear uncertainty, after its old executor exits.
pub async fn recover<F: std::future::Future<Output = Result<(), String>>>(
    generation: &str,
    recovery: F,
) -> Result<(), String> {
    let mut state = CONTROL.lock().await;
    if !state.uncertain.contains(generation) {
        return Ok(());
    }
    if let Err(error) = recovery.await {
        RECOVERY.send_if_modified(|requests| {
            if requests.get(generation).and_then(Option::as_ref) == Some(&error) {
                return false;
            }
            requests.insert(generation.into(), Some(error.clone()));
            true
        });
        return Err(error);
    }
    state.uncertain.remove(generation);
    RECOVERY.send_modify(|requests| {
        requests.remove(generation);
    });
    Ok(())
}

pub struct ControlLease<'a> {
    state: MutexGuard<'a, ControlState>,
    generation: String,
}

impl Drop for ControlLease<'_> {
    fn drop(&mut self) {
        if self.state.uncertain.contains(&self.generation) {
            RECOVERY.send_modify(|requests| {
                requests.entry(self.generation.clone()).or_insert(None);
            });
        }
    }
}
impl ControlLease<'_> {
    pub fn submitted(&mut self) {
        self.state.uncertain.insert(self.generation.clone());
    }
    pub fn settled(&mut self) {
        self.state.uncertain.remove(&self.generation);
    }
}

pub(crate) async fn acquire<'a>(
    generation: &str,
    scope: &CallScope,
    ctx: &McpCallContext,
) -> McpResult<ControlLease<'a>> {
    let state = match CONTROL.try_lock() {
        Ok(state) => state,
        Err(_) => {
            if RECOVERY.borrow().contains_key(generation) {
                return Err(recovery_error(generation));
            }
            if let Some(sender) = &ctx.event_tx {
                let id = ctx
                    .metadata
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let _ = sender.try_send(agent_types::AgentEvent::ToolResultDelta {
                    id: id.into(),
                    name: Some(format!("mcp__{SERVER}__{INTERACT}")),
                    output_delta:
                        "Waiting for this device's desktop control; Stop cancels this wait.\n"
                            .into(),
                    metadata: json!({"state":"waiting_for_desktop_control"}),
                });
            }
            scope.wait(CONTROL.lock()).await?
        }
    };
    if state.uncertain.contains(generation) {
        return Err(recovery_error(generation));
    }
    Ok(ControlLease {
        state,
        generation: generation.into(),
    })
}

pub async fn requires_reset(generation: &str) -> bool {
    RECOVERY.borrow().contains_key(generation)
}

fn recovery_error(generation: &str) -> McpError {
    if let Some(Some(error)) = RECOVERY.borrow().get(generation) {
        return invalid(&format!("The previous desktop input has an unknown outcome. Executor recovery failed: {error}. The Runtime will retry; no action was replayed."));
    }
    invalid("The previous desktop input has an unknown outcome. The desktop Runtime is recovering its executor; observe again after recovery. No action was replayed.")
}

fn invalid(message: &str) -> McpError {
    McpError::ConfigInvalid {
        message: message.into(),
    }
}

pub fn interaction_tool(tools: &[McpToolInfo]) -> Option<McpToolInfo> {
    let variants: Vec<_> = ["click", "type_text", "press_key", "scroll"].iter().filter_map(|name| {
        let tool=tools.iter().find(|tool| tool.name==*name)?;
        let mut schema=tool.input_schema.clone();
        if let Some(props)=schema["properties"].as_object_mut() { props.retain(|key,_| action_properties(name).contains(&key.as_str())); }
        if let Some(required)=schema["required"].as_array_mut() { required.retain(|v| v.as_str().is_some_and(|v|action_properties(name).contains(&v))); }
        schema["additionalProperties"]=json!(false);
        Some(json!({"type":"object","required":["tool","arguments"],"additionalProperties":false,"properties":{"tool":{"type":"string","enum":[name]},"arguments":schema}}))
    }).collect();
    if variants.len() != 4 {
        return None;
    }
    let schema = json!({"type":"object","required":["pid","window_id","observation_id","actions"],"additionalProperties":false,"properties":{
        "pid":{"type":"integer","minimum":1},"window_id":{"type":"integer","minimum":1},
        "observation_id":{"type":"string","description":"Rua observation_id from this Chat's latest get_window_state. Valid for two minutes and one input sequence."},
        "actions":{"type":"array","minItems":1,"maxItems":12,"items":{"oneOf":variants}},
        "include_screenshot":{"type":"boolean","default":true,"description":"Return a final observation with image; false returns accessibility text only."}
    }});
    let description="Execute one bounded desktop input sequence on exactly the observed pid/window_id. Actions retain their real tool names and arguments for authorization. The execution device is held for the whole sequence; waiting is cancellable. No scope=desktop, arbitrary commands, session ids or output files. Stop prevents subsequent steps; a submitted step may already have happened. Always inspect the final observation and action effects; success does not guarantee UI effect. Unknown input is never replayed. Rua automatically recovers its executor; observe again after recovery before new input.".to_string();
    Some(McpToolInfo {
        server_id: McpServerId::new(SERVER),
        name: INTERACT.into(),
        description: description.clone(),
        input_schema: schema.clone(),
        raw: json!({"name":INTERACT,"description":description,"inputSchema":schema,"ruaBuiltinComputerInteraction":true,"annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":true}}),
    })
}

fn action_properties(name: &str) -> &'static [&'static str] {
    match name {
        "click" => &[
            "x",
            "y",
            "element_token",
            "snapshot_id",
            "button",
            "delivery_mode",
        ],
        "type_text" => &[
            "text",
            "x",
            "y",
            "element_token",
            "snapshot_id",
            "delivery_mode",
        ],
        "press_key" => &[
            "key",
            "modifiers",
            "x",
            "y",
            "element_token",
            "snapshot_id",
            "delivery_mode",
        ],
        "scroll" => &[
            "direction",
            "by",
            "amount",
            "x",
            "y",
            "element_token",
            "snapshot_id",
            "delivery_mode",
        ],
        _ => &[],
    }
}

/// Public for the bridge's ordinary Tool Guard preflight. It returns the exact
/// names and visible target arguments that will be sent to the native driver.
pub fn action_calls(args: &Value) -> McpResult<Vec<(String, Value)>> {
    let top = args
        .as_object()
        .ok_or_else(|| invalid("Computer interaction arguments must be an object"))?;
    if top.keys().any(|key| {
        ![
            "pid",
            "window_id",
            "observation_id",
            "actions",
            "include_screenshot",
        ]
        .contains(&key.as_str())
    }) {
        return Err(invalid("Unknown Computer interaction argument"));
    }
    if args["pid"]
        .as_u64()
        .is_none_or(|n| n == 0 || n > u32::MAX as u64)
        || args["window_id"].as_u64().is_none_or(|n| n == 0)
        || args["observation_id"].as_str().is_none_or(str::is_empty)
    {
        return Err(invalid(
            "Computer interaction requires an observed positive pid/window_id and observation_id",
        ));
    }
    if args
        .get("include_screenshot")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err(invalid("include_screenshot must be boolean"));
    }
    let actions = args["actions"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 12)
        .ok_or_else(|| invalid("Computer interaction needs 1 to 12 actions"))?;
    actions
        .iter()
        .map(|step| {
            let step = step
                .as_object()
                .ok_or_else(|| invalid("Each action must be an object"))?;
            if step
                .keys()
                .any(|key| !["tool", "arguments"].contains(&key.as_str()))
            {
                return Err(invalid("Unknown action field"));
            }
            let name = step
                .get("tool")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("Action needs tool name"))?;
            let allowed = action_properties(name);
            if allowed.is_empty() {
                return Err(invalid(
                    "Only click, type_text, press_key and scroll are supported",
                ));
            }
            let mut input = step
                .get("arguments")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| invalid("Action needs arguments"))?;
            if input.keys().any(|key| !allowed.contains(&key.as_str())) {
                return Err(invalid(
                    "Action cannot choose another target, session, command or output path",
                ));
            }
            let x = input.get("x");
            let y = input.get("y");
            if x.is_some() != y.is_some()
                || [x, y]
                    .iter()
                    .flatten()
                    .any(|v| v.as_f64().is_none_or(|v| !(0.0..=100_000.0).contains(&v)))
            {
                return Err(invalid("Pixel target requires nonnegative x and y"));
            }
            if input.contains_key("element_token") && x.is_some() {
                return Err(invalid("Choose element_token or pixels, not both"));
            }
            if input
                .get("element_token")
                .is_some_and(|v| v.as_str().is_none_or(str::is_empty))
            {
                return Err(invalid("Invalid element token"));
            }
            if input
                .get("delivery_mode")
                .is_some_and(|v| !matches!(v.as_str(), Some("background" | "foreground")))
            {
                return Err(invalid("Invalid delivery mode"));
            }
            match name {
                "click" if x.is_none() && !input.contains_key("element_token") => {
                    return Err(invalid(
                        "Click requires an observed pixel or element target",
                    ))
                }
                "type_text"
                    if input
                        .get("text")
                        .and_then(Value::as_str)
                        .is_none_or(|s| s.is_empty() || s.chars().count() > 200) =>
                {
                    return Err(invalid(
                        "One input step accepts 1 to 200 characters; observe between larger inputs",
                    ))
                }
                "press_key"
                    if input
                        .get("key")
                        .and_then(Value::as_str)
                        .is_none_or(|s| s.is_empty() || s.len() > 20) =>
                {
                    return Err(invalid("Invalid key name"))
                }
                "scroll"
                    if !matches!(
                        input.get("direction").and_then(Value::as_str),
                        Some("up" | "down" | "left" | "right")
                    ) =>
                {
                    return Err(invalid("Invalid scroll direction"))
                }
                _ => {}
            }
            if input
                .get("button")
                .is_some_and(|v| !matches!(v.as_str(), Some("left" | "right" | "middle")))
            {
                return Err(invalid("Invalid mouse button"));
            }
            if input
                .get("by")
                .is_some_and(|v| !matches!(v.as_str(), Some("line" | "page")))
            {
                return Err(invalid("Invalid scroll granularity"));
            }
            if input
                .get("amount")
                .is_some_and(|v| v.as_u64().is_none_or(|n| !(1..=50).contains(&n)))
            {
                return Err(invalid("Invalid scroll amount"));
            }
            if input.get("modifiers").is_some_and(|v| {
                v.as_array().is_none_or(|a| {
                    a.len() > 4
                        || a.iter().any(|v| {
                            !matches!(
                                v.as_str(),
                                Some("cmd" | "shift" | "option" | "alt" | "ctrl" | "fn")
                            )
                        })
                })
            }) {
                return Err(invalid("Invalid key modifiers"));
            }
            input.insert("pid".into(), args["pid"].clone());
            input.insert("window_id".into(), args["window_id"].clone());
            Ok((name.into(), Value::Object(input)))
        })
        .collect()
}

pub fn window_fingerprint(window: &Value) -> Value {
    json!({"pid":window["pid"],"window_id":window["window_id"],"app_name":window["app_name"],"title":window["title"],"bounds":window["bounds"]})
}

/// Bound native synthesis to one Unicode scalar. The shared daemon does not
/// expose legacy per-call cancellation; do not submit the remaining characters
/// until the current reply and a fresh cancellation/target check.
pub(crate) fn dispatch_steps(actions: Vec<(String, Value)>) -> Vec<(String, Value)> {
    actions
        .into_iter()
        .flat_map(|(name, input)| {
            if name != "type_text" {
                return vec![(name, input)];
            }
            input["text"]
                .as_str()
                .expect("validated text")
                .chars()
                .enumerate()
                .map(|(index, ch)| {
                    let mut fragment = input.clone();
                    if index > 0 {
                        let fields = fragment.as_object_mut().expect("validated object");
                        fields.remove("x");
                        fields.remove("y");
                    }
                    fragment["text"] = json!(ch.to_string());
                    (name.clone(), fragment)
                })
                .collect()
        })
        .collect()
}
