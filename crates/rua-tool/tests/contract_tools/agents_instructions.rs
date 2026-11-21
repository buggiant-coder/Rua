use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rua_core::agents_instruction_discovery::{
    AgentsInstructionDiscovery, AgentsInstructionDiscoveryRequest, AgentsInstructionTargetKind,
    AGENTS_INSTRUCTIONS_MAX_BYTES,
};
use rua_tool::agents_instructions::discover_agents_instructions;
use rua_tool::WorkspaceFs;
use tokio_util::sync::CancellationToken;

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    fs: WorkspaceFs,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let fs = WorkspaceFs::new(&root).unwrap();
        Self {
            _directory: directory,
            root,
            fs,
        }
    }

    fn write(&self, path: &str, text: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn request(&self, path: &str) -> AgentsInstructionDiscoveryRequest {
        AgentsInstructionDiscoveryRequest {
            device_id: "phone".into(),
            target_path: self.root.join(path).to_str().unwrap().into(),
            max_bytes: AGENTS_INSTRUCTIONS_MAX_BYTES,
            ..Default::default()
        }
    }

    async fn discover(
        &self,
        request: &AgentsInstructionDiscoveryRequest,
    ) -> AgentsInstructionDiscovery {
        discover_agents_instructions(&self.fs, request, &CancellationToken::new(), |_, _| async {
            true
        })
        .await
        .unwrap()
    }
}

fn contents(result: &AgentsInstructionDiscovery) -> Vec<&str> {
    result
        .sources
        .iter()
        .map(|source| source.content.as_str())
        .collect()
}

#[test]
fn discovery_dtos_preserve_unknown_fields() {
    let request = serde_json::json!({
        "device_id": "phone", "target_path": "/photos", "target_kind": "directory",
        "checked_directories": [], "max_bytes": 10, "future_request": {"enabled": true}
    });
    let parsed: AgentsInstructionDiscoveryRequest =
        serde_json::from_value(request.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), request);
    let discovery = serde_json::json!({
        "target_directory": "/photos", "checked_directories": ["/photos"],
        "sources": [{"device_id": "phone", "path": "/photos/AGENTS.md", "scope_directory": "/photos",
            "content": "photos", "source_bytes": 6, "truncated": false, "future_source": "value"}],
        "diagnostics": [], "truncated": false, "future_discovery": 42
    });
    let parsed: AgentsInstructionDiscovery = serde_json::from_value(discovery.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), discovery);
}

#[tokio::test]
async fn nearest_git_root_and_override_define_the_ordered_chain() {
    let f = Fixture::new();
    f.write(".git", "outer root");
    std::fs::create_dir_all(f.root.join("repo/.git")).unwrap();
    f.write("AGENTS.md", "must not load outer root");
    f.write("repo/AGENTS.md", "repository");
    f.write("repo/src/AGENTS.md", "shadowed");
    f.write("repo/src/AGENTS.override.md", "specific");
    f.write("repo/src/nested/AGENTS.md", "leaf");
    let file = f.write("repo/src/nested/code.rs", "code");
    let result = f.discover(&f.request("repo/src/nested/code.rs")).await;
    assert_eq!(contents(&result), ["repository", "specific", "leaf"]);
    assert_eq!(
        result.target_directory.as_deref(),
        file.parent().unwrap().to_str()
    );
    assert_eq!(
        result.checked_directories,
        ["repo", "repo/src", "repo/src/nested"].map(|p| f
            .root
            .join(p)
            .to_str()
            .unwrap()
            .to_owned())
    );
    for source in &result.sources {
        assert_eq!(source.device_id, "phone");
        assert_eq!(
            Path::new(&source.path).parent().unwrap().to_str(),
            Some(source.scope_directory.as_str())
        );
        assert_eq!(source.source_bytes, source.content.len() as u64);
        assert!(!source.truncated);
    }
}

