#!/usr/bin/env python3
"""Coherent component staging and rollback; no external network or compiler needed."""
from contextlib import contextmanager
import hashlib
from http.client import IncompleteRead
from http.server import BaseHTTPRequestHandler, HTTPServer
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
from threading import Thread
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("stage_automation", ROOT / "scripts/ci/stage-desktop-automation.py")
staging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(staging)


def pe_fixture(text):
    import struct
    data = bytearray(512)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3c, 64)
    data[64:68] = b"PE\0\0"
    data[88:90] = b"\x0b\x02"
    data[400:400+len(text)] = text
    return bytes(data)


@contextmanager
def archive_server(body, truncated_requests=0):
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            sent = body[:len(body) // 2] if len(requests) <= truncated_requests else body
            self.wfile.write(sent)
            self.wfile.flush()

        def log_message(self, *args):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    thread = Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/archive", requests
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


class ArchiveDownloadTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.enterContext(patch.object(staging, "ROOT", self.root))
        self.enterContext(patch("time.sleep", return_value=None))

    def cache_path(self, expected):
        return self.root / "target/desktop-drivers/archives" / expected

    def test_truncated_response_retries_complete_archive(self):
        body = b"pinned native driver fixture" * 32
        expected = staging.digest(body)
        with archive_server(body, truncated_requests=1) as (url, requests):
            data = staging.archive_bytes(None, url, expected)
        self.assertEqual(data, body)
        self.assertEqual(len(requests), 2)
        self.assertEqual(self.cache_path(expected).read_bytes(), body)

    def test_exhausted_download_does_not_cache_partial_and_can_recover(self):
        body = b"pinned driver after network recovers" * 32
        expected = staging.digest(body)
        with archive_server(body, truncated_requests=3) as (url, requests):
            with self.assertRaisesRegex(RuntimeError, "failed after 3 attempts") as failure:
                staging.archive_bytes(None, url, expected)
            self.assertIsInstance(failure.exception.__cause__, IncompleteRead)
            self.assertEqual(len(requests), 3)
            self.assertFalse(self.cache_path(expected).exists())
            self.assertEqual(staging.archive_bytes(None, url, expected), body)
            self.assertEqual(len(requests), 4)
        self.assertEqual(self.cache_path(expected).read_bytes(), body)

    def test_complete_response_still_requires_pinned_checksum(self):
        expected = staging.digest(b"pinned driver")
        with archive_server(b"wrong driver") as (url, requests):
            with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                staging.archive_bytes(None, url, expected)
        self.assertEqual(len(requests), 1)
        self.assertFalse(self.cache_path(expected).exists())

    def test_verified_cache_reused_without_network(self):
        body = b"cached pinned driver"
        expected = staging.digest(body)
        with archive_server(body) as (url, requests):
            self.assertEqual(staging.archive_bytes(None, url, expected), body)
            with patch.object(staging, "urlopen") as download:
                self.assertEqual(staging.archive_bytes(None, url, expected), body)
                download.assert_not_called()
        self.assertEqual(len(requests), 1)

    def test_corrupt_cache_downloaded_again(self):
        body = b"valid pinned driver"
        expected = staging.digest(body)
        staging.atomic_write(self.cache_path(expected), b"corrupt cache")
        with archive_server(body) as (url, requests):
            self.assertEqual(staging.archive_bytes(None, url, expected), body)
        self.assertEqual(len(requests), 1)
        self.assertEqual(self.cache_path(expected).read_bytes(), body)

    def test_explicit_archive_stays_offline_and_requires_pinned_checksum(self):
        archive = self.root / "driver.tar.gz"
        data = b"offline driver fixture"
        staging.atomic_write(archive, data)
        with patch.object(staging, "urlopen") as download:
            self.assertEqual(staging.archive_bytes(archive, "unused", staging.digest(data)), data)
            with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                staging.archive_bytes(archive, "unused", staging.digest(b"other driver"))
            download.assert_not_called()
        self.assertFalse(self.cache_path(staging.digest(data)).exists())


class StagingTests(unittest.TestCase):
    def fixture(self, root):
        source = root / "cua.zip"
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as package:
            package.writestr("cua-driver.exe", pe_fixture(b"Cua native fixture"))
            package.writestr("cua_driver_sdk.dll", b"not shipped")
        staging.atomic_write(source, archive.getvalue())
        manifest = json.loads(staging.MANIFEST.read_text())
        driver = manifest["cua_driver"]
        driver["license_sha256"] = staging.digest(b"MIT fixture")
        artifact = driver["artifacts"]["x86_64-pc-windows-msvc"]
        artifact["archive_sha256"] = staging.digest(archive.getvalue())
        artifact["sha256"] = staging.digest(pe_fixture(b"Cua native fixture"))
        staging.atomic_write(root / "manifest.json", json.dumps(manifest).encode())
        for name in ("cua-driver-LICENSE", "agent-browser-LICENSE", "NOTICE.md", "agent-browser-rua.patch"):
            staging.atomic_write(root / name, b"MIT fixture")
        runtime = root / "rua.exe"
        staging.atomic_write(runtime, pe_fixture(b"Rua native fixture"))
        return source, root / "manifest.json", runtime

    def stage(self, root):
        source, manifest, runtime = self.fixture(root)
        receipt = {"version": "0.38.1", "sha256": staging.digest(pe_fixture(b"Browser fixture"))}
        with patch.object(staging, "build_browser", return_value=(pe_fixture(b"Browser fixture"), receipt)):
            staging.stage("x86_64-pc-windows-msvc", root / "out", cua_archive=source, manifest_path=manifest, runtime=runtime, product_version="1.0.0")
        return source, manifest, runtime

    def test_stages_complete_set_and_preserves_other_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            staging.atomic_write(root / "out/existing.txt", b"keep")
            self.stage(root)
            self.assertEqual((root / "out/existing.txt").read_bytes(), b"keep")
            receipt=json.loads((root / "out/automation/release-set.json").read_text())
            self.assertEqual(set(receipt["components"]), {"agent-browser", "cua-driver", "rua"})
            self.assertEqual(receipt["product_version"], "1.0.0")
            for name, data in (("rua", pe_fixture(b"Rua native fixture")), ("cua-driver", pe_fixture(b"Cua native fixture")), ("agent-browser", pe_fixture(b"Browser fixture"))):
                self.assertEqual((root / f"out/{name}-x86_64-pc-windows-msvc.exe").read_bytes(), data)
            self.assertFalse((root / "out/cua_driver_sdk.dll").exists())

    def test_verified_existing_cua_reused_while_runtime_is_updated(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _, manifest, runtime = self.stage(root)
            updated_runtime = pe_fixture(b"Updated Rua runtime")
            staging.atomic_write(runtime, updated_runtime)
            browser = pe_fixture(b"Browser fixture")
            receipt = {"version": "0.38.1", "sha256": staging.digest(browser)}
            with patch.object(staging, "archive_bytes", side_effect=AssertionError("verified existing Cua must not download")) as download:
                with patch.object(staging, "build_browser", return_value=(browser, receipt)):
                    result = staging.stage(
                        "x86_64-pc-windows-msvc", root / "out", manifest_path=manifest,
                        runtime=runtime, product_version="1.0.1",
                    )
                download.assert_not_called()
            self.assertEqual((root / "out/cua-driver-x86_64-pc-windows-msvc.exe").read_bytes(), pe_fixture(b"Cua native fixture"))
            self.assertEqual((root / "out/rua-x86_64-pc-windows-msvc.exe").read_bytes(), updated_runtime)
            self.assertEqual(result["components"]["rua"]["sha256"], staging.digest(updated_runtime))
            self.assertEqual(result["product_version"], "1.0.1")

    def test_corrupt_existing_cua_downloaded_again_before_promotion(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, manifest, runtime = self.stage(root)
            destination = root / "out/cua-driver-x86_64-pc-windows-msvc.exe"
            staging.atomic_write(destination, b"corrupt previously staged driver")
            browser = pe_fixture(b"Browser fixture")
            receipt = {"version": "0.38.1", "sha256": staging.digest(browser)}
            with patch.object(staging, "archive_bytes", return_value=source.read_bytes()) as download:
                with patch.object(staging, "build_browser", return_value=(browser, receipt)):
                    result = staging.stage(
                        "x86_64-pc-windows-msvc", root / "out", manifest_path=manifest,
                        runtime=runtime, product_version="1.0.1",
                    )
                artifact = json.loads(manifest.read_text())["cua_driver"]["artifacts"]["x86_64-pc-windows-msvc"]
                download.assert_called_once_with(None, artifact["archive_url"], artifact["archive_sha256"])
            self.assertEqual(destination.read_bytes(), pe_fixture(b"Cua native fixture"))
            self.assertEqual(result["components"]["cua-driver"]["sha256"], staging.digest(destination.read_bytes()))

    def test_corrupt_second_driver_does_not_replace_any_component(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            source, manifest, runtime=self.stage(root)
            before={p.relative_to(root / "out"): p.read_bytes() for p in (root / "out").rglob("*") if p.is_file()}
            staging.atomic_write(source,b"corrupt")
            with patch.object(staging,"build_browser") as build:
                with self.assertRaisesRegex(ValueError,"SHA-256 mismatch"):
                    staging.stage("x86_64-pc-windows-msvc", root / "out", cua_archive=source, manifest_path=manifest, runtime=runtime)
                build.assert_not_called()
            after={p.relative_to(root / "out"): p.read_bytes() for p in (root / "out").rglob("*") if p.is_file()}
            self.assertEqual(before, after)

    def test_failed_promotion_restores_entire_previous_set(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            self.stage(root)
            before=(root / "out/automation/release-set.json").read_bytes()
            replace=staging.os.replace
            def fail_once(source, dest):
                if Path(source).name=="binaries" and Path(dest)==(root / "out").resolve():
                    raise OSError("injected rename failure")
                return replace(source,dest)
            with patch.object(staging.os,"replace",side_effect=fail_once):
                with self.assertRaisesRegex(OSError,"injected"):
                    staging.promote_set(root / "out",{"rua-x86_64-pc-windows-msvc.exe":(b"bad upgrade",0o755)})
            self.assertEqual((root / "out/automation/release-set.json").read_bytes(),before)
            self.assertEqual((root / "out/rua-x86_64-pc-windows-msvc.exe").read_bytes(),pe_fixture(b"Rua native fixture"))

    def test_interrupted_rename_recovers_before_next_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            staging.atomic_write(root / "out.previous/old",b"previous")
            staging.promote_set(root / "out",{"new":(b"current",0o644)})
            self.assertEqual((root / "out/old").read_bytes(),b"previous")
            self.assertFalse((root / "out.previous").exists())

    def test_authenticode_only_changes_do_not_change_pe_image_identity(self):
        import struct
        unsigned=pe_fixture(b"machine code")
        signed=bytearray(unsigned)
        struct.pack_into("<I",signed,152,123)
        struct.pack_into("<II",signed,232,512,16)
        signed.extend(bytes([11])*16)
        self.assertEqual(staging.pe_image_digest(unsigned),staging.pe_image_digest(signed))
        signed[400] ^= 1
        self.assertNotEqual(staging.pe_image_digest(unsigned),staging.pe_image_digest(signed))
        with self.assertRaises(ValueError):
            staging.pe_image_digest(signed[:-1])

    def test_unsupported_mobile_target_rejected_before_download(self):
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(staging,"archive_bytes") as download:
                with self.assertRaisesRegex(ValueError,"unsupported Desktop target"):
                    staging.stage("aarch64-linux-android",Path(temporary))
                download.assert_not_called()

    def test_upstream_source_links_and_traversal_rejected(self):
        for path, kind in (("source/../../escape",tarfile.REGTYPE),("source/link",tarfile.SYMTYPE)):
            with self.subTest(path=path), tempfile.TemporaryDirectory() as temporary:
                archive=io.BytesIO()
                with tarfile.open(fileobj=archive,mode="w:gz") as package:
                    entry=tarfile.TarInfo(path);entry.type=kind;entry.linkname="/tmp"
                    package.addfile(entry)
                with self.assertRaises(ValueError):
                    staging.extract_source(archive.getvalue(),Path(temporary))


if __name__=="__main__":
    unittest.main()
