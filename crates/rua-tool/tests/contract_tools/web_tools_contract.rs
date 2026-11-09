use std::collections::HashMap;

use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::extended::web_fetch::WebFetchTool;
use rua_tool::extended::web_search::{
    BraveSearchProvider, ExaMcpSearchProvider, ExaSearchProvider, SearchProvider,
    SerperSearchProvider, TavilySearchProvider, WebSearchTool,
};
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_ctx(root: &TempDir, metadata: HashMap<String, serde_json::Value>) -> ToolContext {
    ToolContext {
        agent_id: "agent".into(),
        session_id: "session".into(),
        working_dir: root.path().to_path_buf(),
        metadata,
        cancel: None,
        task_runtime: None,
        ..Default::default()
    }
}

fn text(result: &rua_core::tool::ToolResult) -> String {
    result.text_content()
}

#[tokio::test]
async fn web_fetch_native_sends_browser_navigation_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string("<main><h1>Browser headers accepted</h1></main>"),
        )
        .mount(&server)
        .await;

    let root = tempfile::tempdir().expect("tempdir");
    let result = WebFetchTool::new()
        .call(
            json!({
                "url": format!("{}/page", server.uri()),
                "reader": "native",
                "allow_private": true
            }),
            make_ctx(&root, HashMap::new()),
        )
        .await
        .expect("native web fetch");

    assert!(!result.is_error, "got: {}", text(&result));
    assert!(text(&result).contains("Browser headers accepted"));

    let requests = server
        .received_requests()
        .await
        .expect("request recording is enabled");
    let request = requests.first().expect("one web fetch request");
    assert_eq!(
        request
            .headers
            .get("user-agent")
            .and_then(|value| value.to_str().ok()),
        Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36")
    );
    assert_eq!(
        request
            .headers
            .get("accept")
            .and_then(|value| value.to_str().ok()),
        Some("text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,application/json;q=0.8,*/*;q=0.7")
    );
    assert_eq!(
        request
            .headers
            .get("accept-language")
            .and_then(|value| value.to_str().ok()),
        Some("en-US,en;q=0.9")
    );
    assert_eq!(
        request
            .headers
            .get("upgrade-insecure-requests")
            .and_then(|value| value.to_str().ok()),
        Some("1")
    );
    assert_eq!(
        request
            .headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok()),
        Some("none")
    );
    assert_eq!(
        request
            .headers
            .get("sec-fetch-mode")
            .and_then(|value| value.to_str().ok()),
        Some("navigate")
    );
    assert_eq!(
        request
            .headers
            .get("sec-fetch-user")
            .and_then(|value| value.to_str().ok()),
        Some("?1")
    );
    assert_eq!(
        request
            .headers
            .get("sec-fetch-dest")
            .and_then(|value| value.to_str().ok()),
        Some("document")
    );
    assert_eq!(
        request
            .headers
            .get("sec-ch-ua")
            .and_then(|value| value.to_str().ok()),
        Some("\"Not(A:Brand\";v=\"99\", \"Google Chrome\";v=\"133\", \"Chromium\";v=\"133\"")
    );
    assert_eq!(
        request
            .headers
            .get("sec-ch-ua-mobile")
            .and_then(|value| value.to_str().ok()),
        Some("?0")
    );
    assert_eq!(
        request
            .headers
            .get("sec-ch-ua-platform")
            .and_then(|value| value.to_str().ok()),
        Some("\"Windows\"")
    );
}

#[cfg(not(feature = "browser-tools"))]
#[tokio::test]
async fn web_fetch_browser_reader_reports_unavailable_without_browser_tools_feature() {
    let root = tempfile::tempdir().expect("tempdir");
    let err = WebFetchTool::new()
        .call(
            json!({
                "url": "http://127.0.0.1/",
                "reader": "browser",
                "allow_private": true
            }),
            make_ctx(&root, HashMap::new()),
        )
        .await
        .expect_err("browser reader should require browser-tools feature");

    assert!(
        err.to_string().contains("browser reader is not available"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn web_fetch_can_use_jina_reader_for_public_pages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/markdown")
                .set_body_string("# Example\n\nReader extracted text."),
        )
        .mount(&server)
        .await;

    let root = tempfile::tempdir().expect("tempdir");
    let mut metadata = HashMap::new();
    metadata.insert("jina_reader_base_url".into(), json!(server.uri()));
    let result = WebFetchTool::new()
        .call(
            json!({
                "url": "https://93.184.216.34/article",
                "reader": "jina"
            }),
            make_ctx(&root, metadata),
        )
        .await
        .expect("fetch via jina");

    assert!(!result.is_error, "got: {}", text(&result));
    assert!(text(&result).contains("Reader extracted text."));
}

#[tokio::test]
async fn web_fetch_returns_full_text_for_shared_output_processor() {
    let large_text = "large web content\n".repeat(2_000);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/markdown")
                .set_body_string(large_text.clone()),
        )
        .mount(&server)
        .await;

    let root = tempfile::tempdir().expect("tempdir");
    let mut metadata = HashMap::new();
    metadata.insert("jina_reader_base_url".into(), json!(server.uri()));
    let result = WebFetchTool::new()
        .call(
            json!({
                "url": "https://93.184.216.34/large",
                "reader": "jina"
            }),
            make_ctx(&root, metadata),
        )
        .await
        .expect("fetch via jina");

    assert!(!result.is_error, "got: {}", text(&result));
    assert!(result.truncation.is_none());
    assert_eq!(text(&result), large_text);
}

