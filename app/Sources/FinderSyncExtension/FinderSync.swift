import Cocoa
import FinderSync
import OSLog
import FastZipKit

/// Finder Sync extension (FR-501..504, FR-510, FR-511).
///
/// Build this file into an Xcode "Finder Sync Extension" target (see app/README.md); SwiftPM cannot
/// produce the .appex. The extension is sandboxed: it reads the accessible-folders list that the
/// app publishes into the shared app-group container (`FinderConfig`) and never touches archive data
/// itself. Every action either opens a `fastzip://` URL or drops a request into the app-group inbox.
final class FinderSync: FIFinderSync {

    private var config = FinderConfig.read()
    private var actionLinks: [Int: DeepLink] = [:]
    private var nextTag = 1

    override init() {
        super.init()
        applyConfig()
    }

    private func applyConfig() {
        config = FinderConfig.read()
        L10n.configure(language: config.languageCode ?? "zh-Hans")
        // Sandboxed without a provisioning profile: the config may be unreadable
        // and the list empty. Watching the whole home keeps the menu available
        // (05 section 8: the default first-run recommendation is the home folder).
        if config.accessibleFolders.isEmpty { config.accessibleFolders = ["/Users/\(NSUserName())"] }
        var folders = config.accessibleFolders
        if config.enableOnExternalVolumes { folders.append("/Volumes") }
        FIFinderSyncController.default().directoryURLs =
            Set(folders.map { URL(fileURLWithPath: $0) })
    }

    // MARK: Toolbar item

    override var toolbarItemName: String { "FastZip" }
    override var toolbarItemToolTip: String { L10n.t("Compress or extract the selected items") }
    override var toolbarItemImage: NSImage {
        NSImage(systemSymbolName: "archivebox", accessibilityDescription: "FastZip") ?? NSImage()
    }

    // MARK: Menus

    override func menu(for menuKind: FIMenuKind) -> NSMenu? {
        // Re-read the shared config on each invocation so Preferences changes apply immediately.
        applyConfig()

        let controller = FIFinderSyncController.default()
        var selection: [URL] = controller.selectedItemURLs() ?? []
        switch menuKind {
        case .contextualMenuForItems, .toolbarItemMenu:
            break
        case .contextualMenuForContainer:
            if selection.isEmpty, let target = controller.targetedURL() { selection = [target] }
        default:
            return nil
        }

        let specs = FinderMenuBuilder.build(selection: selection, config: config)
        guard !specs.isEmpty else { return nil }

        let menu = NSMenu(title: FinderMenuBuilder.rootTitle)
        append(specs, to: menu)
        return menu
    }

    private func append(_ specs: [MenuSpec], to menu: NSMenu) {
        for spec in specs {
            if spec.isSeparator {
                menu.addItem(.separator())
                continue
            }
            let item = NSMenuItem(title: spec.title, action: nil, keyEquivalent: "")
            if !config.hideIcon { item.image = NSImage(systemSymbolName: "archivebox", accessibilityDescription: nil) }
            if !spec.children.isEmpty {
                let sub = NSMenu(title: spec.title)
                append(spec.children, to: sub)
                item.submenu = sub
            } else if let link = spec.link {
                item.action = #selector(openLink(_:))
                item.target = self
                item.tag = nextTag
                actionLinks[nextTag] = link
                nextTag += 1
                if actionLinks.count > 4096 { actionLinks = actionLinks.filter { $0.key > nextTag - 2048 } }
                item.representedObject = link.url.absoluteString
            }
            menu.addItem(item)
        }
    }

    // MARK: Actions

    @objc private func openLink(_ sender: NSMenuItem) {
        // Finder serializes menu items across XPC. representedObject need not survive;
        // the integer tag is the stable action token sent back to the extension.
        guard let link = actionLinks[sender.tag] else { return }
        let url = link.url
        Logger(subsystem: "org.opensiliconzip.mac", category: "Finder").notice("Dispatching Finder action tag \(sender.tag)")

        // Target the containing app explicitly: old FastZip installs may also own fastzip://.
        let containingApp = Bundle.main.bundleURL.deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let options = NSWorkspace.OpenConfiguration()
        options.activates = FinderMenuBuilder.needsUI(link)
        do {
            try JobInbox.post(link)
            NSWorkspace.shared.openApplication(at: containingApp, configuration: options) { _, error in
                if let error { Logger(subsystem: "org.opensiliconzip.mac", category: "Finder").error("Launch failed: \(error.localizedDescription, privacy: .public)") }
            }
        } catch {
            NSWorkspace.shared.open([url], withApplicationAt: containingApp, configuration: options) { _, error in
                if let error { Logger(subsystem: "org.opensiliconzip.mac", category: "Finder").error("URL handoff failed: \(error.localizedDescription, privacy: .public)") }
            }
        }

    }
}
