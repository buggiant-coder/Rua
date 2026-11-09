use std::collections::HashMap;

use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::extended::search::{GlobSearchTool, GrepSearchTool};
use serde_json::json;
use tempfile::TempDir;

fn make_ctx(root: &TempDir) -> ToolContext {
    let tool_result_store_dir = root.path().join("tool-results");
    std::fs::create_dir_all(&tool_result_store_dir).expect("tool result store");
    ToolContext {
        agent_id: "agent".into(),
        session_id: "session".into(),
        working_dir: root.path().to_path_buf(),
        tool_result_store_dir: Some(tool_result_store_dir),
        metadata: HashMap::new(),
        cancel: None,
        task_runtime: None,
        ..Default::default()
    }
}

fn make_cancelled_ctx(root: &TempDir) -> ToolContext {
    let (tx, rx) = tokio::sync::watch::channel(false);
    tx.send(true).expect("mark cancelled");
    ToolContext {
        cancel: Some(rx),
        ..make_ctx(root)
    }
}

fn text(result: &rua_core::tool::ToolResult) -> String {
    result.text_content()
}

fn tempdir_in_home(prefix: &str) -> Option<TempDir> {
    let home = dirs::home_dir()?;
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(home)
        .ok()
}

#[cfg(unix)]
struct ReadPermissionGuard(std::path::PathBuf);

#[cfg(unix)]
impl Drop for ReadPermissionGuard {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;

        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o600));
    }
}

#[cfg(unix)]
fn make_unreadable(path: &std::path::Path) -> Option<ReadPermissionGuard> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
        .expect("remove read permission");
    if std::fs::read(path).is_ok() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("restore read permission");
        return None;
    }
    Some(ReadPermissionGuard(path.to_path_buf()))
}

#[tokio::test]
async fn grep_single_file_reports_matches_and_coalesced_context() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        root.path().join("file.txt"),
        "line zero\nline one\nline two\nline three\nline four\n",
    )
    .expect("seed file");

    let tool = GrepSearchTool::new();
    assert_eq!(tool.name(), "grep");
    let schema = tool.schema();
    assert_eq!(
        schema.parameters["properties"]["context_lines"]["default"],
        0
    );
    assert_eq!(
        schema.parameters["properties"]["context_lines"]["minimum"],
        0
    );
    assert_eq!(
        schema.parameters["properties"]["context_lines"]["maximum"],
        5
    );

    let no_context = tool
        .call(
            json!({"pattern": "two", "path": "file.txt", "context_lines": 0}),
            make_ctx(&root),
        )
        .await
        .expect("grep without context");
    assert_eq!(text(&no_context), "file.txt:3:line two");

    let empty_filters = tool
        .call(
            json!({
                "pattern": "two",
                "path": root.path().join("file.txt"),
                "context_lines": 0,
                "include": [],
                "exclude": []
            }),
            make_ctx(&root),
        )
        .await
        .expect("grep with explicit empty filters");
    assert_eq!(text(&empty_filters), "file.txt:3:line two");

    let with_context = tool
        .call(
            json!({"pattern": "two", "path": "file.txt", "context_lines": 2}),
            make_ctx(&root),
        )
        .await
        .expect("grep with context");
    assert_eq!(
        text(&with_context),
        "file.txt:1:  line zero\n\
         file.txt:2:  line one\n\
         file.txt:3:> line two\n\
         file.txt:4:  line three\n\
         file.txt:5:  line four"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn grep_reports_file_read_failures_instead_of_no_matches() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("unreadable.txt");
    std::fs::write(&path, "needle\n").expect("seed unreadable file");
    let Some(_permission_guard) = make_unreadable(&path) else {
        return;
    };

    let result = GrepSearchTool::new()
        .call(
            json!({"pattern": "needle", "path": "unreadable.txt"}),
            make_ctx(&root),
        )
        .await
        .expect("grep response");
    let body = text(&result);

    assert!(result.is_error, "got: {body}");
    assert!(body.contains("Search incomplete"), "got: {body}");
    assert!(body.contains("unreadable.txt"), "got: {body}");
    assert!(!body.contains("No matches found"), "got: {body}");
}

