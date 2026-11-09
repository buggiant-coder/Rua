#!/usr/bin/env python3
"""Update a private Debian candidate from the APK's authenticated recipe.

No rootfs overlay, removal, distro upgrade, or Python/Node ABI transition.
APT verifies repository signatures; artifact versions are lower bounds so an
already newer user-installed package is retained rather than downgraded.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile


def atomic_text(path, text):
    fd, name = tempfile.mkstemp(dir=path.parent)
    try:
        with os.fdopen(fd, "w") as output:
            output.write(text)
            output.flush()
            os.fsync(output.fileno())
        os.replace(name, path)
        parent = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(parent)
        finally:
            os.close(parent)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def at_least(actual, required):
    return actual == required or subprocess.call(["dpkg", "--compare-versions", actual, "ge", required]) == 0


def packages(text):
    result = {}
    for paragraph in text.split("\n\n"):
        fields = dict(line.split(": ", 1) for line in paragraph.splitlines() if ": " in line and not line.startswith(" "))
        if fields.get("Status") == "install ok installed":
            result[fields["Package"]] = fields["Version"]
    return result


def requirements(manifest, archive):
    with tarfile.open(archive) as tar:
        entry = next(member for member in tar if member.name.removeprefix("./") == "var/lib/dpkg/status")
        required = packages(tar.extractfile(entry).read().decode())
    required.update({item["package"]: item["version"] for item in manifest["packages"]})
    return required


def main():
    manifest = json.loads(Path("/run/rua-install/runtime-manifest.json").read_text())
    contract = manifest["compatibility"]
    release = dict(line.split("=", 1) for line in Path("/etc/os-release").read_text().splitlines() if "=" in line)
    assert release["ID"].strip('"') == "debian" and release["VERSION_ID"].strip('"') == contract["debian"], "Debian version is incompatible"
    assert '.'.join(map(str, sys.version_info[:2])) == contract["python"], "Python ABI transition is unsupported"
    assert run("node", "--version").lstrip("v").split(".")[0] == contract["node"], "Node ABI transition is unsupported"
    assert not run("dpkg", "--audit"), "Existing dpkg state is incomplete"
    before = packages(Path("/var/lib/dpkg/status").read_text())
    required = requirements(manifest, "/run/rua-cache/" + manifest["rootfs"]["sha256"])
    changes = []
    for name, version in required.items():
        assert re.fullmatch(r"[a-z0-9][a-z0-9+.-]*", name)
        if name not in before or not at_least(before[name], version):
            changes.append(name + "=" + version)
    artifacts = {item["package"]: item for item in manifest["packages"]}
    local = all(item.split("=", 1)[0] in artifacts for item in changes)
    if changes and local:
        directory = Path("/run/rua-install/packages")
        directory.mkdir()
        paths = []
        for change in changes:
            artifact = artifacts[change.split("=", 1)[0]]
            path = directory / artifact["file"]
            path.symlink_to("/run/rua-cache/" + artifact["sha256"])
            paths.append(str(path))
        # dpkg consumes already authenticated local files. Unlike apt's no-download
        # mode, this does not require another copy in apt's archive cache.
        subprocess.run(["dpkg", "--force-confold", "--unpack", *paths], check=True)
        subprocess.run(["dpkg", "--configure", "--pending"], check=True)
        subprocess.run(["apt-get", "check"], check=True)
    elif changes:
        snapshot = manifest["snapshot"]
        assert re.fullmatch(r"[0-9]{8}T[0-9]{6}Z", snapshot)
        suite = manifest["suite"]
        assert suite == "trixie"
        # Candidate-only sources and lists; retain the user's APT configuration.
        private = Path("/run/rua-install/apt")
        (private / "lists/partial").mkdir(parents=True, exist_ok=True)
        source = private / "sources.list"
        atomic_text(source, "".join(
            f"deb [check-valid-until=no signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://snapshot.debian.org/archive/{archive}/{snapshot}/ {component} main\n"
            for archive, component in (("debian", suite), ("debian", suite + "-updates"), ("debian-security", suite + "-security"))))
        options = ["-o", "Dir::Etc::sourcelist=" + str(source), "-o", "Dir::Etc::sourceparts=-",
                   "-o", "Dir::State::lists=" + str(private / "lists"), "-o", "APT::Sandbox::User=root",
                   "-o", "Acquire::Retries=1", "-o", "Acquire::https::Timeout=20",
                   "-o", "Dpkg::Options::=--force-confold"]
        subprocess.run(["apt-get", *options, "update"], check=True)
        subprocess.run(["apt-get", *options, "--no-remove", "--no-install-recommends", "-y", "install", *changes], check=True)
    after = packages(Path("/var/lib/dpkg/status").read_text())
    assert before.keys() <= after.keys(), "An existing package was removed"
    for expected in (before, required):
        for name, version in expected.items():
            assert at_least(after[name], version), name + " was downgraded"
    assert not run("dpkg", "--audit"), "Updated dpkg state is incomplete"
    run("python3", "-c", "import ssl,sqlite3,venv; assert __import__('sys').version_info[:2] == (3,13)")
    print("RUA_SYSTEM_UPDATE_VERIFIED", flush=True)


if __name__ == "__main__":
    main()
