#!/usr/bin/env python3
"""Provision an official AOSP WebView on disposable API 26/28 SDK images only.

The SDK image uses AOSP's PUBLIC test certificate. Re-sign the verified upstream
prebuilt for a normal package update; do not modify framework provider policy.
This test dependency is never included in the Rua APK.
"""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
COMMIT = "b44f390d379bc993866c918ecc7697e364632d2e"
BLOB = "a0beb2aaad2169a4a3c9740d8829e2787b4bbbcd"
APK_SHA256 = "a1782aeba80f95921d4663e8ffe8e106ecde4df9e2995ea2841d620bcc390502"
PUBLIC_CERT_SHA256 = "a4384ba815b9499a5ce349b4e33c1755278873fe2eac150a068823f526e6dbde"
PUBLIC_KEY_SHA256 = "495675d32e89a149d5abe191f4e9c0e218b9068714e9b53a7c91e164a0741a23"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", help="Install on a named disposable AVD; omit to prepare CI artifacts")
    parser.add_argument("--adb", default="adb")
    args = parser.parse_args()
    adb = [args.adb, "-s", args.serial] if args.serial else None
    if adb:
        avd = subprocess.check_output(adb + ["emu", "avd", "name"], text=True).splitlines()[0]
        if avd not in ("rua_native_e2e", "rua_native_api26_e2e", "rua_native_api28_e2e"):
            raise SystemExit("Only owned acceptance AVDs are allowed")
        sdk = subprocess.check_output(adb + ["shell", "getprop", "ro.build.version.sdk"], text=True).strip()
        if sdk not in ("26", "28"):
            raise SystemExit("This provider fixture is restricted to API 26/28 AOSP default images")
    sdk_root = Path(os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT") or str(Path.home() / "Library/Android/sdk"))
    signer = sdk_root / "build-tools/36.0.0/apksigner"
    if not signer.is_file(): raise SystemExit("Install Android build-tools;36.0.0")
    spec = importlib.util.spec_from_file_location("stage", ROOT / "scripts/android/stage-native-payload.py")
    stage = importlib.util.module_from_spec(spec); spec.loader.exec_module(stage)
    cache = ROOT / "target/android-system-images"
    cache.mkdir(parents=True, exist_ok=True)
    def fetch(name, url, digest):
        target = cache / name
        if not target.is_file():
            with urllib.request.urlopen(url, timeout=90) as response:
                data = base64.b64decode(response.read(), validate=True)
            if hashlib.sha256(data).hexdigest() != digest: raise RuntimeError("Checksum mismatch: " + name)
            stage.atomic_bytes(target, data)
        if hashlib.sha256(target.read_bytes()).hexdigest() != digest: raise RuntimeError("Checksum mismatch: " + name)
        return target
    upstream = "https://android.googlesource.com/platform/"
    apk = fetch("aosp-webview.apk", upstream + "external/chromium-webview/+/" + COMMIT + "/prebuilt/arm64/webview.apk?format=TEXT", APK_SHA256)
    certificate = fetch("testkey.x509.pem", upstream + "build/+/android-9.0.0_r1/target/product/security/testkey.x509.pem?format=TEXT", PUBLIC_CERT_SHA256)
    key = fetch("testkey.pk8", upstream + "build/+/android-9.0.0_r1/target/product/security/testkey.pk8?format=TEXT", PUBLIC_KEY_SHA256)
    signed = cache / "aosp-webview-sdk-signed.apk"
    with tempfile.TemporaryDirectory(dir=cache) as temporary:
        candidate = Path(temporary) / "webview.apk"
        subprocess.run([str(signer), "sign", "--key", str(key), "--cert", str(certificate), "--out", str(candidate), str(apk)], check=True)
        subprocess.run([str(signer), "verify", str(candidate)], check=True)
        stage.atomic_bytes(signed, candidate.read_bytes())
    report = {"upstream_commit": COMMIT, "upstream_blob": BLOB, "upstream_sha256": APK_SHA256,
              "version": "113.0.5672.136", "sdk_test_certificate": "a40da80a59d170caa950cf15c18c454d47a39b26989d8b640ecd745ba71bf5dc",
              "signed_sha256": hashlib.sha256(signed.read_bytes()).hexdigest()}
    if adb:
        subprocess.run(adb + ["install", "-r", str(signed)], check=True)
        report["device_provider"] = subprocess.check_output(adb + ["shell", "dumpsys", "webviewupdate"], text=True)
    stage.atomic_bytes(cache / "webview-evidence.json", json.dumps(report, indent=2).encode())
    if adb and "(com.android.webview, 113.0.5672.136)" not in report["device_provider"]:
        raise RuntimeError("Expected AOSP provider was not selected; see webview-evidence.json")
    print("Verified AOSP WebView fixture: " + str(signed))


if __name__ == "__main__":
    main()