#[cfg(unix)]
#[tokio::test]
async fn grep_preserves_partial_results_when_another_file_cannot_be_read() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("readable.txt"), "needle\n").expect("seed readable file");
    let unreadable = root.path().join("unreadable.txt");
    std::fs::write(&unreadable, "needle\n").expect("seed unreadable file");
    let Some(_permission_guard) = make_unreadable(&unreadable) else {
        return;
    };

    let result = GrepSearchTool::new()
        .call(json!({"pattern": "needle", "path": "."}), make_ctx(&root))
        .await
        .expect("grep response");
    let body = text(&result);

    assert!(result.is_error, "got: {body}");
    assert!(body.contains("Search incomplete"), "got: {body}");
    assert!(body.contains("[partial results]"), "got: {body}");
    assert!(body.contains("readable.txt:1:needle"), "got: {body}");
    assert!(body.contains("unreadable.txt"), "got: {body}");
}

#[tokio::test]
async fn grep_and_glob_expand_current_user_tilde_paths() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let Some(home_dir) = tempdir_in_home("rua-tool-home-search-") else {
        return;
    };
    let home_leaf = home_dir
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .expect("home tempdir leaf");
    std::fs::write(home_dir.path().join("match.rs"), "fn acp_marker() {}\n")
        .expect("seed home file");

    let grep = GrepSearchTool::new()
        .call(
            json!({
                "pattern": "acp_marker",
                "path": format!("~/{home_leaf}"),
                "include": ["*.rs"],
                "context_lines": 0
            }),
            make_ctx(&workspace),
        )
        .await
        .expect("grep expands tilde");
    assert_eq!(text(&grep), "match.rs:1:fn acp_marker() {}");

    let glob = GlobSearchTool::new()
        .call(
            json!({
                "pattern": "*.rs",
                "path": format!("~/{home_leaf}")
            }),
            make_ctx(&workspace),
        )
        .await
        .expect("glob expands tilde");
    assert_eq!(text(&glob), "match.rs");
}

#[tokio::test]
async fn grep_context_windows_are_coalesced_and_separated_like_rust_contract() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        root.path().join("timeline.txt"),
        [
            "line 1",
            "line 2 hit",
            "line 3 hit",
            "line 4",
            "line 5",
            "line 6",
            "line 7 hit",
            "line 8",
            "line 9",
            "line 10",
            "line 11",
            "line 12",
            "line 13 hit",
        ]
        .join("\n")
            + "\n",
    )
    .expect("seed file");

    let result = GrepSearchTool::new()
        .call(
            json!({
                "pattern": "hit",
                "path": "timeline.txt",
                "context_lines": 2
            }),
            make_ctx(&root),
        )
        .await
        .expect("grep context windows");

    assert_eq!(
        text(&result),
        "timeline.txt:1:  line 1\n\
         timeline.txt:2:> line 2 hit\n\
         timeline.txt:3:> line 3 hit\n\
         timeline.txt:4:  line 4\n\
         timeline.txt:5:  line 5\n\
         timeline.txt:6:  line 6\n\
         timeline.txt:7:> line 7 hit\n\
         timeline.txt:8:  line 8\n\
         timeline.txt:9:  line 9\n\
         ---\n\
         timeline.txt:11:  line 11\n\
         timeline.txt:12:  line 12\n\
         timeline.txt:13:> line 13 hit"
    );
}

