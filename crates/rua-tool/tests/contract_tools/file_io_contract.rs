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
async fn opened_regular_read_uses_the_selected_handle_for_the_normal_text_window() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("selected.txt");
    std::fs::write(&path, "first\nsecond\nthird\n").unwrap();
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());
    ctx.metadata.insert("tool_call_id".into(), json!("read-1"));
    let args = json!({"device_id":"desktop","file_path":path,"start_line":2,"limit":1});
    let ordinary = ReadFileTool.call(args.clone(), ctx.clone()).await.unwrap();
    let opened = ReadFileTool
        .open_regular_file(&path, root.path())
        .await
        .unwrap();
    assert_eq!(opened.public_path(), path);
    assert_eq!(opened.backing_path(), path.canonicalize().unwrap());

    std::fs::rename(&path, root.path().join("previous.txt")).unwrap();
    std::fs::write(&path, "replacement\nreplacement\n").unwrap();
    let selected = ReadFileTool
        .read_opened_file(opened, args, ctx)
        .await
        .unwrap();
    assert_eq!(selected, ordinary);
    assert!(selected.text_content().contains("2 | second"));
    assert_eq!(selected.metadata["file_location"]["device_id"], "desktop");
    assert_eq!(selected.metadata["continuation"]["start_line"], 3);
}

#[tokio::test]
async fn opened_regular_read_keeps_byte_truncation_and_checks_the_file_and_device() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("selected.txt");
    std::fs::write(&path, "a whole long line with a bounded output").unwrap();
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());
    ctx.raw_capture_max_bytes = 10;
    ctx.metadata.insert("tool_call_id".into(), json!("read-1"));
    let args = json!({"device_id":"desktop","file_path":path});
    let ordinary = ReadFileTool.call(args.clone(), ctx.clone()).await.unwrap();
    let open = || ReadFileTool.open_regular_file(&path, root.path());
    let selected = ReadFileTool
        .read_opened_file(open().await.unwrap(), args.clone(), ctx.clone())
        .await
        .unwrap();
    assert_eq!(selected, ordinary);
    assert_eq!(selected.metadata["byte_range"]["end_exclusive"], 10);
    assert_eq!(selected.metadata["continuation"]["byte_offset"], 10);
    assert_eq!(selected.metadata["file_location"]["coverage"], "partial");

    std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
    ctx.raw_capture_max_bytes = 20;
    let lines = ReadFileTool
        .read_opened_file(open().await.unwrap(), args.clone(), ctx.clone())
        .await
        .unwrap();
    assert!(lines.truncation.is_some());
    assert_eq!(
        lines,
        ReadFileTool.call(args.clone(), ctx.clone()).await.unwrap()
    );

    for changed in [
        json!({"device_id":"other","file_path":path}),
        json!({"device_id":"desktop","file_path":root.path().join("other.txt")}),
    ] {
        assert!(ReadFileTool
            .read_opened_file(open().await.unwrap(), changed, ctx.clone())
            .await
            .is_err());
    }
    let mut page = args.clone();
    page["after"] = json!("a");
    assert!(
        ReadFileTool
            .read_opened_file(open().await.unwrap(), page, ctx.clone())
            .await
            .unwrap()
            .is_error
    );
    ctx.cancel = Some(tokio::sync::watch::channel(true).1);
    let stopped = ReadFileTool
        .read_opened_file(open().await.unwrap(), args, ctx)
        .await
        .unwrap();
    assert!(stopped.is_error && stopped.text_content().contains("Read cancelled"));
}

