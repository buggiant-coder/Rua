use std::collections::HashMap;

use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::{EditFileTool, ReadFileTool};
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

fn text(result: &rua_core::tool::ToolResult) -> String {
    result.text_content()
}

fn patch_arg(patch: &str) -> serde_json::Value {
    json!({ "patch": patch })
}

#[tokio::test]
async fn read_file_reports_missing_directory_and_line_range_errors() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        root.path().join("lines.txt"),
        "line1\nline2\nline3\nline4\n",
    )
    .expect("write lines");
    std::fs::create_dir(root.path().join("folder")).expect("folder");

    let read = ReadFileTool::new();
    assert_eq!(read.name(), "read");

    let missing = read
        .call(json!({"file_path": "missing.txt"}), make_ctx(&root))
        .await
        .expect("missing file response");
    assert!(missing.is_error);
    assert!(text(&missing).contains("does not exist"));

    let directory = read
        .call(json!({"file_path": "folder"}), make_ctx(&root))
        .await
        .expect("directory response");
    assert!(directory.is_error);
    assert!(text(&directory).contains("not a file"));

    let ranged = read
        .call(
            json!({"file_path": "lines.txt", "start_line": 2, "limit": 2}),
            make_ctx(&root),
        )
        .await
        .expect("range response");
    let ranged_text = text(&ranged);
    assert!(ranged_text.contains("2 | line2"), "got: {ranged_text}");
    assert!(ranged_text.contains("3 | line3"), "got: {ranged_text}");
    assert!(!ranged_text.contains("1 | line1"), "got: {ranged_text}");
    assert!(!ranged_text.contains("4 | line4"), "got: {ranged_text}");
    assert!(
        ranged.truncation.is_none(),
        "an explicitly requested line window is a complete result"
    );
    assert!(
        !ranged_text.contains("<truncated_output"),
        "an explicitly requested line window must not be labeled as truncated: {ranged_text}"
    );
    assert!(
        !ranged_text.contains('\t'),
        "line number separator should not render as a tab escape: {ranged_text}"
    );

    let too_far = read
        .call(
            json!({"file_path": "lines.txt", "start_line": 100}),
            make_ctx(&root),
        )
        .await
        .expect("range error response");
    assert!(too_far.is_error);
    assert!(text(&too_far).contains("exceeds file length"));

    let invalid_start = read
        .call(
            json!({"file_path": "lines.txt", "start_line": "abc"}),
            make_ctx(&root),
        )
        .await
        .expect("invalid start_line response");
    assert!(invalid_start.is_error);
    assert!(text(&invalid_start).contains("start_line must be an integer"));

    let invalid_limit = read
        .call(
            json!({"file_path": "lines.txt", "limit": "xyz"}),
            make_ctx(&root),
        )
        .await
        .expect("invalid limit response");
    assert!(invalid_limit.is_error);
    assert!(text(&invalid_limit).contains("limit must be an integer"));

    let zero_start = read
        .call(
            json!({"file_path": "lines.txt", "start_line": 0}),
            make_ctx(&root),
        )
        .await
        .expect("zero start_line response");
    assert!(zero_start.is_error);
    assert!(text(&zero_start).contains("start_line"));

    let unsupported_end_line = read
        .call(
            json!({"file_path": "lines.txt", "end_line": 3}),
            make_ctx(&root),
        )
        .await
        .expect("unsupported end_line response");
    assert!(unsupported_end_line.is_error);
    assert!(text(&unsupported_end_line).contains("end_line is not supported"));
}

#[tokio::test]
async fn read_file_defaults_to_a_continuable_line_window() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut body = String::new();
    for line in 1..=405 {
        body.push_str(&format!("line{line}\n"));
    }
    std::fs::write(root.path().join("many.txt"), body).expect("write many lines");

    let read = ReadFileTool::new();
    let result = read
        .call(json!({"file_path": "many.txt"}), make_ctx(&root))
        .await
        .expect("default window response");
    let body = text(&result);

    assert!(!result.is_error, "got: {body}");
    assert!(body.contains("     1 | line1\n"), "got: {body}");
    assert!(body.contains("   400 | line400\n"), "got: {body}");
    assert!(!body.contains("   401 | line401\n"), "got: {body}");
    assert!(
        body.contains("read(file_path=\"many.txt\", start_line=401, limit=400)"),
        "continuation hint should preserve the default window size: {body}"
    );
    assert!(
        result.truncation.is_some(),
        "windowed read should expose continuation metadata"
    );
}