#[tokio::test]
async fn grep_include_exclude_and_default_excluded_dirs_match_rust_contract() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("keep.py"), "def keep(): pass\n").expect("keep");
    std::fs::write(root.path().join("skip.txt"), "def skip(): pass\n").expect("skip txt");
    std::fs::create_dir(root.path().join("target")).expect("target dir");
    std::fs::write(root.path().join("target/hidden.py"), "def hidden(): pass\n").expect("hidden");
    std::fs::create_dir(root.path().join(".git")).expect("git dir");
    std::fs::write(root.path().join(".git/config.py"), "def hidden(): pass\n").expect("git file");
    std::fs::create_dir(root.path().join("node_modules")).expect("node_modules dir");
    std::fs::write(
        root.path().join("node_modules/pkg.py"),
        "def hidden(): pass\n",
    )
    .expect("node file");

    let result = GrepSearchTool::new()
        .call(
            json!({
                "pattern": "def",
                "path": ".",
                "include": ["*.py"],
                "context_lines": 0
            }),
            make_ctx(&root),
        )
        .await
        .expect("grep include pattern");

    let output = text(&result);
    assert!(
        output.contains("keep.py:1:def keep(): pass"),
        "got: {output}"
    );
    assert!(!output.contains("skip.txt"), "got: {output}");
    assert!(!output.contains("target/hidden.py"), "got: {output}");
    assert!(!output.contains(".git/config.py"), "got: {output}");
    assert!(!output.contains("node_modules/pkg.py"), "got: {output}");
}

#[tokio::test]
async fn grep_skips_known_binary_extensions_before_text_search() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("image.png"), b"def hidden_in_media()\n").expect("png");
    std::fs::write(
        root.path().join("archive.zip"),
        b"def hidden_in_archive()\n",
    )
    .expect("zip");
    std::fs::write(root.path().join("code.py"), "def visible(): pass\n").expect("py");

    let result = GrepSearchTool::new()
        .call(
            json!({"pattern": "def", "path": ".", "context_lines": 0}),
            make_ctx(&root),
        )
        .await
        .expect("grep binary extensions");

    let output = text(&result);
    assert!(
        output.contains("code.py:1:def visible(): pass"),
        "got: {output}"
    );
    assert!(!output.contains("image.png"), "got: {output}");
    assert!(!output.contains("archive.zip"), "got: {output}");
}

#[tokio::test]
async fn grep_literal_regex_and_case_sensitivity_are_explicit() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("sample.txt"), "a.b\naxb\nAlpha\nalpha\n").expect("seed");
    let tool = GrepSearchTool::new();

    let literal = tool
        .call(
            json!({"pattern": "a.b", "path": "sample.txt", "is_regex": false}),
            make_ctx(&root),
        )
        .await
        .expect("literal grep");
    let literal_output = text(&literal);
    assert_eq!(literal_output, "sample.txt:1:a.b");

    let regex = tool
        .call(
            json!({"pattern": "a.b", "path": "sample.txt", "is_regex": true}),
            make_ctx(&root),
        )
        .await
        .expect("regex grep");
    let regex_output = text(&regex);
    assert!(
        regex_output.contains("sample.txt:1:a.b"),
        "got: {regex_output}"
    );
    assert!(
        regex_output.contains("sample.txt:2:axb"),
        "got: {regex_output}"
    );

    let case_insensitive = tool
        .call(
            json!({"pattern": "alpha", "path": "sample.txt", "case_sensitive": false}),
            make_ctx(&root),
        )
        .await
        .expect("case-insensitive grep");
    let case_output = text(&case_insensitive);
    assert!(
        case_output.contains("sample.txt:3:Alpha"),
        "got: {case_output}"
    );
    assert!(
        case_output.contains("sample.txt:4:alpha"),
        "got: {case_output}"
    );
}

#[tokio::test]
async fn grep_returns_cross_file_matches_in_sorted_path_order() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("z.txt"), "match z\n").expect("z");
    std::fs::write(root.path().join("a.txt"), "match a\n").expect("a");
    std::fs::write(root.path().join("m.txt"), "match m\n").expect("m");

    let result = GrepSearchTool::new()
        .call(
            json!({"pattern": "match", "path": ".", "context_lines": 0}),
            make_ctx(&root),
        )
        .await
        .expect("grep sorted files");

    assert_eq!(
        text(&result),
        "a.txt:1:match a\nm.txt:1:match m\nz.txt:1:match z"
    );
}