#[tokio::test]
async fn worktree_marker_and_missing_edit_target_use_the_repo_chain() {
    let f = Fixture::new();
    f.write("repo/.git", "gitdir: elsewhere");
    f.write("repo/AGENTS.md", "repository");
    let request = AgentsInstructionDiscoveryRequest {
        target_kind: AgentsInstructionTargetKind::File,
        ..f.request("repo/new/nested/file.txt")
    };
    let result = f.discover(&request).await;
    assert_eq!(contents(&result), ["repository"]);
    assert_eq!(
        result.target_directory,
        Some(f.root.join("repo/new/nested").to_str().unwrap().into())
    );
    assert_eq!(result.checked_directories.len(), 3);
}

#[tokio::test]
async fn without_a_repository_only_the_target_directory_is_loaded() {
    let f = Fixture::new();
    f.write("AGENTS.md", "parent is not a project root");
    f.write("work/AGENTS.md", "cwd");
    f.write("work/child/AGENTS.md", "not scanned");
    let result = f.discover(&f.request("work")).await;
    assert_eq!(contents(&result), ["cwd"]);
    assert_eq!(
        result.checked_directories,
        [f.root.join("work").to_str().unwrap()]
    );
}

#[tokio::test]
async fn empty_or_unreadable_override_falls_back_without_blocking_other_scopes() {
    let f = Fixture::new();
    f.write("repo/.git", "root");
    f.write("repo/AGENTS.override.md", " \n\t");
    f.write("repo/AGENTS.md", "root fallback");
    f.write("repo/nested/AGENTS.override.md", "denied content");
    f.write("repo/nested/AGENTS.md", "nested fallback");
    let denied = f.root.join("repo/nested/AGENTS.override.md");
    let result = discover_agents_instructions(
        &f.fs,
        &f.request("repo/nested"),
        &CancellationToken::new(),
        |path, _| {
            let permitted = path != denied;
            async move { permitted }
        },
    )
    .await
    .unwrap();
    assert_eq!(contents(&result), ["root fallback", "nested fallback"]);
    assert_eq!(result.checked_directories.len(), 2);
    assert!(!result.diagnostics.is_empty());
}

#[tokio::test]
async fn unreadable_and_non_text_files_are_skipped_and_recorded_as_checked() {
    let f = Fixture::new();
    f.write("repo/.git", "root");
    f.write("repo/AGENTS.md", b"binary\0data");
    f.write("repo/a/AGENTS.override.md", [0xff, 0xff]);
    f.write("repo/a/AGENTS.md", "usable");
    std::fs::create_dir_all(f.root.join("repo/a/b/AGENTS.md")).unwrap();
    let result = f.discover(&f.request("repo/a/b")).await;
    assert_eq!(contents(&result), ["usable"]);
    assert_eq!(result.checked_directories.len(), 3);
    assert_eq!(result.diagnostics.len(), 3);
}

#[tokio::test]
async fn combined_budget_is_bounded_and_utf8_is_never_split() {
    let f = Fixture::new();
    f.write("repo/.git", "root");
    f.write(
        "repo/AGENTS.md",
        "a".repeat(AGENTS_INSTRUCTIONS_MAX_BYTES - 5),
    );
    f.write("repo/nested/AGENTS.md", "照片📷tail");
    let mut request = f.request("repo/nested");
    request.max_bytes = usize::MAX;
    let result = f.discover(&request).await;
    assert_eq!(result.sources.len(), 2);
    assert_eq!(result.sources[1].content, "照");
    assert_eq!(result.sources[1].source_bytes, "照片📷tail".len() as u64);
    assert!(result.sources[1].truncated && result.truncated);
    assert!(
        result
            .sources
            .iter()
            .map(|s| s.content.len())
            .sum::<usize>()
            <= AGENTS_INSTRUCTIONS_MAX_BYTES
    );
}

#[tokio::test]
async fn a_small_budget_does_not_select_a_lower_priority_file() {
    let f = Fixture::new();
    f.write("AGENTS.override.md", "📷");
    f.write("AGENTS.md", "wrong fallback");
    let request = AgentsInstructionDiscoveryRequest {
        max_bytes: 1,
        ..f.request("")
    };
    let result = f.discover(&request).await;
    assert!(result.sources.is_empty());
    assert!(result.truncated);
}

