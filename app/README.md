# macOS application

See the repository [README](../README.md) for requirements, packaging and Finder setup.

- `Sources/FastZipApp`: SwiftUI interface.
- `Sources/FastZipKit`: process transport, queue, settings, localization and Finder menu model.
- `Sources/FinderSyncExtension`: sandboxed Finder extension.
- `XcodeGen/project.yml`: authoritative Xcode configuration.
- `package-app.sh`: reproducible ARM64 packaging entry point.

SwiftPM builds and tests the libraries and UI; Xcode builds the `.appex` with the correct extension entry point. Generated projects, signing identities, build products and user data are excluded from source control. No developer team is hard-coded.

Finder operations are handed to the containing app through an inbox in the extension container; URL fallback explicitly targets the containing application to avoid legacy URL-handler collisions. Default monitoring covers the user's home directory. External volumes require the corresponding setting and an authorized root.
