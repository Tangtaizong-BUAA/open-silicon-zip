import SwiftUI
import AppKit
import FastZipKit

struct ArchiveInput: Identifiable, Equatable {
    var id: URL { url }
    let url: URL
    var size: UInt64?
    var isDirectory: Bool
}

enum SplitChoice: Hashable {
    case none
    case preset(UInt64)
    case custom
}

@MainActor
final class NewArchiveModel: ObservableObject {
    @Published var inputs: [ArchiveInput] = []
    @Published var name = ""
    @Published var destination: URL
    @Published var settings = CreateSettings()
    @Published var splitChoice: SplitChoice = .none
    @Published var customSplit = ""
    @Published var password = ""
    @Published var confirm = ""
    @Published var showMore = false
    @Published var selectedPreset: UUID?
    @Published var presetName = ""
    @Published var presetMessage: String?
    @Published var rememberPasswordInPreset = false
    private var nameEdited = false

    private static let recentDestKey = "FastZip.recentDestinations"

    init(seed: NewArchiveSeed, prefs: Preferences, presets: [Preset]) {
        destination = FileManager.default.urls(for: .desktopDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory())
        settings.format = prefs.compression.format
        settings.level = prefs.compression.level
        settings.filenameRule = "${NAME}"
        if let name = seed.presetName, let p = presets.first(where: { $0.name == name }) {
            apply(p)
        } else if let p = presets.first(where: { $0.isDefault }) {
            apply(p)
        }
        add(seed.paths.map { URL(fileURLWithPath: $0) })
    }

    // MARK: Inputs (FR-101)

    func add(_ urls: [URL]) {
        var fresh: [ArchiveInput] = []
        for u in urls where !inputs.contains(where: { $0.url == u }) {
            let isDir = (try? u.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) ?? false
            fresh.append(ArchiveInput(url: u, size: nil, isDirectory: isDir))
        }
        guard !fresh.isEmpty else { return }
        inputs += fresh
        if !nameEdited { name = suggestedName }
        if let first = inputs.first, destinationIsDefault { destination = first.url.deletingLastPathComponent() }
        for item in fresh {
            Task {
                let size = await Task.detached { NewArchiveModel.measure(item.url) }.value
                if let i = self.inputs.firstIndex(where: { $0.url == item.url }) { self.inputs[i].size = size }
            }
        }
    }

    private var destinationIsDefault: Bool { inputs.count == 1 || !nameEdited }

    func remove(_ ids: Set<URL>) {
        inputs.removeAll { ids.contains($0.url) }
        if !nameEdited { name = suggestedName }
    }

    var suggestedName: String {
        guard let first = inputs.first else { return "" }
        return inputs.count == 1 ? first.url.lastPathComponent : first.url.deletingLastPathComponent().lastPathComponent
    }

    func nameWasEdited() { nameEdited = true }