#[cfg(unix)]
#[tokio::test]
async fn opened_regular_read_accepts_file_aliases_and_rejects_special_files_and_protected_names() {
    use std::os::unix::fs::symlink;
    use std::path::Path;
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().join("selected.txt");
    std::fs::write(&selected, "sample").unwrap();
    let alias = root.path().join("alias");
    symlink(&selected, &alias).unwrap();
    let opened = ReadFileTool
        .open_regular_file(&alias, root.path())
        .await
        .unwrap();
    assert_eq!(opened.public_path(), alias);
    assert_eq!(
        opened.resolved_public_path(),
        selected.canonicalize().unwrap()
    );
    assert_eq!(opened.backing_path(), opened.resolved_public_path());

    let protected = root.path().join(".secret");
    std::fs::create_dir(&protected).unwrap();
    std::fs::write(protected.join("fixture"), "fixture").unwrap();
    let protected_alias = root.path().join("protected-alias");
    symlink(protected.join("fixture"), &protected_alias).unwrap();
    assert!(matches!(
        ReadFileTool
            .open_regular_file(&protected_alias, root.path())
            .await,
        Err(rua_core::RuaError::Security { .. })
    ));
    let directory_alias = root.path().join("directory-alias");
    symlink(root.path(), &directory_alias).unwrap();
    let fifo = root.path().join("fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    for path in [
        root.path(),
        &directory_alias,
        &fifo,
        Path::new("/dev/null"),
        Path::new("media/704d76eb-dee7-47d7-bef3-915891f200db"),
    ] {
        assert!(
            ReadFileTool
                .open_regular_file(path, root.path())
                .await
                .is_err(),
            "{path:?}"
        );
    }
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
    assert!(!directory.is_error);
    assert_eq!(text(&directory), "");

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
async fn edit_cancelled_before_preflight_keeps_all_files_untouched() {
    let root = tempfile::tempdir().unwrap();
    let existing = root.path().join("existing.txt");
    std::fs::write(&existing, "original\n").unwrap();
    let mut ctx = make_ctx(&root);
    let (cancel, receiver) = tokio::sync::watch::channel(false);
    ctx.cancel = Some(receiver);
    cancel.send(true).unwrap();
    let patch = "*** Begin Patch\n*** Update File: existing.txt\n-original\n+changed\n*** Add File: nested/new.txt\n+new\n*** End Patch";

    let result = EditFileTool.call(patch_arg(patch), ctx).await.unwrap();

    assert_eq!(std::fs::read_to_string(existing).unwrap(), "original\n");
    assert!(!root.path().join("nested").exists());
    assert!(result.is_error);
    assert!(text(&result).contains("cancelled"));
    assert_eq!(result.metadata["file_edit_committed_paths"], json!([]));
    assert!(!result.metadata.contains_key("file_edit_unconfirmed_path"));
}

fn cancel_edit_on_io(
    mut ctx: ToolContext,
    patch: &str,
    should_cancel: impl Fn() -> bool,
) -> rua_core::tool::ToolResult {
    let (cancel, receiver) = tokio::sync::watch::channel(false);
    ctx.cancel = Some(receiver);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut call = EditFileTool.call(patch_arg(patch), ctx);
        loop {
            // Poll on the sole filesystem worker so a new I/O operation cannot finish
            // during the same poll. This lets the test cancel between real I/O steps.
            let (next, poll) = tokio::task::spawn_blocking(move || {
                let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
                let poll = call.as_mut().poll(&mut cx);
                (call, poll)
            })
            .await
            .unwrap();
            if let std::task::Poll::Ready(result) = poll {
                break result.unwrap();
            }
            if should_cancel() {
                cancel.send_replace(true);
            }
            call = next;
        }
    })
}

#[test]
fn edit_cancelled_while_rechecking_an_add_never_starts_the_write() {
    let root = tempfile::tempdir().unwrap();
    let patch = "*** Begin Patch\n*** Add File: new.txt\n+new\n*** End Patch";
    // Add preflight is synchronous. Its first pending I/O rechecks the destination.
    let result = cancel_edit_on_io(make_ctx(&root), patch, || true);

    assert!(result.is_error, "{}", text(&result));
    assert!(text(&result).contains("cancelled before writing new.txt"));
    assert_eq!(result.metadata["file_edit_committed_paths"], json!([]));
    assert!(!root.path().join("new.txt").exists());
}