#[tokio::test]
async fn checked_ancestors_are_not_reread_or_charged_against_the_remaining_budget() {
    let f = Fixture::new();
    f.write("repo/.git", "root");
    f.write("repo/AGENTS.md", "captured");
    std::fs::create_dir_all(f.root.join("repo/a")).unwrap();
    let first = f.discover(&f.request("repo/a")).await;
    f.write("repo/AGENTS.md", "x".repeat(AGENTS_INSTRUCTIONS_MAX_BYTES));
    f.write("repo/a/AGENTS.md", "appeared after this scope was checked");
    f.write("repo/a/b/AGENTS.md", "new");
    let request = AgentsInstructionDiscoveryRequest {
        checked_directories: first.checked_directories,
        max_bytes: 3,
        ..f.request("repo/a/b")
    };
    let result = f.discover(&request).await;
    assert_eq!(contents(&result), ["new"]);
    assert_eq!(
        result.checked_directories,
        [f.root.join("repo/a/b").to_str().unwrap()]
    );
    let next_turn = f.discover(&f.request("repo/a")).await;
    assert_eq!(
        next_turn.sources[0].content.len(),
        AGENTS_INSTRUCTIONS_MAX_BYTES
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_targets_keep_real_sources_and_refresh_without_a_cross_turn_cache() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    for repo in ["first", "second"] {
        f.write(&format!("{repo}/.git"), "root");
        f.write(&format!("{repo}/AGENTS.md"), repo);
        std::fs::create_dir_all(f.root.join(repo).join("nested")).unwrap();
    }
    symlink(f.root.join("first/nested"), f.root.join("alias")).unwrap();
    let result = f.discover(&f.request("alias")).await;
    assert_eq!(contents(&result), ["first"]);
    assert_eq!(
        result.target_directory,
        Some(f.root.join("first/nested").to_str().unwrap().into())
    );
    std::fs::remove_file(f.root.join("alias")).unwrap();
    symlink(f.root.join("second/nested"), f.root.join("alias")).unwrap();
    let text = f.write("actual.md", "linked content");
    symlink(&text, f.root.join("second/nested/AGENTS.md")).unwrap();
    let result = f.discover(&f.request("alias")).await;
    assert_eq!(contents(&result), ["second", "linked content"]);
    assert_eq!(result.sources[1].path, text.to_str().unwrap());
    assert_eq!(
        result.sources[1].scope_directory,
        f.root.join("second/nested").to_str().unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ordinary_hardlinked_instructions_do_not_inherit_the_remote_page_restriction() {
    let f = Fixture::new();
    let content = f.write("content.md", "shared local instructions");
    std::fs::hard_link(content, f.root.join("AGENTS.md")).unwrap();
    let result = f.discover(&f.request("")).await;
    assert_eq!(contents(&result), ["shared local instructions"]);
    std::fs::remove_file(f.root.join("AGENTS.md")).unwrap();
    let next = f.discover(&f.request("")).await;
    assert!(
        next.sources.is_empty(),
        "a new discovery cannot reuse an earlier file's content"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn windows_instruction_sources_use_the_same_drive_paths_as_permission_records() {
    let f = Fixture::new();
    f.write("AGENTS.md", "local drive instructions");
    let root = f.root.to_str().unwrap().strip_prefix(r"\\?\").unwrap();
    let result = discover_agents_instructions(
        &f.fs,
        &f.request(""),
        &CancellationToken::new(),
        |candidate, resolved| async move {
            assert!(!candidate.to_str().unwrap().starts_with(r"\\?\"));
            assert!(!resolved.to_str().unwrap().starts_with(r"\\?\"));
            true
        },
    )
    .await
    .unwrap();
    assert_eq!(contents(&result), ["local drive instructions"]);
    assert_eq!(result.target_directory.as_deref(), Some(root));
    assert_eq!(result.sources[0].scope_directory, root);
}

#[cfg(unix)]
#[tokio::test]
async fn permission_failures_and_fifos_do_not_block_discovery() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let denied = f.write("AGENTS.override.md", "unreadable");
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
    f.write("AGENTS.md", "fallback");
    let result = f.discover(&f.request("")).await;
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(contents(&result), ["fallback"]);
    std::fs::remove_file(&denied).unwrap();
    let fifo = std::ffi::CString::new(denied.to_str().unwrap()).unwrap();
    // The reader must reject special files before a blocking open or read.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        f.discover(&f.request("")),
    )
    .await
    .unwrap();
    assert_eq!(contents(&result), ["fallback"]);
}

#[tokio::test]
async fn cancellation_preempts_discovery_and_a_pending_authorization_callback() {
    let f = Fixture::new();
    f.write("AGENTS.md", "must not be returned");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = discover_agents_instructions(&f.fs, &f.request(""), &cancel, |_, _| async {
        panic!("cancelled before authorization")
    })
    .await;
    assert!(matches!(result, Err(rua_core::RuaError::Cancelled)));
    let cancel = CancellationToken::new();
    let observed = Arc::new(tokio::sync::Notify::new());
    let request = f.request("");
    let operation = discover_agents_instructions(&f.fs, &request, &cancel, |_, _| {
        let observed = observed.clone();
        async move {
            observed.notify_one();
            std::future::pending::<bool>().await
        }
    });
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => panic!("unexpected completion: {result:?}"),
        _ = observed.notified() => {},
    }
    cancel.cancel();
    assert!(matches!(
        operation.await,
        Err(rua_core::RuaError::Cancelled)
    ));
}

#[tokio::test]
async fn denied_paths_are_not_opened_and_revoked_content_is_not_returned() {
    let f = Fixture::new();
    let path = f.write("AGENTS.md", "must not be adopted");
    let observed = Mutex::new(Vec::new());
    let result = discover_agents_instructions(
        &f.fs,
        &f.request(""),
        &CancellationToken::new(),
        |candidate, resolved| {
            observed.lock().unwrap().push((candidate, resolved));
            // Removing the file on denial demonstrates that no open/read is needed afterwards.
            std::fs::remove_file(&path).unwrap();
            async { false }
        },
    )
    .await
    .unwrap();
    assert_eq!(observed.lock().unwrap().len(), 1);
    assert!(result.sources.is_empty());
    f.write("AGENTS.md", "revoked after opening");
    let checks = std::sync::atomic::AtomicUsize::new(0);
    let result =
        discover_agents_instructions(&f.fs, &f.request(""), &CancellationToken::new(), |_, _| {
            let allowed = checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2;
            async move { allowed }
        })
        .await
        .unwrap();
    assert_eq!(checks.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert!(result.sources.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn android_discovery_stays_in_public_paths_and_never_walks_host_ancestors() {
    use rua_core::runtime_layout::RuntimeLayout;
    let f = Fixture::new();
    f.write(".git", "host root");
    f.write("AGENTS.md", "host instructions must not load");
    let layout = RuntimeLayout::android(
        f.root.join("native"),
        f.root.join("data"),
        f.root.join("home"),
    );
    let guest = layout.linux_compat.as_ref().unwrap();
    std::fs::create_dir_all(guest.proot_path.parent().unwrap()).unwrap();
    std::fs::write(&guest.proot_path, "").unwrap();
    std::fs::create_dir_all(guest.rootfs_path.join("usr/bin")).unwrap();
    std::fs::write(guest.rootfs_path.join("usr/bin/env"), "").unwrap();
    let cwd = guest.rootfs_path.join("photos");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(
        guest.rootfs_path.join("AGENTS.md"),
        "guest parent must not load without .git",
    )
    .unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "guest photos").unwrap();
    let fs = WorkspaceFs::for_execution(&cwd, layout).unwrap();
    let request = AgentsInstructionDiscoveryRequest {
        target_path: "/photos".into(),
        ..f.request("")
    };
    let result = discover_agents_instructions(
        &fs,
        &request,
        &CancellationToken::new(),
        |candidate, resolved| async move {
            assert_eq!(candidate, Path::new("/photos/AGENTS.md"));
            assert_eq!(resolved, Path::new("/photos/AGENTS.md"));
            true
        },
    )
    .await
    .unwrap();
    assert_eq!(contents(&result), ["guest photos"]);
    assert_eq!(result.target_directory.as_deref(), Some("/photos"));
    assert_eq!(result.checked_directories, ["/photos"]);
    assert_eq!(result.sources[0].path, "/photos/AGENTS.md");
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains(f.root.to_str().unwrap()));
}
