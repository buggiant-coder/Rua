use tokio::process::Child;

/// Owns the whole process group even when the caller's future is dropped.
/// The command must create its own process group before spawn.
pub(crate) struct ChildProcessTree(Child);

impl ChildProcessTree {
    pub(crate) fn new(child: Child) -> Self {
        Self(child)
    }
}

impl std::ops::Deref for ChildProcessTree {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for ChildProcessTree {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for ChildProcessTree {
    fn drop(&mut self) {
        kill_process_tree(self.0.id());
        // kill_on_drop on Child reaps the direct child after group termination.
    }
}

pub(crate) async fn kill_child_process_tree(child: &mut Child) {
    kill_process_tree(child.id());
    let _ = child.kill().await;
    let _ = child.wait().await;
}

pub(crate) fn kill_process_tree(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }

    if cfg!(windows) {
        kill_windows_process_tree(pid);
    }
}

fn kill_windows_process_tree(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };

    let _ = std::process::Command::new("taskkill")
        .args(windows_taskkill_args(pid))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

pub(crate) fn windows_taskkill_args(pid: u32) -> Vec<String> {
    vec![
        "/PID".to_string(),
        pid.to_string(),
        "/T".to_string(),
        "/F".to_string(),
    ]
}
