#!/usr/bin/env python3
"""Build and stage one pinned native Desktop component set, before platform signing."""
import argparse
from contextlib import contextmanager
import hashlib
from http.client import IncompleteRead
import io
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.request import urlopen
import zipfile

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "console/src-tauri/automation/manifest.json"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def pe_image_digest(data):
    """Hash the PE image independently of Authenticode's mutable fields."""
    import struct
    def u32(offset):
        if offset < 0 or offset + 4 > len(data):
            raise ValueError("truncated PE header")
        return struct.unpack_from("<I", data, offset)[0]
    if data[:2] != b"MZ":
        raise ValueError("missing PE DOS header")
    header = u32(0x3c)
    if data[header:header + 4] != b"PE\0\0":
        raise ValueError("missing PE header")
    optional = header + 24
    magic = data[optional:optional + 2]
    if magic not in (b"\x0b\x01", b"\x0b\x02"):
        raise ValueError("unsupported PE optional header")
    directory = optional + (128 if magic == b"\x0b\x01" else 144)
    certificate, size = u32(directory), u32(directory + 4)
    if certificate == 0 and size == 0:
        end = len(data)
    elif certificate >= directory + 8 and certificate + size == len(data):
        end = certificate
    else:
        raise ValueError("invalid PE certificate boundary")
    if end < directory + 8:
        raise ValueError("truncated PE image")
    image = bytearray(data[:end])
    image[optional + 64:optional + 68] = bytes(4)
    image[directory:directory + 8] = bytes(8)
    return digest(image.rstrip(b"\0"))


def verify(data, expected, name):
    if digest(data) != expected:
        raise ValueError(f"{name}: SHA-256 mismatch")


