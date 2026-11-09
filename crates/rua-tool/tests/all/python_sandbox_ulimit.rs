#![cfg(unix)]

use std::collections::HashMap;

use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::builtin::ExecutePythonCodeTool;
use serde_json::json;
use tempfile::TempDir;

struct TestContext {
    context: ToolContext,
    _root: TempDir,
}

fn make_ctx() -> TestContext {
    let root = tempfile::tempdir().expect("tool context root");
    let tool_result_store_dir = root.path().join("tool-results");
    std::fs::create_dir_all(&tool_result_store_dir).expect("tool result store");
    TestContext {
        context: ToolContext {
            agent_id: "test".into(),
            session_id: "test-session".into(),
            working_dir: root.path().to_path_buf(),
            tool_result_store_dir: Some(tool_result_store_dir),
            metadata: HashMap::new(),
            cancel: None,
            task_runtime: None,
            ..Default::default()
        },
        _root: root,
    }
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn test_memory_limit_oom() {
    let tool = ExecutePythonCodeTool::new();

    let code = r#"
try:
    x = bytearray(512 * 1024 * 1024)
except MemoryError:
    import sys
    sys.stderr.write('MemoryError\n')
    sys.exit(137)
"#;
    let args = json!({ "code": code, "timeout_secs": 10 });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    assert!(result.is_error, "expected error result for OOM");
    let text = result.text_content();
    assert!(
        text.contains("memory limit") || text.contains("MemoryError") || text.contains("memory"),
        "expected memory limit hint in output, got: {text}"
    );
}

#[tokio::test]
async fn test_fork_bomb_limited() {
    let tool = ExecutePythonCodeTool::new();

    let code = r#"
import os, sys
children = []
for i in range(200):
    try:
        pid = os.fork()
        if pid == 0:
            os._exit(0)
        children.append(pid)
    except OSError as e:
        sys.stderr.write(f'fork failed: {e}\nResource temporarily unavailable\n')
        sys.exit(1)
for p in children:
    os.waitpid(p, 0)
"#;
    let args = json!({ "code": code, "timeout_secs": 10 });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    assert!(result.is_error, "expected error result for fork bomb");
    let text = result.text_content();
    assert!(
        text.contains("process limit")
            || text.contains("Resource temporarily unavailable")
            || text.contains("fork"),
        "expected process limit hint in output, got: {text}"
    );
}

#[tokio::test]
async fn test_file_size_limited() {
    let tool = ExecutePythonCodeTool::new();

    let code = r#"
import os, sys, tempfile, signal

def handler(signum, frame):
    sys.stderr.write('file size limit exceeded\n')
    sys.exit(1)

signal.signal(signal.SIGXFSZ, handler)

with tempfile.NamedTemporaryFile(delete=True) as f:
    try:
        f.write(b'x' * (20 * 1024 * 1024))
        f.flush()
    except (IOError, OSError) as e:
        sys.stderr.write(f'write failed: {e}\n')
        sys.exit(1)
"#;
    let args = json!({ "code": code, "timeout_secs": 10 });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    assert!(result.is_error, "expected error result for file size limit");
    let text = result.text_content();
    assert!(
        text.contains("file size limit")
            || text.contains("RLIMIT_FSIZE")
            || text.contains("SIGXFSZ"),
        "expected file size limit hint in output, got: {text}"
    );
}

#[tokio::test]
async fn test_normal_code_still_works() {
    let tool = ExecutePythonCodeTool::new();
    let args = json!({ "code": "print('sandbox ok')" });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    assert!(!result.is_error);
    let text = result.text_content();
    assert!(text.contains("sandbox ok"));
}

#[tokio::test]
async fn test_custom_limits_configurable() {
    use rua_tool::builtin::SandboxLimits;

    let limits = SandboxLimits {
        memory_mb: 256,
        max_procs: 64,
        max_file_mb: 1,
    };
    let tool = ExecutePythonCodeTool::with_limits(limits);
    let code = r#"
import tempfile, signal, sys

def handler(signum, frame):
    sys.stderr.write('file size limit exceeded\n')
    sys.exit(1)

signal.signal(signal.SIGXFSZ, handler)

with tempfile.NamedTemporaryFile(delete=True) as f:
    try:
        f.write(b'x' * (2 * 1024 * 1024))
        f.flush()
    except (IOError, OSError) as e:
        sys.stderr.write(f'write failed: {e}\n')
        sys.exit(1)
"#;
    let args = json!({ "code": code, "timeout_secs": 10 });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    assert!(result.is_error, "expected error with 1MB fsize limit");
}

#[tokio::test]
async fn test_relaxed_limits_allow_larger_writes() {
    use rua_tool::builtin::SandboxLimits;

    let limits = SandboxLimits {
        memory_mb: 256,
        max_procs: 64,
        max_file_mb: 50,
    };
    let tool = ExecutePythonCodeTool::with_limits(limits);
    let code = r#"
import tempfile
with tempfile.NamedTemporaryFile(delete=True) as f:
    f.write(b'x' * (5 * 1024 * 1024))
    f.flush()
print('write ok')
"#;
    let args = json!({ "code": code, "timeout_secs": 10 });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    assert!(!result.is_error, "expected success with 50MB fsize limit");
    let text = result.text_content();
    assert!(text.contains("write ok"));
}
