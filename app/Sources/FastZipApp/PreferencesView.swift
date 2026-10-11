import SwiftUI
import AppKit
import UniformTypeIdentifiers
import FastZipKit

/// Preferences window with the FR-901 tabs.
struct PreferencesView: View {
    @EnvironmentObject private var app: AppModel

    var body: some View {
        TabView {
            GeneralTab().tabItem { Label("General", systemImage: "gearshape") }
            FileAssociationTab().tabItem { Label("File Association", systemImage: "doc.badge.gearshape") }
            FinderMenuTab().tabItem { Label("Finder Menu", systemImage: "contextualmenu.and.cursorarrow") }
            ExtractionTab().tabItem { Label("Extraction", systemImage: "arrow.down.doc") }
            CompressionTab().tabItem { Label("Compression", systemImage: "archivebox") }
            ViewTab().tabItem { Label("View/Edit", systemImage: "paintbrush") }
            AdvancedTab().tabItem { Label("Advanced", systemImage: "slider.horizontal.3") }
            SandboxTab().tabItem { Label("Sandbox", systemImage: "lock.shield") }
            LanguageTab().tabItem { Label("Language", systemImage: "globe") }
        }
        .frame(width: 600, height: 460)
    }
}

private struct PrefPane<Content: View>: View {
    let content: Content
    init(@ViewBuilder content: () -> Content) { self.content = content() }
    var body: some View {
        Form { content }
            .formStyle(.grouped)
    }
}

// MARK: General

private struct GeneralTab: View {
    @EnvironmentObject private var app: AppModel
    @State private var message: String?
    @State private var confirmReset = false

    var body: some View {
        PrefPane {
            Section("Windows") {
                Toggle("Show start screen on launch", isOn: $app.prefs.general.showDashboardOnLaunch)
                Toggle("Press Esc to close windows", isOn: $app.prefs.general.escToClose)
                Toggle("Quit when the last window closes", isOn: $app.prefs.general.quitWhenLastWindowCloses)
            }
            Section("Jobs") {
                Stepper(L10n.f("Run up to %ld jobs at once", app.prefs.general.maxConcurrentJobs),
                        value: $app.prefs.general.maxConcurrentJobs, in: 1...8)
                Toggle("Notify when jobs finish", isOn: $app.prefs.general.notifyOnCompletion)
                Toggle("Open destination folder after extraction", isOn: $app.prefs.general.openDestinationAfterExtract)
                Toggle("Close the Jobs window after success", isOn: $app.prefs.general.closeProgressOnSuccess)
            }
            Section("Passwords") {
                Picker("Try saved passwords automatically", selection: $app.prefs.general.autoPassword) {
                    ForEach(AutoPasswordMode.allCases) { Text($0.displayName).tag($0) }
                }
            }
            Section("Settings file") {
                HStack {
                    Button("Export...") { export() }
                    Button("Import...") { importFile() }
                    Spacer()
                    Button("Reset All...", role: .destructive) { confirmReset = true }
                }
                if let message { Text(message).font(.caption).foregroundStyle(.secondary) }
            }
        }
        .confirmationDialog("Reset all preferences?", isPresented: $confirmReset) {
            Button("Reset", role: .destructive) { app.resetPreferences() }
        } message: {
            Text("Every setting returns to its default. Saved presets and passwords are not affected.")
        }
    }

    private func export() {
        Task { @MainActor in
            guard let url = await FilePanels.saveFile(suggested: "FastZip-Settings.json") else { return }
            do { try app.exportPreferences(to: url); message = L10n.t("Exported.") }
            catch { message = error.localizedDescription }
        }
    }

    private func importFile() {
        Task { @MainActor in
            guard let url = await FilePanels.chooseFiles(allowFolders: false, message: L10n.t("Choose a settings file")).first else { return }
            do { try app.importPreferences(from: url); message = L10n.t("Imported.") }
            catch { message = error.localizedDescription }
        }
    }
}

// MARK: File association (FR-505, FR-509)

private struct FileAssociationTab: View {
    @EnvironmentObject private var app: AppModel

    var body: some View {
        PrefPane {
            Section("Double-click behavior") {
                Picker("When opening an archive", selection: $app.prefs.fileAssociation.doubleClick) {
                    ForEach(DoubleClickAction.allCases) { Text($0.displayName).tag($0) }
                }
                Text("Hold Command while opening to always show the context menu.")
                    .font(.caption).foregroundStyle(.secondary)
                Picker("Files dropped on the Dock icon", selection: $app.prefs.fileAssociation.dockDropAction) {
                    ForEach([DoubleClickAction.openInApp, .smartExtract, .askDestination]) { Text($0.displayName).tag($0) }
                }
            }
            Section("Make FastZip the default app") {
                Text("In Finder, select an archive, choose File > Get Info, pick FastZip under Open With, then click Change All.")
                    .font(.callout)
                Toggle("Show this guidance on first run", isOn: Binding(
                    get: { !app.prefs.fileAssociation.wizardShown },
                    set: { app.prefs.fileAssociation.wizardShown = !$0 }))
            }
        }
    }
}