#[tokio::test]
async fn web_fetch_max_bytes_truncates_instead_of_erroring() {
    let body = "0123456789".repeat(20);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let root = tempfile::tempdir().expect("tempdir");
    let result = WebFetchTool::new()
        .call(
            json!({
                "url": format!("{}/large", server.uri()),
                "reader": "native",
                "allow_private": true,
                "max_bytes": 30
            }),
            make_ctx(&root, HashMap::new()),
        )
        .await
        .expect("fetch with truncation");

    assert!(!result.is_error, "got: {}", text(&result));
    let output = text(&result);
    assert!(output.starts_with("012345678901234567890123456789"));
    assert!(
        output.contains("web_fetch truncated response at 30 bytes"),
        "got: {output}"
    );
}

#[tokio::test]
async fn exa_provider_maps_search_results() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Exa Result",
                "url": "https://exa.example/result",
                "highlights": ["agent-friendly snippet"]
            }]
        })))
        .mount(&server)
        .await;

    let provider =
        ExaSearchProvider::with_base_url("key".into(), format!("{}/search", server.uri()));
    let results = provider.search("agent search", 5).await.expect("exa");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Exa Result");
    assert_eq!(results[0].url, "https://exa.example/result");
    assert_eq!(results[0].snippet, "agent-friendly snippet");
}

#[tokio::test]
async fn exa_mcp_provider_maps_keyless_search_results() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(json!({"method": "initialize"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "exa", "version": "test"}
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(json!({
            "method": "tools/call",
            "params": {
                "name": "web_search_exa",
                "arguments": {
                    "query": "agent search",
                    "numResults": 5
                }
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "content": [{
                    "type": "text",
                    "text": "Title: Exa MCP Result\nURL: https://exa.example/mcp\nPublished: N/A\nAuthor: N/A\nHighlights:\nagent-friendly MCP snippet"
                }]
            }
        })))
        .mount(&server)
        .await;

    let provider = ExaMcpSearchProvider::with_base_url(format!("{}/mcp", server.uri()));
    let results = provider.search("agent search", 5).await.expect("exa mcp");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Exa MCP Result");
    assert_eq!(results[0].url, "https://exa.example/mcp");
    assert_eq!(results[0].snippet, "agent-friendly MCP snippet");
}

#[tokio::test]
async fn tavily_provider_maps_search_results() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Tavily Result",
                "url": "https://tavily.example/result",
                "content": "tavily content"
            }]
        })))
        .mount(&server)
        .await;

    let provider =
        TavilySearchProvider::with_base_url("key".into(), format!("{}/search", server.uri()));
    let results = provider.search("agent search", 5).await.expect("tavily");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Tavily Result");
    assert_eq!(results[0].url, "https://tavily.example/result");
    assert_eq!(results[0].snippet, "tavily content");
}

#[tokio::test]
async fn tavily_keyless_provider_sends_keyless_access_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("x-tavily-access-mode", "keyless"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Tavily Keyless Result",
                "url": "https://tavily.example/keyless",
                "content": "keyless content"
            }]
        })))
        .mount(&server)
        .await;

    let provider = TavilySearchProvider::keyless_with_base_url(format!("{}/search", server.uri()));
    let results = provider
        .search("agent search", 5)
        .await
        .expect("tavily keyless");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Tavily Keyless Result");
    assert_eq!(results[0].url, "https://tavily.example/keyless");
}

#[tokio::test]
async fn serper_provider_maps_search_results() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "organic": [{
                "title": "Serper Result",
                "link": "https://serper.example/result",
                "snippet": "serper snippet"
            }]
        })))
        .mount(&server)
        .await;

    let provider =
        SerperSearchProvider::with_base_url("key".into(), format!("{}/search", server.uri()));
    let results = provider.search("agent search", 5).await.expect("serper");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Serper Result");
    assert_eq!(results[0].url, "https://serper.example/result");
    assert_eq!(results[0].snippet, "serper snippet");
}

#[tokio::test]
async fn brave_provider_maps_search_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "web": {
                "results": [{
                    "title": "Brave Result",
                    "url": "https://brave.example/result",
                    "description": "brave description"
                }]
            }
        })))
        .mount(&server)
        .await;

    let provider =
        BraveSearchProvider::with_base_url("key".into(), format!("{}/search", server.uri()));
    let results = provider.search("agent search", 5).await.expect("brave");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Brave Result");
    assert_eq!(results[0].url, "https://brave.example/result");
    assert_eq!(results[0].snippet, "brave description");
}

