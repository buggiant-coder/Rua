use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::{Arc, OnceLock};

use rua_contract::Tool;
use rua_core::runtime_authority::{ProcessShape, ToolchainIntent};
use rua_core::tool::ToolContext;
use rua_tool::builtin::ExecutePythonCodeTool;
use rua_tool::{
    register_all_builtin_tools, CommandExecutor, MockCommandExecutor, MockResponse, ToolRegistry,
};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Mutex;

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

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvRestore {
    prefix: Option<OsString>,
    native_dir: Option<OsString>,
    data_dir: Option<OsString>,
    rua_mobile: Option<OsString>,
    home: Option<OsString>,
    uv_default_index: Option<OsString>,
    rua_python_uv_default_index: Option<OsString>,
    rua_python_index_region: Option<OsString>,
    rua_python_index_mode: Option<OsString>,
}

impl EnvRestore {
    fn capture() -> Self {
        Self {
            prefix: std::env::var_os("PREFIX"),
            native_dir: std::env::var_os("RUA_ANDROID_NATIVE_DIR"),
            data_dir: std::env::var_os("RUA_ANDROID_DATA_DIR"),
            rua_mobile: std::env::var_os("RUA_MOBILE"),
            home: std::env::var_os("HOME"),
            uv_default_index: std::env::var_os("UV_DEFAULT_INDEX"),
            rua_python_uv_default_index: std::env::var_os("RUA_PYTHON_UV_DEFAULT_INDEX"),
            rua_python_index_region: std::env::var_os("RUA_PYTHON_INDEX_REGION"),
            rua_python_index_mode: std::env::var_os("RUA_PYTHON_INDEX_MODE"),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        restore_env("PREFIX", &self.prefix);
        restore_env("RUA_ANDROID_NATIVE_DIR", &self.native_dir);
        restore_env("RUA_ANDROID_DATA_DIR", &self.data_dir);
        restore_env("RUA_MOBILE", &self.rua_mobile);
        restore_env("HOME", &self.home);
        restore_env("UV_DEFAULT_INDEX", &self.uv_default_index);
        restore_env(
            "RUA_PYTHON_UV_DEFAULT_INDEX",
            &self.rua_python_uv_default_index,
        );
        restore_env("RUA_PYTHON_INDEX_REGION", &self.rua_python_index_region);
        restore_env("RUA_PYTHON_INDEX_MODE", &self.rua_python_index_mode);
    }
}

fn restore_env(key: &str, value: &Option<OsString>) {
    match value {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

#[tokio::test]
async fn python_exec_launch_plan_comes_from_runtime_authority() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    std::env::remove_var("RUA_MOBILE");
    let temp = tempfile::tempdir().expect("tempdir");

    let plan = ExecutePythonCodeTool::runtime_authority_python_launch_plan(
        vec!["-I".into(), "-c".into(), "print('ok')".into()],
        temp.path().to_path_buf(),
        vec![("PY_TEST_ENV".into(), "1".into())],
    );

    assert_eq!(plan.process_shape, ProcessShape::Direct);
    assert!(matches!(plan.toolchain_intent, ToolchainIntent::Python));
    assert_eq!(
        plan.process().expect("valid launch").cwd,
        Some(temp.path().to_path_buf())
    );
    assert_eq!(
        plan.process().expect("valid launch").env.get("PY_TEST_ENV"),
        Some(&"1".to_string())
    );
    assert_eq!(
        plan.process().expect("valid launch").args,
        vec!["-I", "-c", "print('ok')"]
    );
}

#[tokio::test]
async fn test_print_hello() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    std::env::remove_var("RUA_MOBILE");
    let tool = ExecutePythonCodeTool::new();
    let args = json!({ "code": "print('hello')" });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    let text = result.text_content();
    assert!(
        text.contains("hello"),
        "expected 'hello' in output, got: {text}"
    );
}

#[tokio::test]
async fn test_exit_code_nonzero() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    std::env::remove_var("RUA_MOBILE");
    let tool = ExecutePythonCodeTool::new();
    let args = json!({ "code": "import sys; sys.exit(42)" });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    assert!(result.is_error);
    let code = result.metadata.get("exit_code").and_then(|v| v.as_i64());
    assert_eq!(code, Some(42));
}

#[tokio::test]
async fn test_timeout_kills_process() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    std::env::remove_var("RUA_MOBILE");
    let tool = ExecutePythonCodeTool::new();
    let args = json!({
        "code": "import time; time.sleep(120)",
        "timeout_secs": 1
    });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    assert!(result.is_error);
    let timed_out = result
        .metadata
        .get("timed_out")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(timed_out, "expected timed_out=true");
}

