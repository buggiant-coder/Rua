# Release signing configuration

Production private keys belong in a protected release environment or maintainer credential vault. Public source contains only public keys, secret variable names, and explicitly untrusted test identities. Never export production keys into source archives or logs.

Desktop release inputs and validation are defined by `scripts/release/check-desktop-signing-config.sh`. The protected `desktop-release` environment supplies Apple Developer ID and notarization inputs, Windows Authenticode inputs, and the separate Tauri updater key. Configure all names required by that script and run its `--require-secrets` preflight in the protected environment. Pull requests run non-secret checks only.

Android debug builds use the committed `testkey_untrusted.jks`, a public development identity. Release builds read `~/.rua-release/android-signing.properties` (`storeFile`, `keyAlias`, `storePassword`, `keyPassword`). The environment variables `RUA_ANDROID_KEYSTORE` (private keystore path), `RUA_ANDROID_KEY_ALIAS`, `RUA_ANDROID_STORE_PASSWORD`, and `RUA_ANDROID_KEY_PASSWORD` override the corresponding local values for CI or an explicit build. Missing credentials fail before release build tasks execute; release signing never falls back to the test key. Keep the keystore outside the repository. CI should supply these values from protected secrets and stage the keystore in its temporary directory.

For local maintainer builds, credentials are stored outside the repository under `~/.rua-release/`: `android-release.jks`, `android-signing.properties`, encrypted `tauri-updater.key`, and `signing.env`. The directory must have mode 700 and private files mode 600. Normal Android packaging and physical-device installation automatically read the local properties file:

```bash
scripts/package-mobile-apk.sh --release
# Or build and install to connected physical devices:
scripts/install-mobile-prod.sh
```

The local macOS launcher automatically selects the unique valid `local-dev` identity from Keychain. `RUA_DESKTOP_SIGNING_IDENTITY` can explicitly select another certificate by its SHA-1 fingerprint. This does not load private Android or updater credentials into desktop builds. Source `~/.rua-release/signing.env` only when a trusted updater-signing build needs those inputs.

The environment file contains passwords; keep it private and never upload it or the key directory as a release asset. Back up keys and recovery credentials in an encrypted maintainer vault. A copy on this Mac alone is not a recovery backup.

The initial public release uses a new Android signer and updater public key. Existing test APKs keep working, but a same-package APK signed with the new key cannot ordinarily overwrite them. Export user data before uninstalling a test APK; uninstalling can delete app data. The package name remains `com.rua.mobile`. Subsequent releases must preserve the production signer and increase `versionCode`.

macOS continues to use its existing local development certificate. An updater signature does not provide Apple Developer ID trust or notarization. GitHub distribution without those can trigger Gatekeeper warnings. Existing desktop test installations need a manual installation of a build containing the new updater public key. Production releases run manually from a version tag through `release.yml` and still require Apple and Windows production credentials. Tag pushes run `build-test-apps.yml`, which publishes explicitly marked GitHub prereleases using ad-hoc macOS signing, unsigned Windows installers, and the public Android development identity. Preview releases do not publish `latest.json` or automatic-update signatures. Preparing local keys does not configure CI credentials.

Updater keys are independent of platform certificates. Existing installations trust the public updater key embedded in their application. Replacing that public key requires a planned transition signed by the currently trusted key. Back up all production keys and recovery credentials outside the repository.

Signing does not replace per-platform installation acceptance, dependency notices or matching source delivery. See [licensing requirements](open-source-licensing.md).
