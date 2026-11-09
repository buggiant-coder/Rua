#!/usr/bin/env python3
"""Stage only APK-owned native executable and resources; never a Linux userland."""
import argparse
import hashlib
import io
import json
import tarfile
import urllib.request
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import zipfile


def atomic_bytes(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(tmp, path)
        sync_dir(path.parent)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def sync_dir(path):
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def stage_proot(destination):
    """Reuse pinned Android ELF dependencies, not a Termux bootstrap or installer."""
    repo = Path(__file__).resolve().parents[2]
    manifest = json.loads((repo / "mobile/runtime/native-dependencies.json").read_text())
    cache = repo / "target/native-runtime-inputs"
    names = {
        "bin/proot": "libproot.so",
        "libexec/proot/loader": "libproot-loader.so",
        "lib/libtalloc.so.2.5.0": "libtalloc.so",
        "lib/libandroid-shmem.so": "libandroid-shmem.so",
    }
    for package in manifest["packages"]:
        archive = cache / Path(package["Filename"]).name
        if not archive.exists():
            with urllib.request.urlopen(manifest["repository"] + package["Filename"], timeout=60) as response:
                atomic_bytes(archive, response.read())
        data = archive.read_bytes()
        if len(data) != int(package["Size"]) or hashlib.sha256(data).hexdigest() != package["SHA256"]:
            raise ValueError(f"Native dependency checksum mismatch: {archive}")
        if not data.startswith(b"!<arch>\n"):
            raise ValueError("Invalid Debian archive")
        offset = 8
        while offset < len(data):
            header = data[offset:offset + 60]
            size = int(header[48:58])
            member = header[:16].decode().strip().rstrip("/")
            body = data[offset + 60:offset + 60 + size]
            offset += 60 + size + size % 2
            if not member.startswith("data.tar"):
                continue
            with tarfile.open(fileobj=io.BytesIO(body)) as tar:
                for entry in tar:
                    relative = entry.name.removeprefix("./data/data/com.termux/files/usr/")
                    if relative not in names:
                        continue
                    if not entry.isfile():
                        raise ValueError("Native dependency must be a regular ELF file")
                    target = destination / names[relative]
                    atomic_bytes(target, tar.extractfile(entry).read())
                    os.chmod(target, 0o755)
    # Android's APK native library extraction accepts lib*.so names only.
    # Change the dependency SONAME explicitly; never create a prefix compatibility tree.
    for name, arguments in [
        ("libproot.so", ["--replace-needed", "libtalloc.so.2", "libtalloc.so"]),
        ("libtalloc.so", ["--set-soname", "libtalloc.so"]),
    ]:
        target = destination / name
        fd, temporary = tempfile.mkstemp(dir=destination)
        os.close(fd)
        try:
            shutil.copyfile(target, temporary)
            subprocess.run(["patchelf", *arguments, temporary], check=True)
            atomic_bytes(target, Path(temporary).read_bytes())
            os.chmod(target, 0o755)
        finally:
            os.unlink(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--console", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--strip", type=Path, required=True)
    parser.add_argument("--test-probe", action="store_true")
    args = parser.parse_args()
    if not args.binary.is_file() or not (args.console / "index.html").is_file():
        parser.error("Build Android rua and console/dist before packaging")
    native = args.out / "jniLibs/arm64-v8a" / ("librua-runtime-probe.so" if args.test_probe else "librua.so")
    native.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=native.parent)
    os.close(fd)
    try:
        shutil.copyfile(args.binary, tmp)
        subprocess.run([str(args.strip), "--strip-debug", tmp], check=True)
        os.chmod(tmp, 0o755)
        with open(tmp, "rb") as source:
            os.fsync(source.fileno())
        os.replace(tmp, native)
        sync_dir(native.parent)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)
    if args.test_probe:
        return
    stage_proot(native.parent)
    compiler = args.strip.parent / "aarch64-linux-android21-clang"
    for name in ("process-group", "runtime-gate"):
        source = Path(__file__).resolve().parents[2] / "mobile/runtime" / (name + ".c")
        with tempfile.TemporaryDirectory(dir=native.parent) as build:
            launcher = Path(build) / name
            subprocess.run([str(compiler), "-std=c11", "-D_GNU_SOURCE", "-O2", "-fPIE", "-pie",
                            "-Wall", "-Wextra", "-Werror", "-Wl,-z,max-page-size=16384",
                            str(source), "-o", str(launcher)], check=True)
            target = native.parent / ("librua-" + name + ".so")
            atomic_bytes(target, launcher.read_bytes())
            os.chmod(target, 0o755)
    assets = args.out / "assets"
    # Generated resources must exactly match this build, including after source removal.
    if assets.exists():
        shutil.rmtree(assets)
    assets.mkdir(parents=True, exist_ok=True)
    atomic_bytes(assets / "runtime-manifest.json", (Path(__file__).resolve().parents[2] / "mobile/runtime/runtime-manifest.json").read_bytes())
    repo = Path(__file__).resolve().parents[2]
    atomic_bytes(assets / "native-dependencies.json", (repo / "mobile/runtime/native-dependencies.json").read_bytes())
    for notice in (repo / "mobile/runtime/licenses").iterdir():
        if notice.is_file():
            atomic_bytes(assets / "licenses" / notice.name, notice.read_bytes())
    atomic_bytes(assets / "update-runtime.py", (repo / "mobile/runtime/update-runtime.py").read_bytes())
    bridge = subprocess.run([
        str(repo / "console/node_modules/esbuild/bin/esbuild"),
        str(repo / "console/scripts/xterm-headless-bridge.mjs"),
        "--bundle", "--platform=node", "--format=esm", "--target=node24",
    ], check=True, stdout=subprocess.PIPE).stdout
    fd, tmp = tempfile.mkstemp(dir=assets)
    try:
        with os.fdopen(fd, "w+b") as output:
            with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                entry = zipfile.ZipInfo("xterm-headless-bridge.mjs", date_time=(1980, 1, 1, 0, 0, 0))
                entry.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(entry, bridge)
                for file in sorted(args.console.rglob("*")):
                    if not file.is_file():
                        continue
                    if file.is_symlink():
                        raise ValueError(f"Symlink not allowed in APK resources: {file}")
                    entry = zipfile.ZipInfo("console/" + file.relative_to(args.console).as_posix(),
                                            date_time=(1980, 1, 1, 0, 0, 0))
                    entry.compress_type = zipfile.ZIP_DEFLATED
                    archive.writestr(entry, file.read_bytes())
            output.flush()
            os.fsync(output.fileno())
        os.replace(tmp, assets / "rua-ui.zip")
        sync_dir(assets)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)
    digest = hashlib.sha256((assets / "rua-ui.zip").read_bytes()).hexdigest()
    atomic_bytes(assets / "rua-ui.sha256", (digest + "\n").encode())
    print(f"Native payload staged: {native.stat().st_size} executable bytes; UI {digest}")


if __name__ == "__main__":
    main()