#[tokio::test]
async fn test_tmp_dir_isolation() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    std::env::remove_var("RUA_MOBILE");
    let tool = ExecutePythonCodeTool::new();
    let marker = format!("rua_test_{}.txt", std::process::id());
    let code =
        format!("with open('{marker}', 'w') as f: f.write('test')\nimport os; print(os.getcwd())");
    let args = json!({ "code": code });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    let text = result.text_content();

    assert!(
        !std::env::current_dir().unwrap().join(&marker).exists(),
        "python wrote file into cwd — isolation broken"
    );

    let cwd = std::env::current_dir().unwrap();
    assert!(
        !text.trim().ends_with(cwd.to_str().unwrap()),
        "subprocess ran in cwd instead of sandbox"
    );

    let sandbox = std::env::temp_dir().join("rua-python-sandbox");
    let _ = std::fs::remove_file(sandbox.join(&marker));
}

#[tokio::test]
async fn test_stderr_captured() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    std::env::remove_var("RUA_MOBILE");
    let tool = ExecutePythonCodeTool::new();
    let args = json!({ "code": "import sys; sys.stderr.write('warning\\n')" });
    let ctx = make_ctx();
    let result = tool.call(args, ctx.context).await.unwrap();
    let text = result.text_content();
    assert!(
        text.contains("warning"),
        "expected stderr content in output, got: {text}"
    );
}

#[tokio::test]
async fn mobile_registered_python_exec_routes_to_linux_compat_python() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    std::env::set_var("RUA_MOBILE", "1");
    std::env::set_var("RUA_PYTHON_INDEX_REGION", "cn");
    std::env::remove_var("RUA_PYTHON_UV_DEFAULT_INDEX");
    std::env::remove_var("UV_DEFAULT_INDEX");

    let working_dir = tempfile::tempdir().expect("tempdir");
    configure_mock_mobile_layout(working_dir.path());
    std::env::set_var("HOME", working_dir.path());

    let mut mock = MockCommandExecutor::new();
    mock.set_default_response(MockResponse {
        stdout: "hello\n".into(),
        stderr: String::new(),
        exit_code: 0,
        delay_ms: 0,
    });
    let mock = Arc::new(mock);
    let executor: Arc<dyn CommandExecutor> = mock.clone();
    let mut registry = ToolRegistry::new();
    register_all_builtin_tools(&mut registry, executor, None).expect("register tools");
    let tool = registry.get("python").expect("python").clone();
    let mut ctx = make_ctx();
    ctx.context.working_dir = working_dir.path().to_path_buf();

    let result = tool
        .call(json!({ "code": "print('hello')" }), ctx.context)
        .await
        .expect("python tool call");

    assert!(!result.is_error);
    assert!(result.text_content().contains("hello"));
    let history = mock.history();
    assert_eq!(history.len(), 1);
    assert!(
        history[0].command.starts_with("PY='/usr/bin/python3'"),
        "mobile python must use the FHS Python path so the Android router selects proot, got: {}",
        history[0].command
    );
    assert!(history[0]
        .command
        .contains("uv venv --python \"$PY\" \"$VENV\""));
    assert!(history[0]
        .command
        .contains("uv pip install --python \"$VENV/bin/python\""));
    assert!(history[0].command.contains("ModuleNotFoundError"));
    assert!(history[0].command.contains("retrying with uv pip install"));
    assert!(history[0]
        .env
        .contains(&("UV_PYTHON_DOWNLOADS".into(), "never".into())));
    assert!(history[0]
        .env
        .contains(&("UV_CACHE_DIR".into(), "/root/.cache/uv".into())));
    assert!(history[0]
        .env
        .contains(&("RUA_PYTHON_VENV".into(), "/root/.venv/rua-mobile".into())));
    assert!(history[0].env.contains(&(
        "UV_DEFAULT_INDEX".into(),
        "https://pypi.tuna.tsinghua.edu.cn/simple".into(),
    )));
    assert_eq!(
        history[0].working_dir,
        working_dir.path().join(".rua-python-sandbox")
    );
}

