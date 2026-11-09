use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::state::AppState;

const TOKEN_EXPIRY_SECONDS: i64 = 7 * 24 * 3600;
const TOKEN_EXPIRY_MAX: i64 = 100 * 365 * 24 * 3600;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct AuthData {
    #[serde(default)]
    user: Option<UserRecord>,
    #[serde(default)]
    jwt_secret: Option<String>,
    #[serde(default)]
    revoked_tokens: Vec<String>,
    #[serde(default)]
    revoked_tokens_meta: HashMap<String, i64>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UserRecord {
    username: String,
    password_hash: String,
    password_salt: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TokenPayload {
    sub: String,
    exp: i64,
    iat: i64,
    jti: String,
}

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let block_size = 64;
    let mut k = vec![0u8; block_size];

    if key.len() > block_size {
        let hash = Sha256::digest(key);
        k[..32].copy_from_slice(&hash);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();

    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(inner_hash);
    let result = outer.finalize();

    let mut output = [0u8; 32];
    output.copy_from_slice(&result);
    output
}

pub fn hash_password(password: &str, salt: &str) -> String {
    let input = format!("{salt}{password}");
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(hash)
}

pub fn generate_salt() -> String {
    hex::encode(uuid::Uuid::new_v4().as_bytes())
}

fn generate_jwt_secret() -> String {
    let mut secret = hex::encode(uuid::Uuid::new_v4().as_bytes());
    secret.push_str(&hex::encode(uuid::Uuid::new_v4().as_bytes()));
    secret
}

pub fn verify_password(password: &str, stored_hash: &str, salt: &str) -> bool {
    let computed = hash_password(password, salt);
    constant_time_eq(computed.as_bytes(), stored_hash.as_bytes())
}

#[derive(Debug)]
pub struct AuthService {
    data: RwLock<AuthData>,
    auth_file: Option<PathBuf>,

    static_token: Option<String>,

    force_enabled: bool,
}

impl AuthService {
    pub fn new_disabled() -> Self {
        Self {
            data: RwLock::new(AuthData::default()),
            auth_file: None,
            static_token: None,
            force_enabled: false,
        }
    }

    pub fn new_with_token(token: String) -> Self {
        let salt = generate_salt();
        let pw_hash = hash_password(&token, &salt);
        let data = AuthData {
            user: Some(UserRecord {
                username: "admin".to_string(),
                password_hash: pw_hash,
                password_salt: salt,
                extra: serde_json::Map::new(),
            }),
            ..Default::default()
        };
        Self {
            data: RwLock::new(data),
            auth_file: None,
            static_token: Some(token),
            force_enabled: true,
        }
    }

    pub fn from_secret_dir(secret_dir: &Path) -> Self {
        let auth_file = secret_dir.join("auth.json");
        let data = Self::load_from_disk(&auth_file);
        Self {
            data: RwLock::new(data),
            auth_file: Some(auth_file),
            static_token: None,
            force_enabled: false,
        }
    }

    fn load_from_disk(path: &Path) -> AuthData {
        if path.is_file() {
            match std::fs::read_to_string(path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(data) => return data,
                    Err(e) => {
                        tracing::error!(path = %path.display(), error = %e, "failed to parse auth.json");
                    }
                },
                Err(e) => {
                    tracing::error!(path = %path.display(), error = %e, "failed to read auth.json");
                }
            }
        }
        AuthData::default()
    }

    fn save_to_disk(&self) {
        let Some(ref path) = self.auth_file else {
            return;
        };
        let guard = self
            .data
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let data = guard.clone();
        drop(guard);

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        match serde_json::to_string_pretty(&data) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, &json) {
                    tracing::error!(path = %path.display(), error = %e, "failed to write auth.json");
                } else {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ =
                            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to serialize auth data");
            }
        }
    }

    fn ensure_jwt_secret(&self) -> String {
        {
            let guard = self
                .data
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(ref secret) = guard.jwt_secret {
                return secret.clone();
            }
        }
        let secret = generate_jwt_secret();
        {
            let mut guard = self
                .data
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.jwt_secret = Some(secret.clone());
        }
        self.save_to_disk();
        secret
    }

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    pub fn is_active(&self) -> bool {
        self.force_enabled || self.auth_file.is_some() || self.is_enabled()
    }

    pub fn is_enabled(&self) -> bool {
        if self.force_enabled {
            return true;
        }
        let val = std::env::var("RUA_AUTH_ENABLED").unwrap_or_default();
        matches!(val.trim().to_lowercase().as_str(), "true" | "1" | "yes")
    }

    pub fn has_registered_users(&self) -> bool {
        let guard = self
            .data
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.user.is_some()
    }

    pub fn register_user(
        &self,
        username: &str,
        password: &str,
        expires_in: Option<i64>,
    ) -> Option<String> {
        {
            let guard = self
                .data
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard.user.is_some() {
                return None;
            }
        }

        let salt = generate_salt();
        let pw_hash = hash_password(password, &salt);
        let user = UserRecord {
            username: username.to_string(),
            password_hash: pw_hash,
            password_salt: salt,
            extra: serde_json::Map::new(),
        };

        {
            let mut guard = self
                .data
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard.user.is_some() {
                return None;
            }
            guard.user = Some(user);
        }
        self.save_to_disk();
        tracing::info!(username, "user registered");
        Some(self.create_token(username, expires_in))
    }

    pub fn authenticate(
        &self,
        username: &str,
        password: &str,
        expires_in: Option<i64>,
    ) -> Option<String> {
        let guard = self
            .data
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let user = guard.user.as_ref()?;

        if user.username != username {
            return None;
        }
        if !verify_password(password, &user.password_hash, &user.password_salt) {
            return None;
        }
        drop(guard);
        Some(self.create_token(username, expires_in))
    }

    fn resolve_expiry(expires_in: Option<i64>) -> i64 {
        match expires_in {
            None => TOKEN_EXPIRY_SECONDS,
            Some(s) if s <= 0 => TOKEN_EXPIRY_MAX,
            Some(s) => s.min(TOKEN_EXPIRY_MAX),
        }
    }

    pub fn update_credentials(
        &self,
        current_password: &str,
        new_username: Option<&str>,
        new_password: Option<&str>,
        expires_in: Option<i64>,
    ) -> Option<String> {
        let (stored_hash, stored_salt, old_username) = {
            let guard = self
                .data
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let user = guard.user.as_ref()?;
            (
                user.password_hash.clone(),
                user.password_salt.clone(),
                user.username.clone(),
            )
        };

        if !verify_password(current_password, &stored_hash, &stored_salt) {
            return None;
        }

        let final_username;
        {
            let mut guard = self
                .data
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let user = guard.user.as_mut()?;

            if let Some(name) = new_username {
                let name = name.trim();
                if !name.is_empty() {
                    user.username = name.to_string();
                }
            }

            let rotate_secret = if let Some(pw) = new_password {
                let salt = generate_salt();
                user.password_hash = hash_password(pw, &salt);
                user.password_salt = salt;
                true
            } else {
                false
            };

            final_username = user.username.clone();

            if rotate_secret {
                rua_core::phone_use::revoke_native_token();
                let new_secret = generate_jwt_secret();
                guard.jwt_secret = Some(new_secret);
            }
        }
        self.save_to_disk();
        tracing::info!(
            old_username = %old_username,
            new_username = %final_username,
            "credentials updated"
        );
        Some(self.create_token(&final_username, expires_in))
    }

    pub fn create_token(&self, username: &str, expires_in: Option<i64>) -> String {
        let secret = self.ensure_jwt_secret();
        let now = Self::now_secs();
        let expiry = Self::resolve_expiry(expires_in);
        let payload = TokenPayload {
            sub: username.to_string(),
            exp: now + expiry,
            iat: now,
            jti: uuid::Uuid::new_v4().to_string(),
        };
        let payload_json = serde_json::to_string(&payload).unwrap_or_default();
        let payload_hex = hex::encode(payload_json.as_bytes());
        let sig = hmac_sha256(secret.as_bytes(), payload_hex.as_bytes());
        let sig_hex = hex::encode(sig);
        format!("{payload_hex}.{sig_hex}")
    }

    pub fn verify_token(&self, token: &str) -> Option<String> {
        if rua_core::phone_use::accepts_native_token(token) {
            return Some("mobile-native".into());
        }
        if let Some(ref expected) = self.static_token {
            if constant_time_eq(token.as_bytes(), expected.as_bytes()) {
                return Some("static-user".to_string());
            }
            return None;
        }

        let parts: Vec<&str> = token.splitn(2, '.').collect();
        if parts.len() != 2 {
            return None;
        }
        let (payload_hex, sig_hex) = (parts[0], parts[1]);

        let secret = {
            let guard = self
                .data
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.jwt_secret.clone()?
        };

        let expected_sig = hmac_sha256(secret.as_bytes(), payload_hex.as_bytes());
        let expected_hex = hex::encode(expected_sig);
        if !constant_time_eq(sig_hex.as_bytes(), expected_hex.as_bytes()) {
            return None;
        }

        let payload_bytes = hex::decode(payload_hex).ok()?;
        let payload: TokenPayload = serde_json::from_slice(&payload_bytes).ok()?;

        if payload.exp < Self::now_secs() {
            return None;
        }

        {
            let guard = self
                .data
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard.revoked_tokens_meta.contains_key(&payload.jti) {
                return None;
            }
        }

        Some(payload.sub)
    }

    pub fn revoke_token(&self, token: &str) -> bool {
        if rua_core::phone_use::accepts_native_token(token) {
            rua_core::phone_use::revoke_native_token();
            return true;
        }
        let parts: Vec<&str> = token.splitn(2, '.').collect();
        if parts.len() != 2 {
            return false;
        }
        let payload_hex = parts[0];

        let payload_bytes = match hex::decode(payload_hex) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let payload: TokenPayload = match serde_json::from_slice(&payload_bytes) {
            Ok(p) => p,
            Err(_) => return false,
        };

        {
            let mut guard = self
                .data
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !guard.revoked_tokens_meta.contains_key(&payload.jti) {
                guard
                    .revoked_tokens_meta
                    .insert(payload.jti.clone(), payload.exp);
                guard.revoked_tokens.push(payload.jti.clone());
            }
        }
        self.save_to_disk();
        tracing::info!(jti = %&payload.jti[..8.min(payload.jti.len())], "token revoked");
        true
    }

    pub fn revoke_all_tokens(&self) -> bool {
        rua_core::phone_use::revoke_native_token();
        let new_secret = generate_jwt_secret();
        {
            let mut guard = self
                .data
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.jwt_secret = Some(new_secret);
            guard.revoked_tokens.clear();
            guard.revoked_tokens_meta.clear();
        }
        self.save_to_disk();
        tracing::info!("all tokens revoked (JWT secret rotated)");
        true
    }
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn is_public_api_path(path: &str) -> bool {
    let path = path.strip_prefix("/api").unwrap_or(path);

    if path == "/auth/login" || path == "/auth/register" || path == "/auth/status" {
        return true;
    }
    if path == "/version" || path == "/doctor/version" {
        return true;
    }
    if path == "/mcp/oauth/callback" {
        return true;
    }
    if path.starts_with("/providers/") && path.ends_with("/oauth/callback") {
        return true;
    }
    if path.starts_with("/doctor/") {
        return true;
    }
    if path.starts_with("/channel/") || path.starts_with("/channels/") {
        return true;
    }
    if path == "/dingtalk/webhook" {
        return true;
    }
    false
}