// MARK: Finder menu (FR-510)

private struct FinderMenuTab: View {
    @EnvironmentObject private var app: AppModel

    var body: some View {
        PrefPane {
            Section("Menu") {
                Toggle("Group actions in a FastZip submenu", isOn: $app.prefs.finderMenu.cascaded)
                Toggle("Show on archive files", isOn: $app.prefs.finderMenu.showOnArchiveFiles)
                Toggle("Enable on external volumes", isOn: $app.prefs.finderMenu.enableOnExternalVolumes)
                Toggle("Show checksum submenu", isOn: $app.prefs.finderMenu.showChecksumSubmenu)
                Toggle("Hide the FastZip icon", isOn: $app.prefs.finderMenu.hideIcon)
            }
            Section("Custom file name") {
                TextField("Rule", text: $app.prefs.finderMenu.customFilenameRule)
                    .font(.system(.body, design: .monospaced))
                Text(FilenameRule.helpText).font(.caption).foregroundStyle(.secondary)
            }
            Section {
                Text("The extension only acts inside folders listed on the Sandbox tab.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}

// MARK: Extraction

private struct ExtractionTab: View {
    @EnvironmentObject private var app: AppModel

    var body: some View {
        PrefPane {
            Section("Destination") {
                Picker("Default destination", selection: $app.prefs.extraction.destination) {
                    ForEach(ExtractDestinationMode.allCases) { Text($0.displayName).tag($0) }
                }
                if app.prefs.extraction.destination == .custom {
                    HStack {
                        Text(app.prefs.extraction.customFolder ?? "No folder chosen").foregroundStyle(.secondary)
                        Spacer()
                        Button("Choose...") {
                            Task { if let u = await FilePanels.chooseFolder() { app.prefs.extraction.customFolder = u.path } }
                        }
                    }
                }
                Picker("When a file exists", selection: $app.prefs.extraction.defaultConflict) {
                    ForEach(ConflictPolicy.allCases) { Text($0.displayName).tag($0) }
                }
            }
            Section("Smart Extract") {
                Toggle("Also wrap single files in a folder", isOn: $app.prefs.extraction.wrapSingleFiles)
                Toggle("Open the single root folder directly", isOn: $app.prefs.extraction.jumpIntoSingleRoot)
            }
            Section("Behavior") {
                Toggle("Skip __MACOSX and ._ metadata", isOn: $app.prefs.extraction.filterMacMetadata)
                Toggle("Move archive to Trash after extraction", isOn: $app.prefs.extraction.deleteArchiveAfter)
                Toggle("Delete partial output when cancelled", isOn: $app.prefs.extraction.deletePartialOnCancel)
                Toggle("Faster writes (write-behind buffering)", isOn: $app.prefs.extraction.fasterWrites)
                Toggle("Extract .tar.gz and friends in a single step", isOn: $app.prefs.extraction.atomicTarCompound)
                Picker("File name normalization", selection: $app.prefs.extraction.normalization) {
                    Text("NFC (recommended)").tag("NFC")
                    Text("NFD").tag("NFD")
                }
                Stepper(L10n.f("Memory budget: %ld MB", app.prefs.extraction.memoryBudgetMB),
                        value: $app.prefs.extraction.memoryBudgetMB, in: 128...16384, step: 128)
                Toggle("Allow symbolic links outside the destination (expert)", isOn: $app.prefs.extraction.allowExternalSymlinks)
            }
        }
    }
}

// MARK: Compression

private struct CompressionTab: View {
    @EnvironmentObject private var app: AppModel
    @State private var skipText = ""

    var body: some View {
        PrefPane {
            Section("Defaults") {
                Picker("Format", selection: $app.prefs.compression.format) {
                    ForEach(ArchiveFormat.allCases) { Text($0.displayName).tag($0) }
                }
                Picker("Level", selection: $app.prefs.compression.level) {
                    ForEach(CompressionLevel.allCases) { Text($0.displayName).tag($0) }
                }
                Picker("Symbolic links", selection: $app.prefs.compression.linkPolicy) {
                    ForEach(LinkPolicy.allCases) { Text($0.displayName).tag($0) }
                }
                Toggle("Wrap a single folder in a root folder", isOn: $app.prefs.compression.wrapSingleFolder)
                Toggle("Include macOS metadata (.DS_Store, ._ files)", isOn: $app.prefs.compression.includeMacMetadata)
            }
            Section("High-speed mode") {
                Picker("Skip incompressible files", selection: $app.prefs.compression.highSpeedMode) {
                    Text("Off").tag("off")
                    Text("On").tag("on")
                    Text("Aggressive").tag("aggressive")
                }
                DisclosureGroup("Extensions stored without compression") {
                    TextEditor(text: Binding(
                        get: { app.prefs.compression.skipExtensions.joined(separator: " ") },
                        set: { app.prefs.compression.skipExtensions = $0.split(whereSeparator: { $0 == " " || $0 == "\n" }).map(String.init) }))
                        .font(.system(.caption, design: .monospaced))
                        .frame(height: 90)
                    Button("Restore defaults") { app.prefs.compression.skipExtensions = Preferences.defaultSkipList }
                        .controlSize(.small)
                }
            }
            Section("Exclusions") {
                TextField("Glob patterns, comma separated", text: Binding(
                    get: { app.prefs.compression.excludeGlobs.joined(separator: ", ") },
                    set: { app.prefs.compression.excludeGlobs = $0.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty } }))
            }
            Section("Presets") {
                PresetManagerList()
            }
        }
    }
}

private struct PresetManagerList: View {
    @EnvironmentObject private var app: AppModel
    @State private var confirmDelete: Preset?

    var body: some View {
        if app.presets.isEmpty {
            Text("Save presets from the New Archive window.").font(.caption).foregroundStyle(.secondary)
        }
        ForEach(app.presets) { p in
            HStack {
                Image(systemName: p.isDefault ? "star.fill" : "star")
                    .foregroundStyle(p.isDefault ? Color.yellow : Color.secondary)
                    .onTapGesture { setDefault(p) }
                    .accessibilityLabel(p.isDefault ? L10n.t("Default preset") : L10n.t("Make default preset"))
                    .accessibilityAddTraits(.isButton)
                VStack(alignment: .leading) {
                    Text(p.name)
                    Text("\(p.settings.format.displayName), \(p.settings.level.displayName)")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Delete", role: .destructive) { confirmDelete = p }.controlSize(.small)
            }
        }
        .confirmationDialog("Delete preset?", isPresented: Binding(get: { confirmDelete != nil }, set: { if !$0 { confirmDelete = nil } })) {
            Button("Delete", role: .destructive) {
                if let p = confirmDelete { try? app.deletePreset(p.id) }
                confirmDelete = nil
            }
        }
    }

    private func setDefault(_ p: Preset) {
        var copy = p
        copy.isDefault.toggle()
        try? app.savePreset(copy)
    }
}

// MARK: View/Edit (FR-906)

private struct ViewTab: View {
    @EnvironmentObject private var app: AppModel

    var body: some View {
        PrefPane {
            Section("Lists") {
                Picker("Row height", selection: $app.prefs.view.rowHeight) {
                    Text("Compact").tag(RowHeight.low)
                    Text("Normal").tag(RowHeight.normal)
                    Text("Comfortable").tag(RowHeight.high)
                }
                Toggle("Show exact sizes", isOn: $app.prefs.view.exactSizes)
                Toggle("Show path bar", isOn: $app.prefs.view.showPathBar)
                Toggle("Expand the folder tree automatically", isOn: $app.prefs.view.autoExpandTree)
            }
            Section("Color") {
                ColorPicker("Accent (light)", selection: hexBinding(\.accentLight, fallback: .blue), supportsOpacity: false)
                ColorPicker("Accent (dark)", selection: hexBinding(\.accentDark, fallback: .blue), supportsOpacity: false)
                Picker("Text color", selection: $app.prefs.view.textColor) {
                    Text("Automatic").tag("auto")
                    Text("White").tag("white")
                    Text("Black").tag("black")
                }
            }
        }
    }

    private func hexBinding(_ key: WritableKeyPath<Preferences.View, String>, fallback: Color) -> Binding<Color> {
        Binding(
            get: { Color(hex: app.prefs.view[keyPath: key]) ?? fallback },
            set: { new in
                if let ns = NSColor(new).usingColorSpace(.sRGB) {
                    let hex = String(format: "#%02X%02X%02X", Int(ns.redComponent * 255), Int(ns.greenComponent * 255), Int(ns.blueComponent * 255))
                    app.prefs.view[keyPath: key] = hex
                }
            })
    }
}

// MARK: Advanced (FR-905)

private struct AdvancedTab: View {
    @EnvironmentObject private var app: AppModel

    var body: some View {
        PrefPane {
            Section("Engine") {
                HStack {
                    TextField("Engine path (fz)", text: Binding(
                        get: { app.prefs.advanced.enginePath ?? "" },
                        set: { app.prefs.advanced.enginePath = $0.isEmpty ? nil : $0 }))
                    Button("Choose...") {
                        Task { if let u = await FilePanels.chooseFiles(allowFolders: false).first { app.prefs.advanced.enginePath = u.path } }
                    }
                }
                Text("Takes effect after restarting FastZip.").font(.caption).foregroundStyle(.secondary)
            }
            Section("Files") {
                Toggle("Move deleted files to the Trash (otherwise delete permanently)", isOn: $app.prefs.advanced.useTrash)
                Stepper(L10n.f("Remember %ld recent files", app.prefs.advanced.recentFilesLimit),
                        value: $app.prefs.advanced.recentFilesLimit, in: 0...100, step: 5)
                TextField("Extra archive extensions", text: Binding(
                    get: { app.prefs.advanced.extraArchiveExtensions.joined(separator: " ") },
                    set: { app.prefs.advanced.extraArchiveExtensions = $0.split(separator: " ").map(String.init) }))
                HStack {
                    Text(app.prefs.advanced.tempFolder ?? "System temporary folder").foregroundStyle(.secondary)
                    Spacer()
                    Button("Choose...") {
                        Task { if let u = await FilePanels.chooseFolder() { app.prefs.advanced.tempFolder = u.path } }
                    }
                    if app.prefs.advanced.tempFolder != nil {
                        Button("Reset") { app.prefs.advanced.tempFolder = nil }
                    }
                }
            }
            Section("Names") {
                Picker("Default code page", selection: $app.prefs.advanced.defaultCodePage) {
                    ForEach(CodePageCatalog.menuItems) { Text($0.menuTitle).tag($0.id) }
                }
                Toggle("Detect UTF-8 names in ZIPs without the UTF-8 flag", isOn: $app.prefs.advanced.unixZipUTF8Detection)
            }
            Section("Diagnostics and power") {
                Toggle("Detailed logging", isOn: $app.prefs.advanced.detailedLogging)
                Toggle("Show precise time in progress", isOn: $app.prefs.advanced.preciseProgressTime)
                Picker("App Nap", selection: $app.prefs.advanced.appNapPolicy) {
                    Text("Allow").tag("allow")
                    Text("Prevent during jobs").tag("prevent")
                }
            }
        }
    }
}

// MARK: Sandbox (FR-511)

private struct SandboxTab: View {
    @EnvironmentObject private var app: AppModel
    @State private var selection: String?

    var body: some View {
        PrefPane {
            Section("Folders the Finder extension may act on") {
                if app.prefs.sandbox.accessibleFolders.isEmpty {
                    Text("None yet. Add your home folder to enable the Finder menu everywhere in it.")
                        .font(.callout).foregroundStyle(.secondary)
                }
                ForEach(app.prefs.sandbox.accessibleFolders, id: \.self) { path in
                    HStack {
                        Image(systemName: "folder")
                        Text((path as NSString).abbreviatingWithTildeInPath).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Button {
                            app.prefs.sandbox.accessibleFolders.removeAll { $0 == path }
                        } label: { Image(systemName: "minus.circle") }
                            .buttonStyle(.borderless)
                            .accessibilityLabel(L10n.f("Remove %@", path))
                    }
                }
                HStack {
                    Button("Add Home") { add(NSHomeDirectory()) }
                    Button("Add External Volumes") { add("/Volumes") }
                    Button("Add Folder...") {
                        Task { if let u = await FilePanels.chooseFolder(prompt: L10n.t("Allow")) { add(u.path) } }
                    }
                }
            }
            Section("Enable the extension") {
                Text("Open System Settings > Privacy & Security > Extensions > Added Extensions and turn on FastZip. On macOS 15 and later extensions are not enabled automatically.")
                    .font(.callout)
                Button("Open Extension Settings") { openExtensionSettings() }
            }
        }
    }

    private func add(_ path: String) {
        if !app.prefs.sandbox.accessibleFolders.contains(path) { app.prefs.sandbox.accessibleFolders.append(path) }
    }

    private func openExtensionSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.ExtensionsPreferences") {
            NSWorkspace.shared.open(url)
        }
    }
}

// MARK: Language

private struct LanguageTab: View {
    @EnvironmentObject private var app: AppModel

    private static let languages: [(code: String, name: String)] = [
        ("zh-Hans", "简体中文"), ("en", "English"), ("zh-Hant", "繁體中文"),
    ]

    var body: some View {
        PrefPane {
            Section("Interface language") {
                Picker("Language", selection: $app.prefs.language.code) {
                    ForEach(Self.languages, id: \.code) { Text($0.name).tag($0.code) }
                }
                Text("Restart the app to apply the interface language.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}