#[tokio::test]
async fn mobile_python_exec_passes_declared_packages_to_uv() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    std::env::set_var("RUA_MOBILE", "1");
    std::env::set_var("RUA_PYTHON_INDEX_REGION", "official");

    let working_dir = tempfile::tempdir().expect("tempdir");
    configure_mock_mobile_layout(working_dir.path());
    let mock = Arc::new(MockCommandExecutor::new());
    let executor: Arc<dyn CommandExecutor> = mock.clone();
    let mut registry = ToolRegistry::new();
    register_all_builtin_tools(&mut registry, executor, None).expect("register tools");
    let mut ctx = make_ctx();
    ctx.context.working_dir = working_dir.path().to_path_buf();

    registry
        .get("python")
        .expect("python")
        .call(
            json!({
                "code": "import requests",
                "packages": ["requests", "Pillow"]
            }),
            ctx.context,
        )
        .await
        .expect("python tool call");

    let history = mock.history();
    assert_eq!(history.len(), 1);
    assert!(
        history[0].command.contains("DEPS='requests Pillow'"),
        "declared packages must seed shared uv venv deps, got: {}",
        history[0].command
    );
    assert!(
        !history[0]
            .env
            .iter()
            .any(|(key, _)| key == "UV_DEFAULT_INDEX"),
        "official index mode must not force a mirror: {:?}",
        history[0].env
    );
}

#[tokio::test]
async fn missing_mobile_guest_never_executes_shell_or_python_on_host() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    let ctx = make_ctx();
    std::env::set_var("RUA_MOBILE", "1");
    std::env::set_var("PREFIX", ctx._root.path().join("absent-userland"));
    std::env::set_var("HOME", ctx._root.path());
    let sentinel = ctx._root.path().join("host-executed");
    let python = ExecutePythonCodeTool::new()
        .call(
            json!({"code": format!("open({}, 'w').write('unexpected')", serde_json::to_string(&sentinel).unwrap())}),
            ctx.context.clone(),
        )
        .await;
    assert!(
        python.is_err(),
        "missing guest must reject Python before spawn"
    );
    assert!(python
        .unwrap_err()
        .to_string()
        .contains("Linux Compatibility"));
    let (_, cancel) = tokio::sync::watch::channel(false);
    let shell = rua_tool::RuntimeCommandExecutor::new()
        .execute(
            rua_tool::CommandRequest {
                command: format!("touch '{}'", sentinel.display()),
                working_dir: ctx.context.working_dir.clone(),
                tool_result_store_dir: ctx.context.tool_result_store_dir.clone().unwrap(),
                timeout: std::time::Duration::from_secs(2),
                env: vec![],
                env_remove: vec![],
                raw_capture_max_bytes: 1024,
                raw_capture_id: None,
                task_session: None,
            },
            cancel,
        )
        .await;
    assert!(
        shell.is_err(),
        "missing guest must reject Shell before spawn"
    );
    assert!(shell
        .unwrap_err()
        .to_string()
        .contains("Linux Compatibility"));
    assert!(
        !sentinel.exists(),
        "neither host interpreter may execute the command"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn direct_python_stop_kills_its_child_and_grandchild() {
    check_python_process_cleanup(false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_python_future_kills_its_child_and_grandchild() {
    check_python_process_cleanup(true).await;
}

#[cfg(unix)]
async fn check_python_process_cleanup(abort: bool) {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    std::env::remove_var("RUA_MOBILE");
    let mut fixture = make_ctx();
    let marker = fixture._root.path().join("python-pids");
    let code = format!("import os,sys,subprocess,time; p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); open({:?},'w').write(str(os.getpid())+' '+str(p.pid)); time.sleep(60)", marker.to_str().unwrap());
    let (cancel, receiver) = tokio::sync::watch::channel(false);
    fixture.context.cancel = Some(receiver);
    let context = fixture.context.clone();
    // RLIMIT_NPROC is per user: don't let the developer's other processes prevent the fixture child.
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NPROC, &mut limit) },
        0
    );
    let task = tokio::spawn(async move {
        let tool = ExecutePythonCodeTool::with_limits(rua_tool::builtin::SandboxLimits {
            max_procs: limit.rlim_cur,
            ..Default::default()
        });
        tool.call(json!({"code": code, "timeout_secs": 30}), context)
            .await
    });
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    if !marker.exists() {
        cancel.send(true).unwrap();
        panic!("Python descendants never started: {:?}", task.await);
    }
    let pids: Vec<i32> = std::fs::read_to_string(&marker)
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    if abort {
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    } else {
        cancel.send(true).unwrap();
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(output.metadata["cancelled"], json!(true));
    }
    for pid in pids {
        for _ in 0..100 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "Python descendant {pid} survived cancellation"
        );
    }
}

// These tests assert routing with a mock executor; readiness still uses the real authority.
fn configure_mock_mobile_layout(root: &std::path::Path) {
    let native = root.join("native");
    for file in [
        "native/libproot.so",
        "runtime/debian/usr/bin/env",
        "runtime/debian/usr/bin/python3",
    ] {
        let file = root.join(file);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "fixture").unwrap();
    }
    std::env::set_var("RUA_ANDROID_NATIVE_DIR", native);
    std::env::set_var("RUA_ANDROID_DATA_DIR", root);
    std::env::set_var("HOME", root);
}