    nonisolated static func measure(_ url: URL) -> UInt64 {
        let fm = FileManager.default
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: url.path, isDirectory: &isDir) else { return 0 }
        if !isDir.boolValue {
            if let n = (try? fm.attributesOfItem(atPath: url.path))?[.size] as? NSNumber { return n.uint64Value }
            return 0
        }
        var total: UInt64 = 0
        let keys: [URLResourceKey] = [.fileSizeKey, .isRegularFileKey]
        if let en = fm.enumerator(at: url, includingPropertiesForKeys: keys) {
            for case let f as URL in en {
                let v = try? f.resourceValues(forKeys: Set(keys))
                if v?.isRegularFile == true { total += UInt64(v?.fileSize ?? 0) }
            }
        }
        return total
    }

    var totalSize: UInt64 { inputs.reduce(0) { $0 + ($1.size ?? 0) } }
    var sizesPending: Bool { inputs.contains { $0.size == nil } }

    // MARK: Split (FR-104)

    var splitBytes: UInt64? {
        switch splitChoice {
        case .none: return nil
        case .preset(let b): return b
        case .custom: return SplitSize.parse(customSplit)
        }
    }

    // MARK: Validation (05 section 10)

    var outputFileName: String {
        FilenameRule.ensureExtension(FilenameRule.expand(settings.filenameRule, name: name), settings.format.fileExtension)
    }

    var passwordStrength: PasswordStrength { PasswordStrength.evaluate(password) }

    /// Blocking problems; Start stays disabled while non-empty.
    var errors: [String] {
        var out: [String] = []
        if inputs.isEmpty { out.append("Add at least one file or folder.") }
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty || trimmed.contains("/") || trimmed.contains(":") { out.append("Enter a valid archive name.") }
        if case .custom = splitChoice {
            if let b = SplitSize.parse(customSplit) {
                if let e = SplitSize.validate(b) { out.append(e.message) }
            } else {
                out.append("Enter a volume size such as 700 MB.")
            }
        }
        if let b = splitBytes, let limit = settings.format.maxVolumeBytes, b > limit {
            out.append(L10n.f("%@ volumes cannot exceed 4 GB. Pick a smaller size or use 7Z.", settings.format.displayName))
        }
        if settings.format.supportsPassword && password != confirm { out.append("Passwords do not match.") }
        return out
    }

    /// Non-blocking advice.
    var warnings: [String] {
        var out: [String] = []
        if !password.isEmpty && !settings.format.supportsPassword {
            out.append(L10n.f("%@ cannot be encrypted. The password will be ignored.", settings.format.displayName))
        }
        if settings.format == .zip, password.isEmpty == false, passwordStrength == .weak {
            out.append(PasswordStrength.weak.hint + ".")
        }
        if settings.options.deleteSources { out.append("Source items move to the Trash after a successful run.") }
        return out
    }

    var canStart: Bool { errors.isEmpty }

    // MARK: Presets (FR-1101)

    func apply(_ p: Preset) {
        settings = p.settings
        selectedPreset = p.id
        if let b = p.settings.splitBytes {
            if let match = SplitSize.presets.first(where: { $0.bytes == b }) { splitChoice = .preset(match.bytes) }
            else { splitChoice = .custom; customSplit = String(b / 1024) + " KB" }
        } else {
            splitChoice = .none
        }
    }

    func currentSettings() -> CreateSettings {
        var s = settings
        s.splitBytes = splitBytes
        return s
    }

    // MARK: Recent destinations (FR-111)

    var recentDestinations: [URL] {
        (UserDefaults.standard.stringArray(forKey: Self.recentDestKey) ?? []).map { URL(fileURLWithPath: $0) }
    }

    func rememberDestination() {
        var list = (UserDefaults.standard.stringArray(forKey: Self.recentDestKey) ?? []).filter { $0 != destination.path }
        list.insert(destination.path, at: 0)
        UserDefaults.standard.set(Array(list.prefix(8)), forKey: Self.recentDestKey)
    }
}

/// New Archive (FR-101..112, 05 section 10). Usable as a sheet or as its own window.
struct NewArchiveView: View {
    @EnvironmentObject private var app: AppModel
    @Environment(\.dismiss) private var dismiss
    @StateObject private var model: NewArchiveModel
    @State private var selection = Set<URL>()
    @State private var targeted = false
    private let presentedAsWindow: Bool

