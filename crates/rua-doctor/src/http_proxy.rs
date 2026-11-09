use std::time::Duration;

pub(crate) fn blocking_client_for_url(
    url: &str,
    timeout: Duration,
) -> Result<reqwest::blocking::Client, rua_http::HttpClientError> {
    let mut builder = reqwest::blocking::Client::builder().timeout(timeout);
    if should_bypass_proxy_for_url(url) {
        builder = builder.no_proxy();
    }
    rua_http::build_blocking_client(builder)
}

fn should_bypass_proxy_for_url(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    is_loopback_host(host)
}

fn is_loopback_host(host: &str) -> bool {
    let normalized_host = host
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']');
    if normalized_host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    normalized_host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}