#[test]
fn edit_cancelled_after_the_first_publication_reports_only_that_file() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.txt");
    let second = root.path().join("second.txt");
    std::fs::write(&second, "original\n").unwrap();
    let patch = "*** Begin Patch\n*** Add File: first.txt\n+first\n*** Update File: second.txt\n-original\n+changed\n*** Add File: third.txt\n+third\n*** End Patch";
    let result = cancel_edit_on_io(make_ctx(&root), patch, || first.exists());

    assert!(result.is_error, "{}", text(&result));
    assert!(text(&result).contains("cancelled before writing second.txt"));
    assert!(text(&result).contains("Later patch entries were not attempted"));
    assert_eq!(
        result.metadata["file_edit_committed_paths"],
        json!(["A first.txt"])
    );
    assert!(!result.metadata.contains_key("file_edit_unconfirmed_path"));
    assert_eq!(std::fs::read_to_string(first).unwrap(), "first\n");
    assert_eq!(std::fs::read_to_string(second).unwrap(), "original\n");
    assert!(!root.path().join("third.txt").exists());
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
    assert!(text(&result).contains("locked/second.txt"));
    assert!(!text(&result).contains(root.path().to_str().unwrap()));
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

#[tokio::test]
async fn directory_pages_keep_the_device_and_skip_previously_returned_names() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("photos");
    std::fs::create_dir(&directory).unwrap();
    for name in ["z.jpg", "a.jpg", "m.jpg"] {
        std::fs::write(directory.join(name), []).unwrap();
    }
    let legacy = ReadFileTool
        .call(json!({"file_path":"photos","limit":2}), make_ctx(&root))
        .await
        .unwrap();
    assert_eq!(
        legacy.text_content().matches("Continue with read(").count(),
        1
    );
    assert!(legacy.text_content().contains(r#""file_path":"photos""#));
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());
    ctx.metadata
        .insert("tool_call_id".into(), json!("read-directory"));
    let page = ReadFileTool
        .call(json!({"file_path":"photos","limit":2}), ctx.clone())
        .await
        .unwrap();
    assert!(page.text_content().starts_with("a.jpg\nm.jpg\n"));
    assert_eq!(page.call_id, "read-directory");
    assert_eq!(page.metadata["file_location"]["coverage"], "partial");
    let next = &page.metadata["continuation"];
    assert_eq!(next["after"], "m.jpg");
    assert_eq!(next["device_id"], "desktop");
    assert_eq!(next["file_path"], json!(directory.canonicalize().unwrap()));
    assert_eq!(
        page.text_content().matches("Continue with read(").count(),
        1,
        "{}",
        page.text_content()
    );
    assert!(page
        .text_content()
        .contains(&format!("Continue with read({next}).")));
    let tail = ReadFileTool.call(next.clone(), ctx).await.unwrap();
    assert!(tail.text_content().starts_with("z.jpg\n"));
    assert_eq!(
        tail.metadata["file_location"]["coverage"],
        "requested_scope"
    );
    assert!(!tail.metadata.contains_key("continuation"));
}