def atomic_write(path, data, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(data)
            output.flush()
            if os.name != "nt":
                os.fchmod(output.fileno(), mode)
            os.fsync(output.fileno())
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def sync_directory(path):
    if os.name != "nt":
        parent = os.open(path, os.O_RDONLY)
        try:
            os.fsync(parent)
        finally:
            os.close(parent)


@contextmanager
def exclusive(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+b") as lock:
        if os.name == "nt":
            import msvcrt
            lock.seek(0)
            lock.write(b"0")
            lock.flush()
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_LOCK, 1)
        else:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            yield
        finally:
            if os.name == "nt":
                lock.seek(0)
                msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(lock, fcntl.LOCK_UN)


def archive_bytes(archive, url, expected):
    if archive is not None:
        data = archive.read_bytes()
        verify(data, expected, "driver source/release archive")
        return data

    cache = ROOT / "target/desktop-drivers/archives" / expected
    with exclusive(cache.with_name(cache.name + ".lock")):
        if cache.exists():
            data = cache.read_bytes()
            if digest(data) == expected:
                return data
            cache.unlink()
            sync_directory(cache.parent)

        attempts = 3
        for attempt in range(1, attempts + 1):
            print(f"downloading driver archive ({attempt}/{attempts}): {url}", file=sys.stderr, flush=True)
            try:
                with urlopen(url, timeout=60) as response:
                    data = response.read()
            except (IncompleteRead, URLError, TimeoutError, ConnectionError) as error:
                if isinstance(error, HTTPError) and error.code not in (408, 429, 500, 502, 503, 504):
                    raise
                if attempt == attempts:
                    raise RuntimeError(f"driver archive download failed after {attempts} attempts: {url}") from error
                print(f"driver archive download interrupted: {error}; retrying in {attempt}s", file=sys.stderr, flush=True)
                time.sleep(attempt)
                continue
            # Never cache partial or unverified bytes, and do not retry integrity failures.
            verify(data, expected, "driver source/release archive")
            atomic_write(cache, data)
            return data


def target_artifact(driver, target):
    if target not in driver["artifacts"]:
        raise ValueError(f"unsupported Desktop target: {target}")
    return driver["artifacts"][target]


def extract_source(data, directory):
    """Extract only regular files in the single pinned archive root; reject links."""
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        roots = set()
        for member in archive:
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or not path.parts:
                raise ValueError("unsafe source archive path")
            roots.add(path.parts[0])
            relative = Path(*path.parts[1:])
            if member.isdir():
                continue
            if not member.isfile() or len(roots) != 1:
                raise ValueError("source archive must have one root and only regular files")
            atomic_write(directory / relative, archive.extractfile(member).read(), member.mode & 0o777)


def build_browser(target, archive=None, manifest_path=MANIFEST, profile="ci", cache=None):
    manifest = json.loads(manifest_path.read_text())
    driver = manifest["agent_browser"]
    target_artifact(driver, target)
    patch = manifest_path.parent / driver["adapter"]["patch"]
    verify(patch.read_bytes(), driver["adapter"]["sha256"], "Rua browser adapter")
    verify((manifest_path.parent / "agent-browser-LICENSE").read_bytes(), driver["license_sha256"], "agent-browser license")
    rustc = subprocess.check_output(["rustc", "-vV"], text=True)
    host = next(line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: "))
    if host != target:
        raise ValueError(f"build the pinned driver on its native target ({target}); build host is {host}")
    inputs = {"source": driver["source"], "adapter": driver["adapter"], "target": target, "profile": profile, "rustc": rustc}
    key = digest(json.dumps(inputs, sort_keys=True).encode())
    cache = (cache or ROOT / "target/desktop-drivers") / key
    cache.mkdir(parents=True, exist_ok=True)
    output = cache / ("agent-browser.exe" if target.endswith("windows-msvc") else "agent-browser")
    receipt_path = cache / "build.json"
    with exclusive(cache / "build.lock"):
        if output.exists() and receipt_path.exists():
            receipt = json.loads(receipt_path.read_text())
            if receipt.get("inputs") == inputs and receipt.get("sha256") == digest(output.read_bytes()):
                return output.read_bytes(), receipt
        data = archive_bytes(archive, driver["source"]["archive_url"], driver["source"]["archive_sha256"])
        with tempfile.TemporaryDirectory(prefix="rua-driver-source-") as temporary:
            source = Path(temporary)
            extract_source(data, source)
            verify((source / "LICENSE").read_bytes(), driver["license_sha256"], "upstream browser license")
            verify((source / "cli/Cargo.lock").read_bytes(), driver["source"]["lock_sha256"], "upstream Cargo.lock")
            if json.loads((source / "package.json").read_text())["version"] != driver["version"]:
                raise ValueError("agent-browser source version mismatch")
            env = os.environ.copy()
            env["GIT_CEILING_DIRECTORIES"] = str(source.parent)
            subprocess.run(["git", "apply", "--check", str(patch.resolve())], cwd=source, env=env, check=True)
            subprocess.run(["git", "apply", str(patch.resolve())], cwd=source, env=env, check=True)
            # Keep the native upstream build independent of the Rua workspace.
            for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTDOCFLAGS"):
                env.pop(name, None)
            env["CARGO_TARGET_DIR"] = str(cache / "target")
            subprocess.run(["cargo", "build", "--locked", "--profile", profile, "--target", target, "--manifest-path", "cli/Cargo.toml"], cwd=source, env=env, check=True)
            built = cache / "target" / target / ("debug" if profile == "dev" else profile) / output.name
            contract = subprocess.check_output([str(built), "--rua-automation-contract"], text=True, timeout=5).strip()
            if contract != driver["adapter"]["contract"]:
                raise ValueError("built browser adapter contract mismatch")
            binary = built.read_bytes()
            receipt = {"version": driver["version"], "commit": driver["commit"], "sha256": digest(binary), "inputs": inputs}
            atomic_write(output, binary, 0o755)
            atomic_write(receipt_path, json.dumps(receipt, indent=2).encode() + b"\n")
            return binary, receipt


def read_cua(target, archive=None, manifest_path=MANIFEST, existing_binary=None):
    driver = json.loads(manifest_path.read_text())["cua_driver"]
    artifact = target_artifact(driver, target)
    verify((manifest_path.parent / "cua-driver-LICENSE").read_bytes(), driver["license_sha256"], "Cua Driver license")
    binary = None
    if archive is None and existing_binary is not None:
        destination = existing_binary.parent.resolve()
        # Read under the promotion lock, but never hold it during a network request.
        with exclusive(destination.with_name(destination.name + ".lock")):
            try:
                candidate = existing_binary.read_bytes()
            except FileNotFoundError:
                candidate = b""
        if digest(candidate) == artifact["sha256"]:
            binary = candidate
            print(f"reusing verified Cua Driver {driver['version']}: {existing_binary}", file=sys.stderr, flush=True)
    if binary is None:
        data = archive_bytes(archive, artifact["archive_url"], artifact["archive_sha256"])
        if target.endswith("windows-msvc"):
            with zipfile.ZipFile(io.BytesIO(data)) as package:
                binary = package.read(artifact["member"])
        else:
            with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as package:
                member = package.getmember(artifact["member"])
                if not member.isfile():
                    raise ValueError("Cua Driver archive member must be a regular file")
                binary = package.extractfile(member).read()
    verify(binary, artifact["sha256"], "Cua Driver executable")
    return binary, {"version": driver["version"], "commit": driver["commit"], "sha256": digest(binary), "archive_sha256": artifact["archive_sha256"]}


def promote_set(destination, files):
    """Stage the whole set, then swap; rollback before exposing a partial set."""
    destination = destination.resolve()
    backup = destination.with_name(destination.name + ".previous")
    with exclusive(destination.with_name(destination.name + ".lock")):
        # Recover an interrupted build-time rename before preparing another set.
        if backup.exists():
            if not destination.exists():
                os.replace(backup, destination)
            else:
                shutil.rmtree(backup)
        with tempfile.TemporaryDirectory(prefix=".automation-set-", dir=destination.parent) as temporary:
            candidate = Path(temporary) / "binaries"
            if destination.exists():
                shutil.copytree(destination, candidate)
            else:
                candidate.mkdir()
            for relative, (data, mode) in files.items():
                atomic_write(candidate / relative, data, mode)
            sync_directory(candidate)
            if destination.exists():
                os.replace(destination, backup)
            try:
                os.replace(candidate, destination)
                sync_directory(destination.parent)
            except BaseException:
                if backup.exists():
                    os.replace(backup, destination)
                    sync_directory(destination.parent)
                raise
            if backup.exists():
                shutil.rmtree(backup)


def stage(target, destination, archive=None, cua_archive=None, manifest_path=MANIFEST, profile="ci", runtime=None, product_version=None):
    manifest = json.loads(manifest_path.read_text())
    target_artifact(manifest["agent_browser"], target)
    extension = ".exe" if target.endswith("windows-msvc") else ""
    # Validate both programs before promotion; a bad second archive cannot replace the first.
    cua, cua_receipt = read_cua(target, cua_archive, manifest_path, destination / f"cua-driver-{target}{extension}")
    browser, browser_receipt = build_browser(target, archive, manifest_path, profile)
    if runtime is None:
        raise ValueError("the unified rua executable is required for a coherent Desktop set")
    if not runtime.exists() and extension and runtime.with_suffix(extension).exists():
        runtime = runtime.with_suffix(extension)
    binary = runtime.read_bytes()
    if not binary:
        raise ValueError("empty unified rua executable")
    product_version = product_version or json.loads((ROOT / "console/package.json").read_text())["version"]
    receipt = {"format": 1, "target": target, "product_version": product_version, "manifest_sha256": digest(manifest_path.read_bytes()), "components": {"agent-browser": browser_receipt, "cua-driver": cua_receipt, "rua": {"version": product_version, "sha256": digest(binary)}}}
    if target.endswith("windows-msvc"):
        for name, image in (("agent-browser", browser), ("cua-driver", cua), ("rua", binary)):
            receipt["components"][name]["image_sha256"] = pe_image_digest(image)
    files = {
        f"agent-browser-{target}{extension}": (browser, 0o755),
        f"cua-driver-{target}{extension}": (cua, 0o755),
        f"rua-{target}{extension}": (binary, 0o755),
        "automation/release-set.json": (json.dumps(receipt, indent=2).encode() + b"\n", 0o644),
    }
    for name in ("manifest.json", "agent-browser-LICENSE", "cua-driver-LICENSE", "NOTICE.md", manifest["agent_browser"]["adapter"]["patch"]):
        files[f"automation/{name}"] = ((manifest_path.parent / name).read_bytes(), 0o644)
    promote_set(destination, files)
    print(f"staged {product_version} Desktop set for {target}: agent-browser {browser_receipt['version']} + Cua {cua_receipt['version']}")
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", nargs="?", help="Defaults to the native Rust target")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "console/src-tauri/binaries")
    parser.add_argument("--archive", type=Path, help="Pinned upstream source tarball (never npm or a browser download)")
    parser.add_argument("--cua-archive", type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--profile", choices=("ci", "dev"), default="ci")
    args = parser.parse_args()
    target = args.target
    if target is None:
        target = next(line.removeprefix("host: ") for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    stage(target, args.output_dir, args.archive, args.cua_archive, profile=args.profile, runtime=args.runtime)


if __name__ == "__main__":
    main()
