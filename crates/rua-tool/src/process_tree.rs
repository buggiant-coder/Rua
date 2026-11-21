use tokio::process::Child;

/// Owns the whole process group even when the caller's future is dropped.
/// The command must create its own process group before spawn.
pub(crate) struct ChildProcessTree {
    child: Child,
    group_id: Option<u32>,
}

impl ChildProcessTree {
    pub(crate) fn new(child: Child) -> Self {
        let group_id = child.id();
        Self { child, group_id }
    }

    fn termination_id(&self) -> Option<u32> {
        if cfg!(unix) {
            // Tokio clears Child::id after wait, but descendants can still own
            // this process group and the command's output pipes.
            self.group_id
        } else {
            self.child.id()
        }
    }
}

impl std::ops::Deref for ChildProcessTree {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}

impl std::ops::DerefMut for ChildProcessTree {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

impl Drop for ChildProcessTree {
    fn drop(&mut self) {
        kill_process_tree(self.termination_id());
        // kill_on_drop on Child reaps the direct child after group termination.
    }
}

pub(crate) async fn kill_child_process_tree(child: &mut ChildProcessTree) {
    kill_process_tree(child.termination_id());
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
