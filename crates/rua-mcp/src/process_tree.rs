use std::time::Duration;

use tokio::process::Child;

pub(crate) async fn kill_child_process_tree(child: &mut Child) {
    kill_process_tree(child.id());
    let _ = child.kill().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
}

pub(crate) fn start_kill_child_process_tree(child: &mut Child) {
    kill_process_tree(child.id());
    let _ = child.start_kill();
}

fn kill_process_tree(pid: Option<u32>) {
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

    let pid = pid.to_string();
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", pid.as_str(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