#[cfg(unix)]
#[tokio::test]
async fn edit_stops_and_reports_an_unconfirmed_write_when_the_directory_cannot_be_flushed() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let limited = root.path().join("write-only");
    std::fs::create_dir(&limited).unwrap();
    std::fs::set_permissions(&limited, std::fs::Permissions::from_mode(0o300)).unwrap();
    let can_open_directory = std::fs::File::open(&limited).is_ok();
    if can_open_directory {
        std::fs::set_permissions(&limited, std::fs::Permissions::from_mode(0o700)).unwrap();
        return; // A privileged process bypasses this permission failure.
    }
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());
    let patch = "*** Begin Patch\n*** Add File: first.txt\n+first\n*** Add File: write-only/current.txt\n+current\n*** Add File: last.txt\n+last\n*** End Patch";
    let result = EditFileTool.call(patch_arg(patch), ctx).await;
    std::fs::set_permissions(&limited, std::fs::Permissions::from_mode(0o700)).unwrap();
    let result = result.unwrap();
    assert!(result.is_error, "{}", result.text_content());
    assert_eq!(
        result.metadata["file_edit_committed_paths"],
        json!(["A first.txt"])
    );
    assert_eq!(
        result.metadata["file_edit_unconfirmed_path"],
        json!(limited.join("current.txt").canonicalize().unwrap())
    );
    assert_eq!(result.metadata["file_location"]["coverage"], "partial");
    assert!(result.text_content().contains("write-only/current.txt"));
    assert!(result.text_content().contains("not confirmed"));
    assert_eq!(
        std::fs::read_to_string(root.path().join("first.txt")).unwrap(),
        "first\n"
    );
    assert_eq!(
        std::fs::read_to_string(limited.join("current.txt")).unwrap(),
        "current\n"
    );
    assert!(!root.path().join("last.txt").exists());
}

#[tokio::test]
async fn checked_remote_read_reports_target_policy_without_reading_its_local_namesake() {
    use rua_core::tool::{DeviceReadAccess, DeviceReadCheck};

    struct CheckedRead(serde_json::Value);
    #[async_trait::async_trait]
    impl DeviceReadAccess for CheckedRead {
        async fn check(
            &self,
            ctx: &ToolContext,
            args: &serde_json::Value,
        ) -> rua_core::Result<DeviceReadCheck> {
            assert_eq!(args, &self.0);
            assert_eq!(ctx.local_device_id.as_deref(), Some("desktop"));
            Ok(DeviceReadCheck::Checked)
        }
    }

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("photo.txt");
    std::fs::write(&path, "desktop-only marker").unwrap();
    let args = json!({"file_path":path,"device_id":"phone","limit":4});
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());
    ctx.device_read_access = Some(std::sync::Arc::new(CheckedRead(args.clone())));
    let result = ReadFileTool.call(args, ctx).await.unwrap();
    assert!(result.is_error);
    assert_eq!(
        result.metadata["file_location"]["target_permission_check"],
        "checked"
    );
    assert_eq!(result.metadata["file_location"]["dispatched"], false);
    assert!(!result.text_content().contains("desktop-only marker"));
}

#[tokio::test]
async fn selected_other_device_never_opens_or_edits_a_local_namesake() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("photo.txt");
    std::fs::write(&path, "original\n").unwrap();
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());
    let read = ReadFileTool
        .call(json!({"file_path":path,"device_id":"phone"}), ctx.clone())
        .await
        .unwrap();
    assert!(read.is_error);
    let location = &read.metadata["file_location"];
    assert_eq!(location["device_id"], "phone");
    assert_eq!(location["execution_device_id"], "desktop");
    assert_eq!(location["dispatched"], false);
    assert!(read.text_content().contains("no route"));
    assert!(!read.text_content().contains("original"));
    let edit=EditFileTool.call(json!({"patch":format!("*** Begin Patch\n*** Update File: {}\n@@\n-original\n+new\n*** End Patch",path.display()),"device_id":"phone"}),ctx).await.unwrap();
    assert!(edit.is_error);
    assert_eq!(edit.metadata["file_location"]["path"], json!(path));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original\n");
}

#[tokio::test]
async fn explicit_device_requires_a_valid_target_and_a_known_execution_device() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("photo.txt"), "local contents").unwrap();
    for (target, reason) in [
        (json!("phone"), "execution device ID is unknown"),
        (json!(" "), "device_id must be a nonempty string"),
        (json!(42), "device_id must be a nonempty string"),
    ] {
        let result = ReadFileTool
            .call(
                json!({"file_path":"photo.txt","device_id":target}),
                make_ctx(&root),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text_content().contains(reason));
        assert_eq!(result.metadata["file_location"]["dispatched"], false);
        assert!(!result.text_content().contains("local contents"));
    }
}

