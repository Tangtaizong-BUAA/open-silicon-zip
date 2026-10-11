import SwiftUI
import AppKit
import FastZipKit

@main
struct FastZipApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var model = AppModel.shared

    var body: some Scene {
        // Start screen / dashboard (FR-508).
        WindowGroup("Open Silicon Zip", id: "dashboard") {
            DashboardView()
                .modifier(AppChrome())
                .environmentObject(model)
        }
        .handlesExternalEvents(matching: Set<String>())
        .commands { AppCommands(model: model) }

        // One window per archive (FR-001, FR-017).
        WindowGroup("Archive", id: "archive", for: URL.self) { binding in
            if let url = binding.wrappedValue {
                ArchiveBrowserView(url: url)
                    .modifier(AppChrome())
                    .environmentObject(model)
            }
        }
        .handlesExternalEvents(matching: Set<String>())
        .defaultSize(width: 1000, height: 680)

        WindowGroup("New Archive", id: "new", for: NewArchiveSeed.self) { binding in
            NewArchiveView(seed: binding.wrappedValue ?? NewArchiveSeed(), presentedAsWindow: true)
                .modifier(AppChrome())
                .environmentObject(model)
        }
        .handlesExternalEvents(matching: Set<String>())
        .windowResizability(.contentSize)

        Window("Jobs", id: "jobs") {
            JobsView()
                .modifier(AppChrome())
                .environmentObject(model)
        }
        .defaultSize(width: 560, height: 520)

        Window("Password Manager", id: "passwords") {
            PasswordManagerView()
                .modifier(AppChrome())
                .environmentObject(model)
        }
        .defaultSize(width: 520, height: 480)

        Window("Tools", id: "tools") {
            ToolsView()
                .modifier(AppChrome())
                .environmentObject(model)
        }
        .defaultSize(width: 640, height: 560)

        Settings {
            PreferencesView()
                .modifier(AppChrome())
                .environmentObject(model)
        }
    }
}

/// Registers window openers with the model, applies the accent colour and the ESC-to-close setting.
struct AppChrome: ViewModifier {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        content
            .tint(Theme.accent(model.prefs, dark: scheme == .dark))
            .onAppear {
                model.openArchiveWindow = { openWindow(id: "archive", value: $0) }
                model.openNewArchiveWindow = { openWindow(id: "new", value: $0) }
                model.openJobsWindow = { openWindow(id: "jobs") }
                model.windowsReady()
            }
            .onExitCommand {
                if model.prefs.general.escToClose { NSApp.keyWindow?.performClose(nil) }
            }
    }
}

struct AppCommands: Commands {
    @ObservedObject var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Archive...") { openWindow(id: "new", value: NewArchiveSeed()) }
                .keyboardShortcut("n")
            Button("Open Archive...") {
                Task { @MainActor in
                    for u in await FilePanels.chooseFiles(allowFolders: false, message: L10n.t("Choose archives to open")) {
                        model.openArchive(u)
                    }
                }
            }
            .keyboardShortcut("o")
            Menu("Open Recent") {
                ForEach(model.recents, id: \.self) { url in
                    Button(url.lastPathComponent) { model.openArchive(url) }
                }
                Divider()
                Button("Clear Menu") { model.clearRecents() }
                    .disabled(model.recents.isEmpty)
            }
        }
        CommandMenu("Tools") {
            Button("Jobs") { openWindow(id: "jobs") }.keyboardShortcut("j")
            Button("Test, Repair, Checksum...") { openWindow(id: "tools") }.keyboardShortcut("t", modifiers: [.command, .shift])
            Button("Password Manager") { openWindow(id: "passwords") }.keyboardShortcut("p", modifiers: [.command, .shift])
        }
    }
}