#[tokio::test]
async fn read_file_handles_bom_and_lossy_non_utf8_but_rejects_binary_nulls() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("bom.txt"), b"\xef\xbb\xbfhello\n").expect("bom");
    std::fs::write(root.path().join("gbk.txt"), [0xc4, 0xe3, 0xba, 0xc3]).expect("gbk bytes");
    std::fs::write(root.path().join("binary.bin"), b"abc\0def").expect("binary");

    let read = ReadFileTool::new();

    let bom = read
        .call(json!({"file_path": "bom.txt"}), make_ctx(&root))
        .await
        .expect("bom response");
    assert!(!bom.is_error, "got: {}", text(&bom));
    assert!(
        text(&bom).contains("hello"),
        "UTF-8 BOM should not hide visible text: {}",
        text(&bom)
    );

    let gbk = read
        .call(json!({"file_path": "gbk.txt"}), make_ctx(&root))
        .await
        .expect("lossy response");
    assert!(
        !gbk.is_error,
        "non-UTF-8 text should be lossy-decoded instead of failing: {}",
        text(&gbk)
    );
    assert!(
        text(&gbk).contains('\u{fffd}'),
        "Rust does not implement QwenPaw's GBK fallback; it should surface replacement chars: {}",
        text(&gbk)
    );

    let binary = read
        .call(json!({"file_path": "binary.bin"}), make_ctx(&root))
        .await;
    assert!(
        binary.is_err(),
        "null-byte binary files should be rejected before lossy text decoding"
    );
}

#[tokio::test]
async fn edit_file_applies_codex_style_add_and_update_hunks() {
    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();
    assert_eq!(edit.name(), "edit");

    let added = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Add File: nested/new.txt
+hello
+world
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("add file response");
    assert!(!added.is_error, "got: {}", text(&added));
    assert!(text(&added).contains("A nested/new.txt"));
    assert_eq!(
        std::fs::read_to_string(root.path().join("nested/new.txt")).expect("read added"),
        "hello\nworld\n"
    );

    std::fs::write(root.path().join("update.txt"), "old\nkeep\n").expect("seed update");
    let updated = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: update.txt
@@
-old
+new
 keep
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("update file response");
    assert!(!updated.is_error, "got: {}", text(&updated));
    assert!(text(&updated).contains("M update.txt"));
    assert_eq!(
        std::fs::read_to_string(root.path().join("update.txt")).expect("read updated"),
        "new\nkeep\n"
    );
}

#[tokio::test]
async fn edit_file_rejects_delete_and_move_hunks_without_removing_files() {
    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();

    std::fs::write(root.path().join("old_name.txt"), "move me\n").expect("seed move");
    let moved = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: old_name.txt
*** Move to: new_name.txt
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("move file response");
    assert!(moved.is_error, "got: {}", text(&moved));
    assert!(
        text(&moved).contains("Moving or renaming files is not supported"),
        "got: {}",
        text(&moved)
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("old_name.txt")).expect("read original"),
        "move me\n"
    );
    assert!(!root.path().join("new_name.txt").exists());

    std::fs::write(root.path().join("delete.txt"), "remove\n").expect("seed delete");
    let deleted = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Delete File: delete.txt
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("delete file response");
    assert!(deleted.is_error, "got: {}", text(&deleted));
    assert!(
        text(&deleted).contains("Deleting files is not supported"),
        "got: {}",
        text(&deleted)
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("delete.txt")).expect("read original"),
        "remove\n"
    );
}