#[tokio::test]
async fn huge_text_and_binary_inputs_are_bounded_before_formatting() {
    let root = tempfile::tempdir().unwrap();
    let file = std::fs::File::create(root.path().join("sparse.bin")).unwrap();
    file.set_len(2 * 1024 * 1024 * 1024).unwrap();
    let ctx = make_ctx(&root);
    let err = ReadFileTool
        .call(json!({"file_path":"sparse.bin"}), ctx.clone())
        .await
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("binary"));
    std::fs::write(
        root.path().join("long.txt"),
        format!("{}\ntail\n", "a".repeat(65536)),
    )
    .unwrap();
    let mut ctx = ctx;
    ctx.raw_capture_max_bytes = 200;
    let first = ReadFileTool
        .call(json!({"file_path":"long.txt"}), ctx.clone())
        .await
        .unwrap();
    assert!(!first.is_error);
    let local_hint = json!({"file_path":"long.txt","byte_offset":200});
    assert!(first
        .text_content()
        .contains(&format!("Continue with read({local_hint}).")));
    assert_eq!(first.text_content().matches("read(").count(), 1);
    assert_eq!(first.metadata["continuation"]["byte_offset"], 200);
    let second = ReadFileTool
        .call(first.metadata["continuation"].clone(), ctx.clone())
        .await
        .unwrap();
    assert_eq!(second.metadata["byte_range"]["start"], 200);
    assert_eq!(second.metadata["byte_range"]["end_exclusive"], 400);
    let tail = ReadFileTool
        .call(json!({"file_path":"long.txt","start_line":2}), ctx)
        .await
        .unwrap();
    assert_eq!(tail.text_content(), "     2 | tail\n");
}

