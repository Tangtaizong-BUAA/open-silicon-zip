# Updates

Open Silicon Zip uses Sparkle 2.10.0. The public update feed is hosted on the project's home server:

**https://home.argonai.cn/open-silicon-zip/appcast.xml**

## Using updates

Choose **Open Silicon Zip → Check for Updates…**, or open **Settings → Updates**. Background checks run every four hours while the app is running. A new release produces a notification and an update entry in the application menu, without opening a large window during Finder operations. Clicking the notification opens the installer. macOS notification permission is required for system banners; the menu remains available when notifications are disabled.

Automatic checks are enabled by default. Automatic downloading and installation are opt-in in Settings. Active compression, extraction and engine work postpone installation/relaunch. Closing the app does not leave a separate update polling daemon running. Notifications are generated after polling the feed; this is not APNs or an instant server push connection.

Both the appcast and the downloaded archive require Ed25519 verification. Archives are verified **before extraction**. HTTPS is required, profiling is disabled, and the private signing key stays in the maintainer's local `~/.config/open-silicon-zip/update-signing.key` file (mode `0600`, enclosing directory `0700`). Only the public key is shipped. An update signature does not replace Apple Developer ID signing or notarization; the preview distribution limits still apply.

## Publishing a release

1. Increase `CFBundleVersion` in `app/scripts/App-Info.plist` and `CURRENT_PROJECT_VERSION` in `app/XcodeGen/project.yml`. Keep the user-facing app version and changelog consistent.
2. Run the tests and `SIGN_IDENTITY=- bash scripts/package-release.sh VERSION` for an ad-hoc preview, or supply an appropriate distribution signing identity for a signed release.
3. Use Sparkle's official `generate_appcast` tool to generate the signed feed from the **final immutable ZIP**:

   ```bash
   python3 scripts/make-appcast.py \
     release-artifacts/vVERSION/OpenSiliconZip-VERSION-macos-arm64.zip \
     --version VERSION --notes-file /path/to/release-summary.txt \
     --appcast-tool /path/to/Sparkle/bin/generate_appcast \
     --output release-artifacts/vVERSION/appcast.xml
   ```

4. Upload the versioned ZIP to the server's `public/releases/` directory. Verify its SHA-256 against the local artifact, then atomically replace `public/appcast.xml` with the signed feed. Never change a ZIP after signing it. Keep previous versioned archives for recovery and GitHub Releases as a separate download source.
5. Fetch the public HTTPS feed and ZIP back, verify their signatures and hashes, and test updating an older installed build. Increment build numbers for subsequent releases; do not silently replace an existing version's assets.

Publishing reads the local key automatically and never requests Keychain authorization. Verification uses only the public key. Keep a secure backup of the private key; losing it can prevent existing clients from trusting future updates. Never commit it or upload it to the serving container.

For a new project only, `swift scripts/update-key.swift create ~/.config/open-silicon-zip/update-signing.key` creates the file and prints only its public key. Put that public key in `SUPublicEDKey` before the first release. Do not regenerate or replace a deployed key as part of routine publishing. This project switched from its initial test Keychain key before its first public release; local test builds were replaced with the matching public key.

## Home-server deployment

The files in [`deploy/updates`](../deploy/updates/) define an isolated nginx container on the existing `demo-site_default` Docker network. It runs as UID 101 with a read-only filesystem, no extra capabilities, 32 MiB memory, and **no published host port**. Only public feed/archive files are mounted. The existing HTTPS gateway forwards `/open-silicon-zip/` to that container through the existing home-server route.

The gateway snippet must be inserted into the existing site's server block, preserving its other routes. Back up the current configuration; if it is bind-mounted, preserve its inode while editing. Run `nginx -t` before a graceful reload. Do not restart or recreate unrelated services. The health endpoint is `/open-silicon-zip/healthz`.

Framework documentation: [Sparkle setup](https://sparkle-project.org/documentation/), [settings and signatures](https://sparkle-project.org/documentation/customization/), [gentle reminders](https://sparkle-project.org/documentation/gentle-reminders/).