    init(seed: NewArchiveSeed, presentedAsWindow: Bool = false) {
        self.presentedAsWindow = presentedAsWindow
        // Preferences/presets are applied on first appear because @EnvironmentObject is unavailable in init.
        _model = StateObject(wrappedValue: NewArchiveModel(seed: seed, prefs: AppModel.shared.prefs, presets: AppModel.shared.presets))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    Text("New Archive").font(.title2.weight(.semibold))
                    inputsSection
                    nameSection
                    formatSection
                    passwordSection
                    optionsSection
                    DisclosureGroup("More options", isExpanded: $model.showMore) {
                        moreOptions.padding(.top, 10)
                    }
                    .animation(Theme.ease, value: model.showMore)
                }
                .padding(Theme.pad)
            }
            Divider()
            footer
        }
        .frame(width: 560, height: 700)
    }

    // MARK: Sections

    private var inputsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Items").font(.headline)
                Spacer()
                Text(model.sizesPending ? "Calculating..." : Format.bytes(model.totalSize))
                    .font(.caption).foregroundStyle(.secondary)
                    .accessibilityLabel(L10n.f("Total size %@", Format.bytes(model.totalSize)))
            }
            ZStack {
                if model.inputs.isEmpty {
                    VStack(spacing: 6) {
                        Image(systemName: "plus.circle").font(.system(size: 24, weight: .light))
                        Text("Drop files here or click Add").font(.callout)
                    }
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 110)
                } else {
                    List(model.inputs, selection: $selection) { item in
                        HStack {
                            Image(nsImage: NSWorkspace.shared.icon(forFile: item.url.path))
                                .resizable().frame(width: 18, height: 18)
                            VStack(alignment: .leading, spacing: 0) {
                                Text(item.url.lastPathComponent).lineLimit(1)
                                Text(item.url.deletingLastPathComponent().displayPath)
                                    .font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                            }
                            Spacer()
                            Text(item.size.map { Format.bytes($0) } ?? "...")
                                .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                        }
                        .accessibilityElement(children: .combine)
                    }
                    .listStyle(.plain)
                    .frame(height: 140)
                    .onDeleteCommand { model.remove(selection); selection = [] }
                }
            }
            .card(highlighted: targeted)
            .dropDestination(for: URL.self) { urls, _ in
                model.add(urls)
                return !urls.isEmpty
            } isTargeted: { targeted = $0 }
            HStack {
                Button("Add...") {
                    Task { model.add(await FilePanels.chooseFiles(message: L10n.t("Choose files and folders to compress"))) }
                }
                Button("Remove") { model.remove(selection); selection = [] }
                    .disabled(selection.isEmpty)
            }
            .controlSize(.small)
        }
    }

    private var nameSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Save as").font(.headline)
            HStack {
                TextField("Archive name", text: Binding(
                    get: { model.name },
                    set: { model.name = $0; model.nameWasEdited() }))
                    .textFieldStyle(.roundedBorder)
                    .accessibilityLabel("Archive name")
                Text("." + model.settings.format.fileExtension).foregroundStyle(.secondary)
            }
            HStack {
                Image(systemName: "folder").foregroundStyle(.secondary)
                Text(model.destination.displayPath).lineLimit(1).truncationMode(.middle)
                Spacer()
                Menu("Recent") {
                    ForEach(model.recentDestinations, id: \.self) { u in
                        Button(u.displayPath) { model.destination = u }
                    }
                }
                .menuStyle(.borderlessButton).fixedSize()
                .disabled(model.recentDestinations.isEmpty)
                Button("Choose...") {
                    Task { if let u = await FilePanels.chooseFolder(prompt: L10n.t("Choose")) { model.destination = u } }
                }
            }
            .accessibilityElement(children: .contain)
        }
    }

    private var formatSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Format").font(.headline)
            HStack {
                Picker("Format", selection: $model.settings.format) {
                    ForEach(ArchiveFormat.allCases) { Text($0.displayName).tag($0) }
                }
                .labelsHidden().frame(width: 110)
                .accessibilityLabel("Archive format")
                Picker("Level", selection: $model.settings.level) {
                    ForEach(CompressionLevel.allCases) { Text($0.displayName).tag($0) }
                }
                .pickerStyle(.segmented).labelsHidden()
                .accessibilityLabel("Compression level")
            }
            HStack {
                Text("Split").foregroundStyle(.secondary)
                Picker("Split", selection: $model.splitChoice) {
                    Text("Don't split").tag(SplitChoice.none)
                    ForEach(SplitSize.presets) { Text($0.label).tag(SplitChoice.preset($0.bytes)) }
                    Text("Custom...").tag(SplitChoice.custom)
                }
                .labelsHidden().frame(width: 140)
                .accessibilityLabel("Split into volumes")
                if model.splitChoice == .custom {
                    TextField("e.g. 700 MB", text: $model.customSplit)
                        .textFieldStyle(.roundedBorder).frame(width: 120)
                        .accessibilityLabel("Custom volume size")
                }
            }
        }
    }

    private var passwordSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Password").font(.headline)
            if model.settings.format.supportsPassword {
                RevealableSecureField(title: L10n.t("Password"), text: $model.password)
                RevealableSecureField(title: L10n.t("Re-enter password"), text: $model.confirm)
                HStack(spacing: 3) {
                    ForEach(1...4, id: \.self) { i in
                        Capsule()
                            .fill(i <= model.passwordStrength.rawValue ? strengthColor : Theme.hairline)
                            .frame(height: 4)
                    }
                }
                .accessibilityHidden(true)
                Text(model.passwordStrength.hint).font(.caption).foregroundStyle(.secondary)
                    .accessibilityLabel(L10n.f("Password strength: %@", model.passwordStrength.hint))
            } else {
                Text(L10n.f("%@ does not support encryption.", model.settings.format.displayName))
                    .font(.callout).foregroundStyle(.secondary)
            }
        }
    }

    private var strengthColor: Color {
        switch model.passwordStrength {
        case .empty, .weak: return .red.opacity(0.8)
        case .fair: return .orange
        case .good: return .yellow
        case .strong: return .green
        }
    }

    private var optionsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Options").font(.headline)
            HStack(spacing: 18) {
                Toggle("Test after", isOn: $model.settings.options.testAfter)
                Toggle("Open after", isOn: $model.settings.options.openAfter)
                Toggle("Delete sources", isOn: $model.settings.options.deleteSources)
                Toggle("One archive per item", isOn: $model.settings.options.oneArchivePerItem)
            }
            .toggleStyle(.checkbox)
            .font(.callout)
            HStack {
                Text("If the file exists").foregroundStyle(.secondary).font(.callout)
                Picker("If the file exists", selection: $model.settings.conflict) {
                    Text("Ask").tag(ConflictPolicy.ask)
                    Text("Auto-rename").tag(ConflictPolicy.rename)
                    Text("Overwrite").tag(ConflictPolicy.overwrite)
                }
                .pickerStyle(.segmented).labelsHidden().frame(width: 260)
                .accessibilityLabel("Overwrite policy")
            }
        }
    }

    private var moreOptions: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Threads").foregroundStyle(.secondary)
                Picker("Threads", selection: Binding(
                    get: { model.settings.threads ?? 0 },
                    set: { model.settings.threads = $0 == 0 ? nil : $0 })) {
                    Text("Auto").tag(0)
                    ForEach([1, 2, 4, 8, 16], id: \.self) { Text("\($0)").tag($0) }
                }
                .labelsHidden().frame(width: 90)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text("File name rule").foregroundStyle(.secondary)
                TextField("${NAME}", text: $model.settings.filenameRule)
                    .textFieldStyle(.roundedBorder).font(.system(.body, design: .monospaced))
                    .help(FilenameRule.helpText)
                Text(L10n.f("Result: %@", model.outputFileName)).font(.caption).foregroundStyle(.secondary)
                Text(FilenameRule.helpText).font(.caption2).foregroundStyle(.tertiary)
            }
            Divider()
            VStack(alignment: .leading, spacing: 8) {
                Text("Presets").font(.subheadline.weight(.medium))
                HStack {
                    Picker("Apply preset", selection: Binding(
                        get: { model.selectedPreset },
                        set: { id in
                            model.selectedPreset = id
                            if let id, let p = app.presets.first(where: { $0.id == id }) { model.apply(p) }
                        })) {
                        Text("None").tag(UUID?.none)
                        ForEach(app.presets) { Text($0.name).tag(Optional($0.id)) }
                    }
                    .frame(width: 240)
                    .accessibilityLabel("Apply preset")
                }
                HStack {
                    TextField("Preset name", text: $model.presetName).textFieldStyle(.roundedBorder).frame(width: 200)
                    Button("Save as Preset") { savePreset() }
                        .disabled(model.presetName.trimmingCharacters(in: .whitespaces).isEmpty)
                    if let sel = model.selectedPreset {
                        Button("Delete", role: .destructive) {
                            try? app.deletePreset(sel)
                            model.selectedPreset = nil
                        }
                    }
                }
                Toggle("Remember password with preset (needs unlocked password manager)", isOn: $model.rememberPasswordInPreset)
                    .font(.caption)
                if let m = model.presetMessage { Text(m).font(.caption).foregroundStyle(.secondary) }
            }
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(model.errors + model.warnings, id: \.self) { msg in
                Label(msg, systemImage: model.errors.contains(msg) ? "xmark.circle" : "info.circle")
                    .font(.caption)
                    .foregroundStyle(model.errors.contains(msg) ? Color.red : Color.secondary)
            }
            HStack {
                Spacer()
                Button("Cancel") { close() }.keyboardShortcut(.cancelAction)
                Button("Create") { start() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!model.canStart)
                    .accessibilityHint(model.canStart ? L10n.t("Starts compression") : L10n.t("Fix the listed problems first"))
            }
        }
        .padding(.horizontal, Theme.pad).padding(.vertical, 12)
    }

    // MARK: Actions

    private func close() {
        if presentedAsWindow { NSApp.keyWindow?.close() } else { dismiss() }
    }

    private func start() {
        let request = CreateRequest(inputs: model.inputs.map(\.url), settings: model.currentSettings(),
                                    destination: model.destination,
                                    baseName: model.name.trimmingCharacters(in: .whitespaces),
                                    password: model.password.isEmpty ? nil : model.password)
        model.rememberDestination()
        Task {
            await app.create(request)
        }
        close()
    }

    private func savePreset() {
        let name = model.presetName
        Task {
            var passwordID: UUID?
            if model.rememberPasswordInPreset, !model.password.isEmpty {
                let item = PasswordItem(name: "Preset: \(name)", secret: model.password)
                do {
                    try await app.passwordStore.upsert(item)
                    passwordID = item.id
                } catch {
                    model.presetMessage = L10n.t("Unlock the password manager first to store a password.")
                    return
                }
            }
            do {
                let saved = Preset(id: model.selectedPreset ?? UUID(), name: name, settings: model.currentSettings(),
                                   passwordItemID: passwordID)
                try app.savePreset(saved)
                model.selectedPreset = saved.id
                model.presetMessage = L10n.f("Saved \"%@\".", name)
            } catch {
                model.presetMessage = error.localizedDescription
            }
        }
    }
}
