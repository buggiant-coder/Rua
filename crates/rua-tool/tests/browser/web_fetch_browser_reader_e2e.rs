#[cfg(all(feature = "web", feature = "browser-tools"))]
mod tests {
    use std::collections::HashMap;

    use rua_contract::Tool;
    use rua_core::tool::ToolContext;
    use rua_tool::extended::web_fetch::WebFetchTool;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_ctx() -> ToolContext {
        ToolContext {
            agent_id: "test".into(),
            session_id: "test".into(),
            working_dir: std::env::temp_dir(),
            metadata: HashMap::new(),
            cancel: None,
            task_runtime: None,
            ..Default::default()
        }
    }

    #[tokio::test]
    #[ignore = "requires Chrome/Chromium installed"]
    async fn browser_reader_executes_page_javascript_and_returns_visible_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/dynamic"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(
                        r#"
                        <html>
                          <body>
                            <main id="app">loading</main>
                            <script>
                              document.getElementById("app").innerText =
                                "Rendered by browser reader";
                            </script>
                          </body>
                        </html>
                        "#,
                    ),
            )
            .mount(&server)
            .await;

        let result = WebFetchTool::new()
            .call(
                json!({
                    "url": format!("{}/dynamic", server.uri()),
                    "reader": "browser",
                    "allow_private": true
                }),
                test_ctx(),
            )
            .await
            .expect("browser web fetch");

        assert!(!result.is_error, "got: {}", result.text_content());
        assert!(result.text_content().contains("Rendered by browser reader"));
    }
}
