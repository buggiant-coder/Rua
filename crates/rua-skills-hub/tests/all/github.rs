use rua_skills_hub::client::{GithubCache, HubClient, RETRYABLE_HTTP_STATUS};
use rua_skills_hub::types::{BundleSource, GithubSpec, HubError};
use rua_skills_hub::{resolve_bundle, resolve_bundle_source};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[test]
fn retryable_status_matches_brief() {
    let expected = [408u16, 409, 425, 429, 500, 502, 503, 504];
    let actual: Vec<u16> = RETRYABLE_HTTP_STATUS.to_vec();
    assert_eq!(actual, expected.to_vec());
}

#[test]
fn resolve_github_url_yields_github_spec() {
    let source = resolve_bundle_source("https://github.com/owner/repo").expect("ok");
    match source {
        BundleSource::GitHub(GithubSpec {
            owner,
            repo,
            branch,
            path,
        }) => {
            assert_eq!(owner, "owner");
            assert_eq!(repo, "repo");
            assert_eq!(branch, "");
            assert_eq!(path, "");
        }
        other => panic!("expected GitHub, got {other:?}"),
    }

    let source =
        resolve_bundle_source("https://github.com/foo/bar/tree/main/skills/baz").expect("ok");
    match source {
        BundleSource::GitHub(GithubSpec {
            owner,
            repo,
            branch,
            path,
        }) => {
            assert_eq!(owner, "foo");
            assert_eq!(repo, "bar");
            assert_eq!(branch, "main");
            assert_eq!(path, "skills/baz");
        }
        other => panic!("expected GitHub, got {other:?}"),
    }
}

#[test]
fn github_cache_returns_value_within_ttl_then_evicts() {
    let cache = GithubCache::new();
    cache.put(
        "default_branch",
        "owner/repo",
        serde_json::Value::String("main".into()),
        Duration::from_secs(60),
    );
    let got = cache
        .get("default_branch", "owner/repo")
        .expect("cached value present");
    assert_eq!(got.as_str(), Some("main"));

    let cache = GithubCache::new();
    cache.put(
        "default_branch",
        "owner/repo",
        serde_json::Value::String("main".into()),
        Duration::from_millis(0),
    );
    std::thread::sleep(Duration::from_millis(5));
    assert!(cache.get("default_branch", "owner/repo").is_none());
}

#[test]
fn rate_limit_conversion_only_fires_on_github_api_host() {
    use rua_skills_hub::client::{convert_github_rate_limit, is_github_api};

    assert!(is_github_api("https://api.github.com/repos/foo/bar"));
    assert!(!is_github_api("https://github.com/foo/bar"));
    assert!(!is_github_api("https://example.com/api"));

    let url = "https://api.github.com/repos/foo/bar";
    let body = "API rate limit exceeded for user 12345";
    let err = convert_github_rate_limit(url, 403, body).expect("rate limit");
    matches!(err, HubError::RateLimit)
        .then_some(())
        .expect("must be RateLimit");

    assert!(convert_github_rate_limit(url, 500, body).is_none());

    assert!(convert_github_rate_limit("https://example.com/x", 403, body).is_none());

    assert!(convert_github_rate_limit(url, 403, "permission denied").is_none());
}

#[tokio::test]
async fn github_cache_dedup_short_circuits_repeat_calls() {
    let server = MockServer::start().await;
    let counter = Arc::new(AtomicUsize::new(0));

    struct Counting {
        counter: Arc<AtomicUsize>,
        body: String,
    }
    impl Respond for Counting {
        fn respond(&self, _req: &Request) -> ResponseTemplate {
            self.counter.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_string(self.body.clone())
        }
    }

    Mock::given(method("GET"))
        .and(path("/cached"))
        .respond_with(Counting {
            counter: counter.clone(),
            body: "{}".to_string(),
        })
        .mount(&server)
        .await;

    let client = HubClient::new().expect("client");
    let url = format!("{}/cached", server.uri());

    let body = rua_skills_hub::client::http_get_text_with_retry(&client, &url, None)
        .await
        .expect("first fetch");
    assert_eq!(body, "{}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    client
        .github_cache()
        .put("content", &url, parsed, Duration::from_secs(60));

    let cached = client
        .github_cache()
        .get("content", &url)
        .expect("cache hit");
    assert_eq!(cached, serde_json::json!({}));
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "cache hit must not trigger a second HTTP request",
    );
}

#[tokio::test]
async fn direct_source_fetches_raw_json() {
    let server = MockServer::start().await;
    let body = serde_json::json!({"name": "x", "files": {"SKILL.md": "---\nname: x\ndescription: ok\n---\nbody"}});
    Mock::given(method("GET"))
        .and(path("/raw"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let client = HubClient::new().expect("client");
    let url = format!("{}/raw", server.uri());
    let (normalized, source_url) = resolve_bundle(&client, &url, "", None)
        .await
        .expect("normalize ok");
    assert_eq!(normalized.name, "x");
    assert!(normalized.content.contains("name: x"));
    assert_eq!(source_url, url);
}