#[tokio::test]
async fn web_search_tool_defaults_to_tavily_keyless_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("x-tavily-access-mode", "keyless"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Default Tavily",
                "url": "https://tavily.example/default",
                "content": "selected without an API key"
            }]
        })))
        .mount(&server)
        .await;

    let root = tempfile::tempdir().expect("tempdir");
    let mut metadata = HashMap::new();
    metadata.insert(
        "tavily_base_url".into(),
        json!(format!("{}/search", server.uri())),
    );

    let result = WebSearchTool::new()
        .call(
            json!({"query": "agent search", "max_results": 3}),
            make_ctx(&root, metadata),
        )
        .await
        .expect("web search");

    assert!(!result.is_error, "got: {}", text(&result));
    let output = text(&result);
    assert!(output.contains("Default Tavily"), "got: {output}");
    assert!(
        output.contains("URL: https://tavily.example/default"),
        "got: {output}"
    );
}

#[tokio::test]
async fn web_search_tool_falls_back_from_tavily_to_exa() {
    let tavily = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&tavily)
        .await;

    let exa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Exa Fallback",
                "url": "https://exa.example/fallback",
                "highlights": ["selected after Tavily"]
            }]
        })))
        .mount(&exa)
        .await;

    let root = tempfile::tempdir().expect("tempdir");
    let mut metadata = HashMap::new();
    metadata.insert(
        "tavily_base_url".into(),
        json!(format!("{}/search", tavily.uri())),
    );
    metadata.insert("exa_api_key".into(), json!("exa-key"));
    metadata.insert(
        "exa_base_url".into(),
        json!(format!("{}/search", exa.uri())),
    );

    let result = WebSearchTool::new()
        .call(
            json!({"query": "terminal.js JavaScript library", "max_results": 3}),
            make_ctx(&root, metadata),
        )
        .await
        .expect("web search");

    assert!(!result.is_error, "got: {}", text(&result));
    let output = text(&result);
    assert!(output.contains("Exa Fallback"), "got: {output}");
    assert!(
        output.contains("https://exa.example/fallback"),
        "got: {output}"
    );
}

#[tokio::test]
async fn web_search_tool_selects_configured_key_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "organic": [{
                "title": "Configured Serper",
                "link": "https://serper.example/configured",
                "snippet": "selected through metadata"
            }]
        })))
        .mount(&server)
        .await;

    let root = tempfile::tempdir().expect("tempdir");
    let mut metadata = HashMap::new();
    metadata.insert("search_provider".into(), json!("serper"));
    metadata.insert("serper_api_key".into(), json!("key"));
    metadata.insert(
        "serper_base_url".into(),
        json!(format!("{}/search", server.uri())),
    );

    let result = WebSearchTool::new()
        .call(
            json!({"query": "agent search", "max_results": 3}),
            make_ctx(&root, metadata),
        )
        .await
        .expect("web search");

    assert!(!result.is_error, "got: {}", text(&result));
    let output = text(&result);
    assert!(output.contains("Configured Serper"), "got: {output}");
    assert!(
        output.contains("https://serper.example/configured"),
        "got: {output}"
    );
}

#[tokio::test]
async fn web_search_tool_uses_ordered_runtime_tool_config() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "organic": [{
                "title": "Runtime Tool Config Serper",
                "link": "https://serper.example/runtime-config",
                "snippet": "selected through tool_configs metadata"
            }]
        })))
        .mount(&server)
        .await;

    let root = tempfile::tempdir().expect("tempdir");
    let mut metadata = HashMap::new();
    metadata.insert(
        "tool_configs".into(),
        json!({
            "web_search": {
                "mode": "ordered_fallback",
                "providers": [
                    {
                        "id": "serper",
                        "enabled": true,
                        "api_key": "key",
                        "base_url": format!("{}/search", server.uri())
                    }
                ]
            }
        }),
    );

    let result = WebSearchTool::new()
        .call(
            json!({"query": "agent search", "max_results": 3}),
            make_ctx(&root, metadata),
        )
        .await
        .expect("web search");

    assert!(!result.is_error, "got: {}", text(&result));
    let output = text(&result);
    assert!(
        output.contains("Runtime Tool Config Serper"),
        "got: {output}"
    );
    assert!(
        output.contains("https://serper.example/runtime-config"),
        "got: {output}"
    );
}

#[tokio::test]
async fn web_search_tool_respects_all_providers_disabled() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut metadata = HashMap::new();
    metadata.insert(
        "tool_configs".into(),
        json!({
            "web_search": {
                "mode": "ordered_fallback",
                "providers": [
                    {"id": "tavily", "enabled": false},
                    {"id": "exa", "enabled": false}
                ]
            }
        }),
    );

    let error = WebSearchTool::new()
        .call(
            json!({"query": "agent search", "max_results": 3}),
            make_ctx(&root, metadata),
        )
        .await
        .expect_err("all-disabled config must not use defaults");

    assert!(
        error
            .to_string()
            .contains("no web search providers are enabled"),
        "got: {error}"
    );
}
