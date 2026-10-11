import AppKit
import SwiftUI
import Sparkle
import FastZipKit

/// Sparkle verifies every installation with the public Ed25519 key in Info.plist.
/// The private key remains in the maintainer's local signing directory, never on the feed server.
@MainActor
final class UpdateController: NSObject, ObservableObject, SPUUpdaterDelegate, SPUStandardUserDriverDelegate {
    static let shared = UpdateController()
    @Published private(set) var availableVersion: String?
    @Published private(set) var canCheck = false
    @Published private(set) var waitingForJobs = false
    private var started = false
    private var deferredInstall: Task<Void, Never>?
    private lazy var controller = SPUStandardUpdaterController(startingUpdater: false,
                                                               updaterDelegate: self, userDriverDelegate: self)
    var updater: SPUUpdater { controller.updater }
    private var busy: Bool { !AppModel.shared.summary.isIdle || EngineProcesses.isBusy }

    func start() {
        guard !started, Bundle.main.bundleURL.pathExtension == "app" else { return }
        started = true
        updater.publisher(for: \.canCheckForUpdates).assign(to: &$canCheck)
        controller.startUpdater()
    }

    func check() { controller.checkForUpdates(nil) }

    var supportsGentleScheduledUpdateReminders: Bool { true }

    func standardUserDriverShouldHandleShowingScheduledUpdate(_ update: SUAppcastItem, andInImmediateFocus immediateFocus: Bool) -> Bool { false }

    func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool, forUpdate update: SUAppcastItem, state: SPUUserUpdateState) {
        availableVersion = update.displayVersionString
        if !handleShowingUpdate, UserDefaults.standard.string(forKey: "OSZ.lastUpdateNotice") != update.versionString {
            UserDefaults.standard.set(update.versionString, forKey: "OSZ.lastUpdateNotice")
            Notifier.post(title: L10n.t("An update is available"), body: L10n.f("Open Silicon Zip %@ is ready. Open Check for Updates to install it.", update.displayVersionString), identifier: "osz-update")
        }
    }

    func standardUserDriverWillFinishUpdateSession() { availableVersion = nil }

    func updater(_ updater: SPUUpdater, mayPerform updateCheck: SPUUpdateCheck) throws {
        guard !busy else {
            throw NSError(domain: "org.opensiliconzip.updates", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: L10n.t("Updates will wait until all jobs finish.")])
        }
    }

    func updater(_ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem,
                 untilInvokingBlock installHandler: @escaping () -> Void) -> Bool {
        guard busy else { return false }
        waitingForJobs = true
        deferredInstall = Task { @MainActor [weak self] in
            while let self, self.busy {
                try? await Task.sleep(for: .seconds(1))
                if Task.isCancelled { return }
            }
            guard let self, !Task.isCancelled else { return }
            self.waitingForJobs = false
            installHandler()
        }
        return true
    }
}

struct UpdateSettingsView: View {
    @ObservedObject private var updates = UpdateController.shared
    @EnvironmentObject private var app: AppModel
    var body: some View {
        Form {
            Section("Software Updates") {
                Toggle("Check for updates automatically", isOn: Binding(
                    get: { updates.updater.automaticallyChecksForUpdates },
                    set: { updates.updater.automaticallyChecksForUpdates = $0; updates.objectWillChange.send() }))
                Toggle("Download and install updates automatically", isOn: Binding(
                    get: { updates.updater.automaticallyDownloadsUpdates },
                    set: { updates.updater.automaticallyDownloadsUpdates = $0; updates.objectWillChange.send() }))
                Button("Check for Updates…") { updates.check() }
                    .disabled(!updates.canCheck || !app.summary.isIdle)
                Text("New versions are checked every four hours while the app is running. Updates are signed and served by the home server.")
                    .font(.caption).foregroundStyle(.secondary)
                Text("Updates will wait until all jobs finish.").font(.caption).foregroundStyle(.secondary)
                if updates.waitingForJobs { Text("The update is ready and will restart after your jobs finish.") }
            }
        }.formStyle(.grouped)
    }
}

struct CheckForUpdatesButton: View {
    @ObservedObject private var updates = UpdateController.shared
    var body: some View {
        Button(updates.availableVersion.map { L10n.f("Update to %@…", $0) } ?? L10n.t("Check for Updates…")) { updates.check() }.disabled(!updates.canCheck)
    }
}
