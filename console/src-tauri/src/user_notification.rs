use serde::Deserialize;
use tauri::Emitter;

pub const USER_NOTIFICATION_ACTIVATED_EVENT: &str = "rua:user-notification-activated";

#[derive(Debug, Clone, Deserialize)]
pub struct UserNotificationSender {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserNotificationRequest {
    pub target_device_id: String,
    pub sender: UserNotificationSender,
    pub title: String,
    pub body: Option<String>,
    pub rua_uri: Option<String>,
}

#[tauri::command]
pub fn show_user_notification(
    app: tauri::AppHandle,
    request: UserNotificationRequest,
) -> Result<(), String> {
    validate_request(&request)?;

    let mut notification = notify_rust::Notification::new();
    notification
        .appname("Rua")
        .summary(&request.title)
        .body(&notification_body(&request));
    #[cfg(target_os = "windows")]
    notification.app_id(&app.config().identifier);

    let handle = notification
        .show()
        .map_err(|error| format!("failed to show notification: {error}"))?;
    let uri = request
        .rua_uri
        .as_deref()
        .and_then(valid_rua_uri)
        .map(str::to_owned);
    std::thread::spawn(move || {
        let _ = handle.wait_for_response(move |response: &notify_rust::NotificationResponse| {
            if !matches!(
                response,
                notify_rust::NotificationResponse::Default
                    | notify_rust::NotificationResponse::Action(_)
            ) {
                return;
            }
            crate::tray::show_main_window(&app);
            if let Some(uri) = uri {
                let _ = app.emit_to("main", USER_NOTIFICATION_ACTIVATED_EVENT, uri);
            }
        });
    });
    Ok(())
}

fn validate_request(request: &UserNotificationRequest) -> Result<(), String> {
    if request.target_device_id.trim().is_empty()
        || request.sender.id.trim().is_empty()
        || request.sender.name.trim().is_empty()
        || request.title.trim().is_empty()
    {
        return Err("notification identity and title must not be empty".to_string());
    }
    Ok(())
}

#[doc(hidden)]
pub fn notification_body(request: &UserNotificationRequest) -> String {
    match request.body.as_deref().filter(|body| !body.is_empty()) {
        Some(body) => format!("{}\n{body}", request.sender.name),
        None => request.sender.name.clone(),
    }
}

#[doc(hidden)]
pub fn valid_rua_uri(raw: &str) -> Option<&str> {
    if raw.trim() != raw || raw.chars().any(char::is_control) {
        return None;
    }
    let parsed = tauri::Url::parse(raw).ok()?;
    (parsed.scheme() == "rua"
        && parsed.host_str().is_some_and(|host| !host.is_empty())
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.port().is_none())
    .then_some(raw)
}
