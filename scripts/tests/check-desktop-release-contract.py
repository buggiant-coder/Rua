#!/usr/bin/env python3


from __future__ import annotations

import json
import os
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
TAURI = ROOT / "console" / "src-tauri"


class DesktopReleaseContractTests(unittest.TestCase):

    @unittest.skipIf(os.name == "nt", "Desktop packaging shell runs under Git Bash on Windows")
    def test_preview_collection_excludes_deb_internal_archives(self) -> None:
        import shutil
        with tempfile.TemporaryDirectory(prefix="rua bundle ") as temp:
            root = Path(temp)
            scripts = root / "scripts/ci"
            scripts.mkdir(parents=True)
            shutil.copy2(ROOT / "scripts/ci/build-desktop.sh", scripts / "build-desktop.sh")
            (scripts / "prepare-desktop-bundle.sh").write_text("exit 0\n")
            console = root / "console"
            bundle = console / "src-tauri/target/x86_64-unknown-linux-gnu/release/bundle/deb"
            bundle.mkdir(parents=True)
            commands = root / "bin"
            commands.mkdir()
            npx = commands / "npx"
            npx.write_text("#!/bin/sh\n" +
                           f"touch '{bundle}/Rua_0.8.0_amd64.deb' " +
                           f"'{bundle}/control.tar.gz' '{bundle}/data.tar.gz'\n")
            npx.chmod(0o755)
            result = subprocess.run(
                ["bash", str(scripts / "build-desktop.sh"), "x86_64-unknown-linux-gnu", "--test-package"],
                env={**os.environ, "PATH": str(commands) + os.pathsep + os.environ["PATH"]},
                capture_output=True, text=True, timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            files = list((root / "target/dist/desktop/x86_64-unknown-linux-gnu").iterdir())
            self.assertEqual([path.suffix for path in files], [".deb"])

    def test_release_draft_rejects_mismatched_source_and_failed_installer_checks(self) -> None:
        import copy
        import sys
        workflow = (ROOT / ".github/workflows/build-test-apps.yml").read_text()
        source = workflow.split("          python3 - <<'PYTHON'\n", 1)[1].split("          PYTHON\n", 1)[0]
        source = "\n".join(line[10:] for line in source.splitlines())
        names = ["Desktop (aarch64-apple-darwin)", "Desktop (x86_64-apple-darwin)",
                 "Desktop (x86_64-pc-windows-msvc)", "Desktop (x86_64-unknown-linux-gnu)", "android"]
        jobs = [{"id": index, "name": name, "conclusion": "success"} for index, name in enumerate(names)]
        ci_names = ["Format", "Clippy", "Check", "Android Release Contract", "Desktop Signing Contract", "Release Gate"]
        ci_jobs = [{"id": index, "name": name, "conclusion": "success"} for index, name in enumerate(ci_names)]
        smoke = [{"name": name, "conclusion": "success"} for name in (
            "Check first-input shutdown and Task input in isolation",
            "Check MCP application and sync completion in isolation")]
        ci_jobs.append({"id": 6, "name": "Test", "conclusion": "success", "steps": smoke})
        run = {"id": 10, "repository": {"full_name": "owner/Rua"}, "path": ".github/workflows/build-test-apps.yml",
               "head_sha": "matched-commit", "status": "completed", "conclusion": "success"}
        ci_run = {**copy.deepcopy(run), "id": 20, "path": ".github/workflows/release.yml",
                  "event": "push", "head_branch": "main", "html_url": "https://github.com/owner/Rua/actions/runs/20"}
        suites = ("smoke", "workspace", "server-contracts", "server-product-hooks", "server-endpoints",
                  "server-infra", "server-terminal", "server-sync", "docs")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "Cargo.toml").write_text((ROOT / "Cargo.toml").read_text())
            version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
            env = {**os.environ, "RELEASE_TAG": "v" + version, "RELEASE_COMMIT": "matched-commit",
                   "GITHUB_REPOSITORY": "owner/Rua", "CURRENT_RUN_ID": "99"}
            for shape in ("legacy", "matrix", "current_tag", "rerun"):
                for fault in (None, "suite_running", "suite_failed", "gate_failed", "source", "ci_source",
                              "installer", "smoke", "mcp_smoke", "incomplete", "ci_repository", "ci_workflow",
                              "ci_branch", "ci_event", "build_running", "missing_group"):
                    if shape != "matrix" and fault == "missing_group":
                        continue
                    with self.subTest(shape=shape, fault=fault):
                        candidate, checks = copy.deepcopy(run), copy.deepcopy(jobs)
                        verified, ci_checks = copy.deepcopy(ci_run), copy.deepcopy(ci_jobs)
                        if shape == "matrix":
                            for index, suite in enumerate(suites, 10):
                                ci_checks.append({"id": index, "name": "Test (" + suite + ")", "conclusion": "success"})
                            ci_checks[6]["steps"] = []
                            ci_checks[7]["steps"] = copy.deepcopy(smoke)
                        if shape == "current_tag":
                            candidate.update(id=99, status="in_progress", conclusion=None)
                        if shape == "rerun":
                            ci_checks.append({"id": -1, "name": "Test", "conclusion": "failure"})
                        if fault == "source": candidate["head_sha"] = "different-commit"
                        if fault == "ci_source": verified["head_sha"] = "different-commit"
                        if fault == "installer": checks[0]["conclusion"] = "failure"
                        if fault == "smoke": ci_checks[7 if shape == "matrix" else 6]["steps"][0]["conclusion"] = "failure"
                        if fault == "mcp_smoke": ci_checks[7 if shape == "matrix" else 6]["steps"][1]["conclusion"] = "failure"
                        if fault == "suite_failed": ci_checks[6]["conclusion"] = "failure"
                        if fault == "gate_failed": ci_checks[5]["conclusion"] = "failure"
                        if fault == "suite_running": verified.update(status="in_progress", conclusion=None)
                        if fault == "incomplete": checks[0]["conclusion"] = None
                        if fault == "ci_repository": verified["repository"]["full_name"] = "elsewhere/Rua"
                        if fault == "ci_workflow": verified["path"] = ".github/workflows/other.yml"
                        if fault == "ci_branch": verified["head_branch"] = "feature"
                        if fault == "ci_event": verified["event"] = "pull_request"
                        if fault == "build_running": candidate.update(id=11, status="in_progress", conclusion=None)
                        if fault == "missing_group": ci_checks.pop()
                        for name, data in (("build-run", candidate), ("build-jobs", [{"jobs": checks}]),
                                           ("ci-run", verified), ("ci-jobs", [{"jobs": ci_checks}])):
                            (root / (name + ".json")).write_text(json.dumps(data))
                        result = subprocess.run([sys.executable, "-c", source], cwd=root, env=env,
                                                capture_output=True, text=True, timeout=5)
                        self.assertEqual(result.returncode == 0, fault is None, result.stdout + result.stderr)

    def test_preview_cleanup_requires_complete_replacements_for_drafts_and_published_previews(self) -> None:
        import contextlib
        from unittest.mock import patch
        workflow = (ROOT / ".github/workflows/build-test-apps.yml").read_text()
        step = workflow.split("      - name: Remove superseded", 1)[1]
        source = step.split("          python3 - <<'PYTHON'\n", 1)[1].split("          PYTHON\n", 1)[0]
        source = "\n".join(line[10:] for line in source.splitlines())
        names = ["arm.dmg", "intel.dmg", "windows.exe", "linux.deb", "android.apk",
                 "Rua_0.8.0_dependency-sources.tar.gz", "SHA256SUMS"]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            assets = root / "release-assets"
            assets.mkdir()
            for name in names:
                (assets / name).touch()
            for case in ("draft", "published", "clean_published", "incomplete_draft",
                         "incomplete_published", "production", "missing", "duplicate"):
                with self.subTest(case=case):
                    attachments = [{"id": index, "name": name} for index, name in enumerate(names)]
                    if case.startswith("incomplete_"): attachments.pop()
                    if case != "clean_published":
                        attachments.append({"id": 99, "name": "old-notices.tar.gz"})
                    release = {"tag_name": "v0.8.0", "draft": case in ("draft", "incomplete_draft"),
                               "prerelease": case != "production", "assets": attachments}
                    releases = [] if case == "missing" else [release] * (2 if case == "duplicate" else 1)
                    (root / "preview-releases.json").write_text(json.dumps([releases]))
                    with contextlib.chdir(root), patch.dict(os.environ, {
                        "RELEASE_TAG": "v0.8.0", "GITHUB_REPOSITORY": "owner/Rua",
                    }), patch("subprocess.run") as delete:
                        if case in ("draft", "published", "clean_published"):
                            exec(source, {})
                            if case == "clean_published":
                                delete.assert_not_called()
                            else:
                                delete.assert_called_once_with([
                                    "gh", "api", "--method", "DELETE", "repos/owner/Rua/releases/assets/99",
                                ], check=True)
                        else:
                            with self.assertRaises(AssertionError):
                                exec(source, {})
                            delete.assert_not_called()


    def test_console_typecheck_never_falls_back_to_global_typescript(self) -> None:
        manifest = json.loads((ROOT / "console/package.json").read_text())
        lock = json.loads((ROOT / "console/package-lock.json").read_text())
        self.assertEqual(manifest["devDependencies"]["typescript"], "5.8.3")
        self.assertEqual(lock["packages"][""]["devDependencies"]["typescript"], "5.8.3")
        self.assertEqual(lock["packages"]["node_modules/typescript"]["version"], "5.8.3")
        with tempfile.TemporaryDirectory(prefix="rua compiler ") as temp:
            fixture = Path(temp)
            (fixture / "package.json").write_text(json.dumps(manifest))
            commands = fixture / "bin"
            commands.mkdir()
            compiler = commands / "tsc"
            compiler.write_text("#!/bin/sh\ntouch global-compiler-called\n")
            compiler.chmod(0o755)
            result = subprocess.run(
                ["npm", "run", "typecheck"], cwd=fixture,
                env={**os.environ, "PATH": f"{commands}{os.pathsep}{os.environ['PATH']}"},
                capture_output=True, text=True, timeout=15,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((fixture / "global-compiler-called").exists())
            self.assertIn("typescript/bin/tsc", result.stderr)

    def test_protoc_installs_verified_official_assets_through_the_api(self) -> None:
        import base64
        import io
        import textwrap
        import zipfile

        action = (ROOT / ".github/actions/setup-protoc/action.yml").read_text()
        script = textwrap.dedent(action.split("        script: |\n", 1)[1])
        for host, arch, platform, failure in (
            ("Linux", "X64", "linux-x86_64", ""),
            ("macOS", "X64", "osx-x86_64", ""),
            ("macOS", "ARM64", "osx-aarch_64", ""),
            ("Windows", "X64", "win64", ""),
            ("Linux", "X64", "linux-x86_64", "missing"),
            ("Linux", "X64", "linux-x86_64", "version"),
        ):
            with self.subTest(host=host, arch=arch, failure=failure):
                archive = io.BytesIO()
                executable = "protoc.exe" if host == "Windows" else "protoc"
                with zipfile.ZipFile(archive, "w") as fixture:
                    fixture.writestr(f"bin/{executable}", "fixture compiler")
                    fixture.writestr("include/google/protobuf/empty.proto", "fixture include")
                config = {
                    "script": script, "failure": failure, "executable": executable,
                    "asset": f"protoc-23.4-{platform}.zip",
                    "archive": base64.b64encode(archive.getvalue()).decode(),
                }
                runner = r"""
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const config = JSON.parse(process.env.PROTOC_FIXTURE);
const archive = Buffer.from(config.archive, 'base64');
const exported = {};
const paths = [];
const github = {
  rest: { repos: { getReleaseByTag: async request => {
    assert.deepEqual(request, {owner:'protocolbuffers', repo:'protobuf', tag:'v23.4'});
    return {data:{assets: config.failure === 'missing' ? [] : [{id:42, name:config.asset}]}};
  }}},
  request: async (route, request) => {
    assert.equal(route, 'GET /repos/{owner}/{repo}/releases/assets/{asset_id}');
    assert.deepEqual(request, {owner:'protocolbuffers', repo:'protobuf', asset_id:42,
      headers:{accept:'application/octet-stream'}});
    return {data:archive.buffer.slice(archive.byteOffset, archive.byteOffset + archive.byteLength)};
  },
};
const core = {
  addPath: value => paths.push(value),
  exportVariable: (key, value) => { exported[key] = value; },
};
const exec = {
  exec: async (command, args) => { execFileSync(command, args); },
  getExecOutput: async (command, args) => {
    assert.equal(path.basename(command), config.executable);
    assert.deepEqual(args, ['--version']);
    assert.equal(fs.readFileSync(command, 'utf8'), 'fixture compiler');
    return {stdout:config.failure === 'version' ? 'libprotoc 99.0' : 'libprotoc 23.4\n'};
  },
};
(async () => {
  const run = new (Object.getPrototypeOf(async function(){}).constructor)('github','core','exec','require',config.script);
  if (config.failure) {
    await assert.rejects(run(github, core, exec, require),
      config.failure === 'missing' ? /asset is missing/ : /Unexpected protoc version/);
    assert.deepEqual(exported, {});
    assert.deepEqual(paths, []);
    return;
  }
  await run(github, core, exec, require);
  assert.deepEqual(paths, [path.dirname(exported.PROTOC)]);
  assert.equal(fs.readFileSync(path.join(exported.PROTOC_INCLUDE, 'google/protobuf/empty.proto'), 'utf8'), 'fixture include');
})().catch(error => { console.error(error); process.exitCode = 1; });
"""
                with tempfile.TemporaryDirectory(prefix="rua protoc ") as temp:
                    result = subprocess.run(
                        ["node", "-e", runner], capture_output=True, text=True, timeout=20,
                        env={**os.environ, "RUNNER_OS": host, "RUNNER_ARCH": arch,
                             "RUNNER_TEMP": temp, "PROTOC_FIXTURE": json.dumps(config)},
                    )
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_versions_sidecar_and_console_resources_are_atomic(self) -> None:
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
        package = json.loads((ROOT / "console" / "package.json").read_text())
        config = json.loads((TAURI / "tauri.conf.json").read_text())

        version = workspace["workspace"]["package"]["version"]
        self.assertEqual(package["version"], version)
        self.assertEqual(config["version"], version)
        self.assertEqual(config["bundle"]["externalBin"], ["binaries/rua", "binaries/agent-browser", "binaries/cua-driver"])
        self.assertEqual(config["bundle"]["resources"]["../dist"], "console")
        self.assertTrue(config["bundle"]["createUpdaterArtifacts"])

    def test_platform_bundles_use_the_supported_release_contract(self) -> None:
        macos = json.loads((TAURI / "tauri.macos.conf.json").read_text())
        windows = json.loads((TAURI / "tauri.windows.conf.json").read_text())

        self.assertEqual(set(macos["bundle"]["targets"]), {"app", "dmg"})
        self.assertEqual(
            macos["bundle"]["macOS"]["entitlements"], "Entitlements.plist"
        )
        self.assertEqual(windows["bundle"]["targets"], ["nsis"])
        self.assertEqual(
            windows["bundle"]["windows"]["webviewInstallMode"],
            {"type": "downloadBootstrapper", "silent": True},
        )
        self.assertEqual(
            windows["bundle"]["windows"]["nsis"]["installMode"], "currentUser"
        )
        self.assertEqual(
            windows["bundle"]["windows"]["nsis"]["installerHooks"],
            "nsis-hooks.nsh",
        )
        self.assertEqual(
            windows["bundle"]["resources"][
                "../../scripts/release/windows-path.ps1"
            ],
            "windows-path.ps1",
        )

        prepare = (ROOT / "scripts" / "ci" / "prepare-desktop-bundle.sh").read_text()
        self.assertIn('PYTHON="${PYTHON:-python3}"', prepare)
        self.assertIn("PYTHON=python", prepare)
        for target in (
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-pc-windows-msvc",
        ):
            self.assertIn(target, prepare)
        self.assertIn("x86_64-unknown-linux-gnu", prepare)

        build = (ROOT / "scripts" / "ci" / "build-desktop.sh").read_text()
        self.assertIn("CI=true npx tauri build --ci", build)
        self.assertIn("! -name 'rw.*'", build)
        self.assertIn('ARTIFACT_COUNT" -ne "$EXPECTED_ARTIFACT_COUNT"', build)

    def test_windows_path_hooks_remove_only_the_installer_owned_entry(self) -> None:
        hooks = (TAURI / "nsis-hooks.nsh").read_text()
        path_script = (
            ROOT / "scripts" / "release" / "windows-path.ps1"
        ).read_text()
        acceptance = (
            ROOT / "scripts" / "tests" / "verify-windows-desktop-installer.ps1"
        ).read_text()

        self.assertIn("-Action Add", hooks)
        self.assertIn("-Action Remove", hooks)
        self.assertIn('$markerName = "PathEntryOwned"', path_script)
        self.assertIn(
            "$owned.Equals($normalizedInstallDir", path_script
        )
        self.assertIn("Uninstall left the Rua-owned PATH entry behind", acceptance)
        self.assertIn("Uninstall deleted product data", acceptance)
        signing = (ROOT / "scripts" / "release" / "sign-windows.ps1").read_text()
        self.assertIn("& $signToolPath sign", signing)
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text()
        self.assertIn("desktop-failure.png", workflow)

    def test_updater_stops_runtime_before_install_and_recovers_on_failure(self) -> None:
        update = (TAURI / "src" / "update.rs").read_text()
        stop = update.index("runtime::stop_owned(app)")
        install = update.index("prepared.install()")
        restart = update.index("app.request_restart()")

        self.assertLess(stop, install)
        self.assertLess(install, restart)
        self.assertIn("runtime::start_owned(app).await", update[install:restart])
        self.assertIn(".try_lock()", update)
        self.assertIn("MacUpdate::prepare", update)
        self.assertIn("WindowsUpdate::prepare", update)

    def test_manifest_contains_exact_signed_platform_archives(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            artifacts = Path(temp)
            archives = {
                "aarch64-apple-darwin": ".app.tar.gz",
                "x86_64-apple-darwin": ".app.tar.gz",
                "x86_64-pc-windows-msvc": ".nsis.zip",
            }
            for target, suffix in archives.items():
                archive = artifacts / f"rua-{target}-Rua{suffix}"
                archive.write_bytes(b"archive")
                Path(f"{archive}.sig").write_text(f"signature-{target}\n")

            output = artifacts / "latest.json"
            subprocess.run(
                [
                    "python3",
                    str(
                        ROOT
                        / "scripts"
                        / "release"
                        / "generate-tauri-update-manifest.py"
                    ),
                    "--artifacts",
                    str(artifacts),
                    "--repository",
                    "agentscope-ai/Rua",
                    "--tag",
                    "v0.1.0+desktop.1",
                    "--output",
                    str(output),
                ],
                check=True,
            )
            manifest = json.loads(output.read_text())

        self.assertEqual(manifest["version"], "0.1.0+desktop.1")
        self.assertEqual(
            set(manifest["platforms"]),
            {"darwin-aarch64", "darwin-x86_64", "windows-x86_64"},
        )
        for platform in manifest["platforms"].values():
            self.assertTrue(platform["signature"].startswith("signature-"))
            self.assertIn("/releases/download/v0.1.0%2Bdesktop.1/", platform["url"])


if __name__ == "__main__":
    unittest.main()
