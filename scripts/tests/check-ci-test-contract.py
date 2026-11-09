#!/usr/bin/env python3
"""Exercise server test selection without compiling the Rust workspace."""

from __future__ import annotations

import collections
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
GROUPS = ("contracts", "product-hooks", "terminal", "endpoints", "infra", "sync")


class CiTestContractTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rua ci groups ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for relative in ("scripts/tests/cargo-test-rua-server.sh", "crates/rua-server/Cargo.toml"):
            destination = self.root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / relative, destination)
        commands = self.root / "bin"
        commands.mkdir()
        cargo = commands / "cargo"
        cargo.write_text("#!/usr/bin/env python3\n"
                         "import json, os, sys\n"
                         "with open(os.environ['RUA_CI_FIXTURE_LOG'], 'a') as log:\n"
                         "    log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
                         "sys.exit(1 if os.environ.get('RUA_CI_FIXTURE_FAIL') in sys.argv[1:] else 0)\n")
        cargo.chmod(0o755)
        self.log = self.root / "calls.jsonl"
        self.env = {**os.environ, "PATH": str(commands) + os.pathsep + os.environ["PATH"],
                    "RUA_CI_FIXTURE_LOG": str(self.log)}

    def invoke(self, *arguments, fail=None):
        self.log.unlink(missing_ok=True)
        env = {**self.env}
        if fail:
            env["RUA_CI_FIXTURE_FAIL"] = fail
        result = subprocess.run(["bash", str(self.root / "scripts/tests/cargo-test-rua-server.sh"), *arguments],
                                env=env, text=True, capture_output=True, timeout=10)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return result, calls

    def test_groups_partition_the_complete_server_suite(self):
        result, complete = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        selected = []
        for suite in GROUPS:
            result, calls = self.invoke("--suite", suite)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(calls, suite)
            selected.extend(calls)
        self.assertEqual(selected, complete)
        targets = [call[index + 1] for call in complete
                   for index, argument in enumerate(call) if argument == "--test"]
        declared = tomllib.loads((ROOT / "crates/rua-server/Cargo.toml").read_text())["test"]
        expected = collections.Counter(target["name"] for target in declared)
        expected["contract_sync"] += 1  # One exact isolated test, then the remaining tests.
        self.assertEqual(collections.Counter(targets), expected)
        self.assertEqual(sum("--lib" in call and "--bins" in call for call in complete), 1)

    def test_fixture_isolation_and_offline_mode_are_preserved(self):
        for suite in GROUPS:
            with self.subTest(suite=suite):
                result, calls = self.invoke("--offline", "--suite", suite)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(all(call[:2] == ["test", "--offline"] for call in calls))
                if suite in ("endpoints", "infra"):
                    self.assertEqual(calls[0][-2:], ["--", "--test-threads=1"])
                if suite == "sync":
                    self.assertEqual(len(calls), 2)
                    exact = calls[0][calls[0].index("contract_sync") + 1]
                    self.assertEqual(calls[0][-2:], ["--", "--exact"])
                    self.assertEqual(calls[1][-4:], ["--", "--skip", exact, "--test-threads=1"])

    def test_failure_propagates_without_suppressing_other_default_groups(self):
        result, calls = self.invoke(fail="--lib")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(calls), 7)
        self.assertEqual(calls[-1][-1], "--test-threads=1")
        result, calls = self.invoke("--suite", "endpoints", fail="endpoints")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(calls), 1)

    def test_invalid_group_options_never_start_tests(self):
        for arguments in (("--suite", "unknown"), ("--suite",), ("--bad-option",)):
            with self.subTest(arguments=arguments):
                result, calls = self.invoke(*arguments)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
