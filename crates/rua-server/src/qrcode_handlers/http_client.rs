use std::time::Duration;

use reqwest::Client;

pub(super) fn qrcode_http_client(timeout: Duration) -> Client {
    build_client(Client::builder().timeout(timeout))
}

pub(super) fn qrcode_redirect_http_client(
    timeout: Duration,
    policy: reqwest::redirect::Policy,
) -> Client {
    build_client(Client::builder().timeout(timeout).redirect(policy))
}

fn build_client(builder: reqwest::ClientBuilder) -> Client {
    rua_http::build_client(builder).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "failed to build QR code HTTP client; using reqwest default client");
            Client::new()
        })
}