#[tokio::test]
async fn grep_missing_path_and_result_limit_return_tool_results() {
    let root = tempfile::tempdir().expect("tempdir");
    let tool = GrepSearchTool::new();

    let missing = tool
        .call(
            json!({"pattern": "anything", "path": "missing"}),
            make_ctx(&root),
        )
        .await
        .expect("missing path response");
    assert!(missing.is_error);
    assert!(text(&missing).contains("does not exist"));

    for i in 0..5 {
        std::fs::write(root.path().join(format!("file_{i}.txt")), "match\n").expect("seed match");
    }
    let limited = tool
        .call(
            json!({"pattern": "match", "path": ".", "max_results": 2}),
            make_ctx(&root),
        )
        .await
        .expect("limited grep");
    assert!(
        limited.truncation.is_none(),
        "agent-requested max_results is a result limit, not system truncation"
    );
    let body = text(&limited);
    assert!(body.contains("[result_limit tool=\"grep\"]"), "got: {body}");
    assert!(body.contains("max_results=2 reached"), "got: {body}");
    assert!(!body.contains("<truncated_output"), "got: {body}");
}

#[tokio::test]
async fn grep_and_glob_report_cancelled_searches_explicitly() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("file.txt"), "match\n").expect("seed file");

    let grep = GrepSearchTool::new()
        .call(
            json!({"pattern": "match", "path": "."}),
            make_cancelled_ctx(&root),
        )
        .await
        .expect("cancelled grep response");
    assert!(grep.is_error);
    assert!(
        text(&grep).contains("cancelled"),
        "cancelled grep must not look like an empty successful search: {}",
        text(&grep)
    );
    assert!(
        !text(&grep).contains("No matches found"),
        "got: {}",
        text(&grep)
    );

    let glob = GlobSearchTool::new()
        .call(
            json!({"pattern": "*.txt", "path": "."}),
            make_cancelled_ctx(&root),
        )
        .await
        .expect("cancelled glob response");
    assert!(glob.is_error);
    assert!(
        text(&glob).contains("cancelled"),
        "cancelled glob must not look like an empty successful search: {}",
        text(&glob)
    );
    assert!(
        !text(&glob).contains("No files matched"),
        "got: {}",
        text(&glob)
    );
}

#[tokio::test]
async fn glob_returns_sorted_paths_skips_defaults_and_marks_directories() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(root.path().join("src")).expect("src dir");
    std::fs::write(root.path().join("src/b.rs"), "").expect("b");
    std::fs::write(root.path().join("src/a.rs"), "").expect("a");
    std::fs::create_dir(root.path().join("target")).expect("target dir");
    std::fs::write(root.path().join("target/hidden.rs"), "").expect("hidden");

    let tool = GlobSearchTool::new();
    assert_eq!(tool.name(), "glob");

    let files = tool
        .call(json!({"pattern": "**/*.rs", "path": "."}), make_ctx(&root))
        .await
        .expect("glob files");
    assert_eq!(text(&files), "src/a.rs\nsrc/b.rs");

    let dirs = tool
        .call(json!({"pattern": "src", "path": "."}), make_ctx(&root))
        .await
        .expect("glob dir");
    assert_eq!(text(&dirs), "src/");
}

#[tokio::test]
async fn glob_missing_path_and_result_limit_return_tool_results() {
    let root = tempfile::tempdir().expect("tempdir");
    let tool = GlobSearchTool::new();

    let missing = tool
        .call(
            json!({"pattern": "**/*", "path": "missing"}),
            make_ctx(&root),
        )
        .await
        .expect("missing path response");
    assert!(missing.is_error);
    assert!(text(&missing).contains("does not exist"));

    for i in 0..5 {
        std::fs::write(root.path().join(format!("file_{i}.txt")), "").expect("seed file");
    }
    let limited = tool
        .call(
            json!({"pattern": "*.txt", "path": ".", "max_results": 2}),
            make_ctx(&root),
        )
        .await
        .expect("limited glob");
    assert!(
        limited.truncation.is_none(),
        "agent-requested max_results is a result limit, not system truncation"
    );
    let body = text(&limited);
    assert!(body.contains("[result_limit tool=\"glob\"]"), "got: {body}");
    assert!(body.contains("max_results=2 reached"), "got: {body}");
    assert!(!body.contains("<truncated_output"), "got: {body}");
}
