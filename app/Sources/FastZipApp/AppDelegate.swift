import AppKit
import FastZipKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    override init() {
        L10n.configure(language: PreferencesStore().load().language.code)
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // FR-506: Services menu entries (see scripts/App-Info.plist).
        NSApp.servicesProvider = ServicesProvider.shared
        NSUpdateDynamicServices()
    }

    /// File opens (double-click, Dock drop) and `fastzip://` URLs all arrive here.
    func application(_ application: NSApplication, open urls: [URL]) {
        Task { @MainActor in
            let model = AppModel.shared
            let files = urls.filter(\.isFileURL)
            let links = urls.filter { !$0.isFileURL }
            if !files.isEmpty { model.route(files: files, action: model.prefs.fileAssociation.doubleClick) }
            links.forEach { model.handle(url: $0) }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        MainActor.assumeIsolated { AppModel.shared.prefs.general.quitWhenLastWindowCloses }
    }

    func applicationWillTerminate(_ notification: Notification) {
        EngineProcesses.shutDown()
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        let busy = MainActor.assumeIsolated { !AppModel.shared.summary.isIdle || EngineProcesses.isBusy }
        guard busy else { return .terminateNow }
        let alert = NSAlert()
        alert.messageText = L10n.t("Jobs are still running")
        alert.informativeText = L10n.t("Quitting now cancels them. Partial output is handled according to your settings.")
        alert.addButton(withTitle: L10n.t("Cancel"))
        alert.addButton(withTitle: L10n.t("Quit Anyway"))
        return alert.runModal() == .alertSecondButtonReturn ? .terminateNow : .terminateCancel
    }
}

/// NSServices entry points. Selector names must match `NSMessage` in the Info.plist snippet.
final class ServicesProvider: NSObject {
    static let shared = ServicesProvider()

    private func paths(_ pboard: NSPasteboard) -> [String] {
        let urls = pboard.readObjects(forClasses: [NSURL.self],
                                      options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        return urls.map(\.path)
    }

    private func dispatch(_ link: DeepLink) {
        Task { @MainActor in AppModel.shared.handle(link) }
    }

    @objc func compressWithFastZip(_ pboard: NSPasteboard, userData: String,
                                   error: AutoreleasingUnsafeMutablePointer<NSString>) {
        dispatch(.new(paths: paths(pboard), preset: nil))
    }

    @objc func compressToZip(_ pboard: NSPasteboard, userData: String,
                             error: AutoreleasingUnsafeMutablePointer<NSString>) {
        dispatch(.compress(paths: paths(pboard), format: .zip, deleteSources: false, each: false, preset: nil, rule: nil))
    }

    @objc func compressTo7z(_ pboard: NSPasteboard, userData: String,
                            error: AutoreleasingUnsafeMutablePointer<NSString>) {
        dispatch(.compress(paths: paths(pboard), format: .sevenZ, deleteSources: false, each: false, preset: nil, rule: nil))
    }

    @objc func extractWithFastZip(_ pboard: NSPasteboard, userData: String,
                                  error: AutoreleasingUnsafeMutablePointer<NSString>) {
        dispatch(.extract(paths: paths(pboard), mode: .smart, deleteArchive: false))
    }
}