pub fn is_api_path(path: &str) -> bool {
    path.starts_with("/api/")
}

pub fn extract_token(req: &Request<Body>) -> Option<String> {
    if let Some(auth_header) = req.headers().get("authorization") {
        if let Ok(value) = auth_header.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                let token = token.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(token) = pair.strip_prefix("token=") {
                let token = token.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    None
}

pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    match authentication_error(&state, &req) {
        Some(error) => error,
        None => next.run(req).await,
    }
}

pub(crate) fn authentication_error(state: &AppState, req: &Request<Body>) -> Option<Response> {
    let path = req.uri().path().to_string();

    let mobile = state.profile == crate::profiles::ServerProfile::MobileApk;
    if (mobile
        && matches!(
            path.strip_prefix("/api").unwrap_or(&path),
            "/auth/login"
                | "/auth/status"
                | "/version"
                | "/doctor/version"
                | "/healthz"
                | "/readyz"
        ))
        || (!mobile && is_public_api_path(&path))
    {
        return None;
    }

    if !mobile && !state.auth.is_active() {
        return None;
    }

    if !mobile && is_allow_no_auth_host(state, req) {
        return None;
    }

    let token = if mobile {
        req.headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    } else {
        extract_token(req)
    };

    match token {
        None => {
            if mobile || state.auth.is_enabled() {
                tracing::debug!(%path, "auth rejected: no token");
                Some(json_error(
                    StatusCode::UNAUTHORIZED,
                    "Authentication required",
                ))
            } else {
                None
            }
        }
        Some(token) => {
            if state.auth.verify_token(&token).is_some() {
                None
            } else {
                tracing::debug!(%path, "auth rejected: invalid or expired token");
                Some(json_error(
                    StatusCode::UNAUTHORIZED,
                    "Invalid or expired token",
                ))
            }
        }
    }
}

fn json_error(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({ "detail": message });
    (status, Json(body)).into_response()
}

fn is_allow_no_auth_host(state: &AppState, req: &Request<Body>) -> bool {
    let Some(client_ip) = resolve_client_ip(req) else {
        return false;
    };
    let store = rua_security::SecurityConfigStore::new(state.rua_root.clone());
    match store.get_allow_no_auth_hosts() {
        Ok(hosts) => hosts.iter().any(|host| {
            host.parse::<IpAddr>()
                .is_ok_and(|allowed| allowed == client_ip)
        }),
        Err(err) => {
            tracing::warn!(error = %err, "failed to read allow-no-auth host config");
            false
        }
    }
}

fn resolve_client_ip(req: &Request<Body>) -> Option<IpAddr> {
    if let Some(forwarded_for) = req.headers().get("x-forwarded-for") {
        if let Ok(value) = forwarded_for.to_str() {
            if let Some(first) = value.split(',').next().map(str::trim) {
                if let Ok(ip) = first.parse::<IpAddr>() {
                    return Some(ip);
                }
            }
        }
    }

    if let Some(real_ip) = req.headers().get("x-real-ip") {
        if let Ok(value) = real_ip.to_str() {
            if let Ok(ip) = value.trim().parse::<IpAddr>() {
                return Some(ip);
            }
        }
    }

    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}