#[cfg(unix)]
fn android_file_fixture(
    env: &str,
    test: &str,
) -> Option<(std::path::PathBuf, std::path::PathBuf, std::path::PathBuf)> {
    use std::path::PathBuf;

    let Some(root) = std::env::var_os(env).map(PathBuf::from) else {
        // Run in a child so the mobile profile cannot affect other tests in this binary.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env(env, &root)
            .env("RUA_HOST_PROFILE", "mobile-apk")
            .env("RUA_ANDROID_NATIVE_DIR", root.join("native"))
            .env("RUA_ANDROID_DATA_DIR", root.join("data"))
            .env("HOME", root.join("home"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    };
    let backing = root.join("data/runtime/debian");
    let cwd = root.join("home/workspace");
    for directory in [
        root.join("native"),
        backing.join("usr/bin"),
        backing.join("etc"),
        cwd.clone(),
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(root.join("native/libproot.so"), []).unwrap();
    std::fs::write(backing.join("usr/bin/env"), []).unwrap();
    std::fs::write(backing.join("etc/release-marker"), "guest hit\n").unwrap();
    Some((root, backing, cwd))
}

#[cfg(unix)]
#[tokio::test]
async fn opened_regular_read_uses_the_android_guest_to_resolve_file_aliases() {
    use std::path::Path;
    let Some((root, backing, cwd)) = android_file_fixture(
        "RUA_TEST_OPENED_REGULAR_ROOT",
        "file_io_contract::opened_regular_read_uses_the_android_guest_to_resolve_file_aliases",
    ) else {
        return;
    };
    std::os::unix::fs::symlink("/etc/release-marker", backing.join("etc/selected")).unwrap();
    let public = Path::new("/etc/selected");
    let opened = ReadFileTool.open_regular_file(public, &cwd).await.unwrap();
    assert_eq!(opened.public_path(), public);
    assert_eq!(
        opened.resolved_public_path(),
        Path::new("/etc/release-marker")
    );
    assert_eq!(opened.backing_path(), backing.join("etc/release-marker"));
    let ctx = ToolContext {
        working_dir: cwd,
        local_device_id: Some("phone".into()),
        tool_result_store_dir: Some(root.join("tool-results")),
        ..Default::default()
    };
    let read = ReadFileTool
        .read_opened_file(opened, json!({"device_id":"phone","file_path":public}), ctx)
        .await
        .unwrap();
    assert_eq!(read.metadata["file_location"]["path"], "/etc/selected");
    assert!(read.text_content().contains("1 | guest hit"));
    assert!(!read.text_content().contains(backing.to_str().unwrap()));
}

#[cfg(all(unix, feature = "search"))]
#[tokio::test]
async fn android_single_file_grep_returns_only_the_public_guest_path() {
    use rua_tool::extended::search::GrepSearchTool;
    let Some((root, backing, cwd)) = android_file_fixture(
        "RUA_TEST_SINGLE_FILE_GREP_ROOT",
        "file_io_contract::android_single_file_grep_returns_only_the_public_guest_path",
    ) else {
        return;
    };
    let public_file = "/etc/release-marker";
    let result = GrepSearchTool
        .call(
            json!({"pattern":"guest hit","path":public_file,"device_id":"phone"}),
            ToolContext {
                tool_result_store_dir: Some(root.join("tool-results")),
                working_dir: cwd,
                local_device_id: Some("phone".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.text_content());
    assert_eq!(result.metadata["file_location"]["device_id"], "phone");
    assert_eq!(result.metadata["file_location"]["path"], public_file);
    let body = result.text_content();
    assert!(!body.contains(backing.to_str().unwrap()), "{body}");
    assert!(
        body.contains("/etc/release-marker:1:") && body.contains("guest hit"),
        "{body}"
    );
}

#[cfg(unix)]
#[test]
fn mobile_file_locations_resolve_inside_the_guest_and_fail_when_unavailable() {
    use rua_core::runtime_layout::RuntimeLayout;
    use rua_tool::fs::WorkspaceFs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let layout = RuntimeLayout::android(root.join("native"), root.join("data"), root.join("home"));
    let guest = layout.linux_compat.as_ref().unwrap();
    std::fs::create_dir_all(guest.proot_path.parent().unwrap()).unwrap();
    std::fs::write(&guest.proot_path, b"").unwrap();
    std::fs::create_dir_all(guest.rootfs_path.join("usr/bin")).unwrap();
    let env = guest.rootfs_path.join("usr/bin/env");
    std::fs::write(&env, b"").unwrap();
    let cwd = guest.rootfs_path.join("etc/album");
    std::fs::create_dir_all(&cwd).unwrap();
    let file = cwd.join("photo");
    std::fs::write(&file, b"guest").unwrap();
    let fs = WorkspaceFs::for_execution(&cwd, layout.clone()).unwrap();
    assert_eq!(fs.resolve("/etc/album/photo").unwrap(), file);
    assert_eq!(fs.resolve("photo").unwrap(), file);
    std::fs::remove_file(env).unwrap();
    assert!(fs.resolve("/etc/album/photo").is_err());
    assert!(WorkspaceFs::for_execution(&cwd, layout).is_err());
}

#[tokio::test]
async fn file_error_display_keeps_the_location_and_original_error_kind() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    std::fs::write(directory.join("binary.bin"), b"\0data").unwrap();
    std::fs::create_dir(directory.join(".secret")).unwrap();
    std::fs::write(directory.join(".secret/private"), b"data").unwrap();
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());

    for (name, protected) in [("binary.bin", false), (".secret/private", true)] {
        let error = ReadFileTool
            .call(json!({"file_path":name}), ctx.clone())
            .await
            .unwrap_err();
        if protected {
            assert!(matches!(error, rua_core::RuaError::Security { .. }));
        } else {
            assert!(matches!(error, rua_core::RuaError::Tool { .. }));
        }
        let model_message = error.to_string();
        assert!(model_message.contains("desktop"), "{model_message}");
        assert!(
            model_message.contains(&serde_json::to_string(&directory.join(name)).unwrap()),
            "{model_message}"
        );
    }

    let bad_page = ReadFileTool
        .call(json!({"file_path":directory,"limit":0}), ctx)
        .await
        .unwrap();
    assert!(bad_page.is_error && bad_page.text_content().contains("limit"));
    assert!(!bad_page.metadata.contains_key("continuation"));
}

#[tokio::test]
async fn byte_pages_reconstruct_lossy_text_without_splitting_valid_utf8() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a\xe7\x85\xa7\xffA12345\xf0\x9f\x8c\x84z";
    std::fs::write(root.path().join("notes"), bytes).unwrap();
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());
    ctx.raw_capture_max_bytes = 6;
    let mut args = json!({"file_path":"notes","byte_offset":0});
    let mut shown = String::new();
    let mut offset = 0;
    loop {
        let result = ReadFileTool.call(args, ctx.clone()).await.unwrap();
        let agent_types::ContentPart::Text { text } = &result.content[0] else {
            panic!("text byte page")
        };
        assert!(text.len() <= 6);
        shown.push_str(text);
        let range = &result.metadata["byte_range"];
        assert_eq!(range["start"], offset);
        let next = range["end_exclusive"].as_u64().unwrap();
        assert!(next > offset);
        offset = next;
        let Some(continuation) = result.metadata.get("continuation") else {
            break;
        };
        assert_eq!(continuation["byte_offset"], offset);
        assert_eq!(continuation["device_id"], "desktop");
        args = continuation.clone();
    }
    assert_eq!(offset as usize, bytes.len());
    assert_eq!(shown, String::from_utf8_lossy(bytes));
}

#[tokio::test]
async fn device_read_line_continuation_names_one_device_and_path() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("notes.txt");
    std::fs::write(&file, "first\nsecond\nthird\n").unwrap();
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());
    ctx.raw_capture_max_bytes = 23;
    let first = ReadFileTool
        .call(
            json!({"device_id":"desktop","file_path":"notes.txt"}),
            ctx.clone(),
        )
        .await
        .unwrap();
    let next = &first.metadata["continuation"];
    assert_eq!(next["file_path"], json!(file.canonicalize().unwrap()));
    assert_eq!(next["device_id"], "desktop");
    assert_eq!(next["start_line"], 2);
    let hint = format!("read({next})");
    let body = first.text_content();
    assert_eq!(body.matches("read(").count(), 1, "{body}");
    assert!(body.contains(&hint), "{body}");
    assert_eq!(
        first
            .truncation
            .as_ref()
            .unwrap()
            .continuation_hint
            .as_deref(),
        Some(hint.as_str())
    );
    let page = ReadFileTool.call(next.clone(), ctx).await.unwrap();
    assert!(page.text_content().contains("2 | second"));
}

#[tokio::test]
async fn device_read_byte_continuation_names_one_device_and_path() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("notes.txt");
    std::fs::write(&file, "12345678abcdefghABCDEFGH").unwrap();
    let mut ctx = make_ctx(&root);
    ctx.local_device_id = Some("desktop".into());
    ctx.raw_capture_max_bytes = 8;
    let mut args = json!({"device_id":"desktop","file_path":"notes.txt"});
    for expected_offset in [8, 16] {
        let page = ReadFileTool.call(args, ctx.clone()).await.unwrap();
        let next = &page.metadata["continuation"];
        assert_eq!(next["file_path"], json!(file.canonicalize().unwrap()));
        assert_eq!(next["device_id"], "desktop");
        assert_eq!(next["byte_offset"], expected_offset);
        let body = page.text_content();
        assert_eq!(body.matches("read(").count(), 1, "{body}");
        assert!(body.contains(&format!("read({next})")), "{body}");
        args = next.clone();
    }
    let last = ReadFileTool.call(args, ctx).await.unwrap();
    assert!(last.text_content().contains("ABCDEFGH"));
    assert!(!last.metadata.contains_key("continuation"));
}