#[tokio::test]
async fn edit_file_reports_patch_parse_and_match_errors_as_tool_results() {
    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();

    let bad_header = edit
        .call(
            patch_arg(
                r#"*** Add File: nope.txt
+hello
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("bad header response");
    assert!(bad_header.is_error);
    assert!(text(&bad_header).contains("patch verification failed"));

    let empty_path = edit
        .call(
            patch_arg(concat!(
                "*** Begin Patch\n",
                "*** Add File: ",
                "\n+content\n",
                "*** End Patch",
            )),
            make_ctx(&root),
        )
        .await
        .expect("empty path response");
    assert!(empty_path.is_error);
    assert!(text(&empty_path).contains("Patch file paths cannot be empty"));

    std::fs::write(root.path().join("edit.txt"), "hello\n").expect("seed edit");
    let no_match = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: edit.txt
@@
-missing
+replacement
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("missing match response");
    assert!(no_match.is_error);
    assert!(text(&no_match).contains("Failed to find expected lines"));

    assert!(
        !root.path().join("content").exists(),
        "blank file paths must not accidentally create workspace files"
    );
}

#[tokio::test]
async fn edit_file_preflights_all_hunks_before_writing_any_file() {
    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();

    std::fs::write(root.path().join("first.txt"), "before first\n").expect("seed first");
    std::fs::write(root.path().join("second.txt"), "before second\n").expect("seed second");

    let result = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: first.txt
@@
-before first
+after first
*** Update File: second.txt
@@
-missing second
+after second
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("preflight response");

    assert!(result.is_error);
    assert!(text(&result).contains("Failed to find expected lines"));
    assert_eq!(
        std::fs::read_to_string(root.path().join("first.txt")).expect("read first"),
        "before first\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("second.txt")).expect("read second"),
        "before second\n"
    );
}

#[tokio::test]
async fn edit_file_matches_context_with_insignificant_whitespace_differences() {
    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();

    std::fs::write(root.path().join("spacing.txt"), "value = 1;   \n").expect("seed spacing");

    let result = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: spacing.txt
@@
-value = 1;
+value = 2;
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("whitespace response");

    assert!(!result.is_error, "got: {}", text(&result));
    assert_eq!(
        std::fs::read_to_string(root.path().join("spacing.txt")).expect("read spacing"),
        "value = 2;\n"
    );
}

#[tokio::test]
async fn edit_file_rejects_ambiguous_tolerant_matches_without_writing() {
    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();
    let original = "  target\nmiddle\n\ttarget\n";

    std::fs::write(root.path().join("ambiguous.txt"), original).expect("seed ambiguous");

    let result = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: ambiguous.txt
@@
-target
+replacement
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("ambiguous response");

    assert!(result.is_error);
    assert!(text(&result).contains("matched multiple locations"));
    assert_eq!(
        std::fs::read_to_string(root.path().join("ambiguous.txt")).expect("read ambiguous"),
        original
    );
}

#[tokio::test]
async fn edit_file_accepts_first_update_chunk_without_explicit_header() {
    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();
    std::fs::write(
        root.path().join("source.rs"),
        "use std::fmt;\nfn main() {}\n",
    )
    .expect("seed source");

    let result = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: source.rs
 use std::fmt;
 fn main() {}
+fn helper() {}
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("edit response");

    assert!(!result.is_error, "{}", text(&result));
    assert_eq!(
        std::fs::read_to_string(root.path().join("source.rs")).expect("read source"),
        "use std::fmt;\nfn main() {}\nfn helper() {}\n"
    );
}

#[tokio::test]
async fn edit_file_end_of_file_marker_anchors_the_update_at_file_end() {
    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();
    std::fs::write(root.path().join("repeated.txt"), "target\nmiddle\ntarget\n")
        .expect("seed repeated source");

    let result = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: repeated.txt
@@
-target
+replacement
*** End of File
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("edit response");

    assert!(!result.is_error, "{}", text(&result));
    assert_eq!(
        std::fs::read_to_string(root.path().join("repeated.txt")).expect("read repeated"),
        "target\nmiddle\nreplacement\n"
    );
}

#[tokio::test]
async fn edit_file_explains_json_escaping_for_unprefixed_update_lines() {
    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();
    std::fs::write(root.path().join("source.rs"), "let command = \"old\";\n").expect("seed source");

    let result = edit
        .call(
            patch_arg(
                "*** Begin Patch\n*** Update File: source.rs\n@@\n-let command = \"old\";\n+let command = \"first\nsecond\";\n*** End Patch",
            ),
            make_ctx(&root),
        )
        .await
        .expect("edit response");

    assert!(result.is_error);
    let message = text(&result);
    assert!(
        message.contains("Every update line must start"),
        "{message}"
    );
    assert!(message.contains(r#"encode it as `\\n`"#), "{message}");
}

#[cfg(unix)]
#[tokio::test]
async fn edit_file_reports_files_committed_before_a_write_failure() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("tempdir");
    let edit = EditFileTool::new();
    let locked_dir = root.path().join("locked");

    std::fs::create_dir(&locked_dir).expect("create locked dir");
    std::fs::write(root.path().join("first.txt"), "before first\n").expect("seed first");
    std::fs::write(locked_dir.join("second.txt"), "before second\n").expect("seed second");
    std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o555))
        .expect("lock second dir");

    let result = edit
        .call(
            patch_arg(
                r#"*** Begin Patch
*** Update File: first.txt
@@
-before first
+after first
*** Update File: locked/second.txt
@@
-before second
+after second
*** End Patch"#,
            ),
            make_ctx(&root),
        )
        .await
        .expect("commit failure response");

    std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o755))
        .expect("unlock second dir");

    assert!(result.is_error);
    assert!(text(&result).contains("partially applying the patch"));
    assert!(text(&result).contains("M first.txt"));
    assert_eq!(
        result.metadata["file_edit_committed_paths"],
        json!(["M first.txt"])
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("first.txt")).expect("read first"),
        "after first\n"
    );
    assert_eq!(
        std::fs::read_to_string(locked_dir.join("second.txt")).expect("read second"),
        "before second\n"
    );
}
