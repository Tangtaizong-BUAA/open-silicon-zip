import SwiftUI
import AppKit
import FastZipKit

/// Archive browser window (FR-001..014).
struct ArchiveBrowserView: View {
    let url: URL
    @EnvironmentObject private var app: AppModel
    @StateObject private var model: BrowserModel
    @State private var selectedFolder: String? = ""

    init(url: URL) {
        self.url = url
        _model = StateObject(wrappedValue: BrowserModel(url: url))
    }

    var body: some View {
        NavigationSplitView {
            sidebar
                .navigationSplitViewColumnWidth(min: 180, ideal: 220, max: 320)
        } detail: {
            detail
        }
        .navigationTitle(url.lastPathComponent)
        .navigationSubtitle(subtitle)
        .searchable(text: $model.search, prompt: L10n.t("Search in archive"))
        .task(id: model.search) { await model.runSearch() }
        .task { await model.load(using: app) }
        .onDisappear { app.releaseOpenFiles(for: url) }
        .toolbar { toolbar }
        .frame(minWidth: 760, minHeight: 440)
        .onChange(of: selectedFolder) { newValue in
            if let f = newValue, f != model.currentFolder { model.currentFolder = f }
        }
        .onChange(of: model.currentFolder) { newValue in
            if selectedFolder != newValue { selectedFolder = newValue }
        }
    }

    private var subtitle: String {
        guard model.state == .ready else { return "" }
        return L10n.f("%ld files, %@", model.index.totalFileCount, Format.bytes(model.index.totalSize))
    }

    // MARK: Sidebar

    private var sidebar: some View {
        List(model.rootNodes, children: \.children, selection: $selectedFolder) { node in
            Label(node.name, systemImage: node.path.isEmpty ? "archivebox" : "folder")
                .lineLimit(1)
                .accessibilityLabel(L10n.f("Folder %@", node.name))
        }
        .listStyle(.sidebar)
    }

    // MARK: Detail

    @ViewBuilder
    private var detail: some View {
        switch model.state {
        case .loading:
            VStack(spacing: 12) {
                ProgressView()
                Text("Reading archive...").foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .accessibilityElement(children: .combine)
        case .failed(let error):
            VStack(spacing: 14) {
                Image(systemName: "exclamationmark.triangle").font(.system(size: 32, weight: .light)).foregroundStyle(.orange)
                Text(error.code.title).font(.title3.weight(.semibold))
                Text(error.code.guidance).foregroundStyle(.secondary).multilineTextAlignment(.center).frame(maxWidth: 420)
                if !error.message.isEmpty {
                    Text(error.message).font(.caption.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                }
                HStack {
                    Button("Try Again") { Task { await model.load(using: app) } }
                    Button("Copy Diagnostics") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(error.diagnostics, forType: .string)
                    }
                }
            }
            .padding(Theme.pad)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .ready:
            VStack(spacing: 0) {
                PathBar(model: model)
                Divider()
                HStack(spacing: 0) {
                    content
                    if model.showPreview, let entry = model.selectedEntries.first, !entry.isDirectory {
                        Divider()
                        PreviewPane(archive: url, entry: entry,
                                    password: model.password, codepage: model.codepage.id)
                            .frame(minWidth: 220, idealWidth: 300)
                            .transition(.move(edge: .trailing).combined(with: .opacity))
                    }
                    if model.showInspector {
                        Divider()
                        InspectorView(entry: model.selectedEntries.first, count: model.selection.count)
                            .frame(width: 250)
                            .transition(.move(edge: .trailing).combined(with: .opacity))
                    }
                }
                .animation(Theme.ease, value: model.showInspector)
                .animation(Theme.ease, value: model.showPreview)
                Divider()
                StatusBar(model: model)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if model.visible.isEmpty {
            VStack(spacing: 8) {
                Image(systemName: model.search.isEmpty ? "folder" : "magnifyingglass")
                    .font(.system(size: 28, weight: .light)).foregroundStyle(.tertiary)
                Text(model.search.isEmpty ? L10n.t("This folder is empty") : L10n.t("No matches"))
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            switch model.viewMode {
            case .details: detailsTable
            case .list: listView
            case .icons: iconGrid
            }
        }
    }

    private var detailsTable: some View {
        Table(model.visible, selection: $model.selection, sortOrder: $model.sortOrder) {
            TableColumn("Name", value: \.displayName) { e in
                Label(e.displayName, systemImage: e.systemImage).lineLimit(1)
            }
            .width(min: 160, ideal: 260)
            TableColumn("Size", value: \.size) { e in
                Text(Format.bytes(e.size, exact: app.prefs.view.exactSizes)).monospacedDigit()
            }
            .width(min: 70, ideal: 90)
            TableColumn("Packed", value: \.compressedSize) { e in
                Text(e.isDirectory ? "" : Format.bytes(e.compressedSize, exact: app.prefs.view.exactSizes)).monospacedDigit()
            }
            .width(min: 70, ideal: 90)
            TableColumn("Type", value: \.kindDescription) { e in Text(e.kindDescription) }
                .width(min: 70, ideal: 100)
            TableColumn("Modified", value: \.modifiedSort) { e in
                Text(e.modified.map { $0.formatted(date: .abbreviated, time: .shortened) } ?? "")
            }
            .width(min: 110, ideal: 150)
            TableColumn("Method", value: \.method) { e in Text(e.method).foregroundStyle(.secondary) }
                .width(min: 60, ideal: 80)
            TableColumn("Encryption", value: \.encryptionSort) { e in
                if e.encrypted {
                    Label(e.encryptionMethod ?? "Encrypted", systemImage: "lock.fill").labelStyle(.titleAndIcon)
                }
            }
            .width(min: 60, ideal: 90)
            TableColumn("CRC", value: \.crcSort) { e in
                Text(e.crc.map { String(format: "%08X", $0) } ?? "").font(.system(.body, design: .monospaced))
            }
            .width(min: 70, ideal: 90)
        }
        .contextMenu(forSelectionType: String.self) { ids in
            rowMenu(for: ids)
        } primaryAction: { ids in
            openFirst(of: ids)
        }
        .accessibilityLabel("Archive contents")
    }

    private var listView: some View {
        List(model.visible, selection: $model.selection) { e in
            HStack {
                Label(e.displayName, systemImage: e.systemImage).lineLimit(1)
                Spacer()
                Text(e.isDirectory ? "" : Format.bytes(e.size)).foregroundStyle(.secondary).monospacedDigit()
            }
            .frame(height: app.prefs.view.rowHeight.points)
            .contentShape(Rectangle())
            .onTapGesture(count: 2) { model.open(e, using: app) }
            .accessibilityLabel("\(e.displayName), \(e.kindDescription)")
        }
        .contextMenu(forSelectionType: String.self) { ids in
            rowMenu(for: ids)
        } primaryAction: { ids in
            openFirst(of: ids)
        }
    }

    private var iconGrid: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 100, maximum: 130), spacing: 12)], spacing: 12) {
                ForEach(model.visible) { e in
                    let selected = model.selection.contains(e.id)
                    VStack(spacing: 6) {
                        Image(systemName: e.systemImage)
                            .font(.system(size: 34, weight: .light))
                            .foregroundStyle(e.isDirectory ? Color.accentColor : Color.secondary)
                            .frame(height: 44)
                        Text(e.displayName).font(.callout).lineLimit(2).multilineTextAlignment(.center)
                    }
                    .padding(8)
                    .frame(maxWidth: .infinity)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(selected ? Color.accentColor.opacity(0.18) : .clear))
                    .contentShape(Rectangle())
                    .onTapGesture(count: 2) { model.open(e, using: app) }
                    .onTapGesture {
                        if NSEvent.modifierFlags.contains(.command) {
                            if selected { model.selection.remove(e.id) } else { model.selection.insert(e.id) }
                        } else {
                            model.selection = [e.id]
                        }
                    }
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel("\(e.displayName), \(e.kindDescription)")
                    .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
                }
            }
            .padding(16)
        }
    }

    @ViewBuilder
    private func rowMenu(for ids: Set<String>) -> some View {
        Button("Open") { openFirst(of: ids) }
        Button("Extract...") {
            model.selection = ids.isEmpty ? model.selection : ids
            model.extractSelected(using: app)
        }
        .disabled(ids.isEmpty && model.selection.isEmpty)
        Button("Show Info") {
            model.selection = ids
            model.showInspector = true
        }
        .disabled(ids.isEmpty)
        if model.isEditableZip {
            Divider()
            Button("Rename File") { model.beginRename(ids, using: app) }
                .disabled(ids.count != 1)
            Button("Delete File") { model.beginDelete(ids, using: app) }
                .disabled(ids.isEmpty)
        }
    }

    private func openFirst(of ids: Set<String>) {
        if let e = model.visible.first(where: { ids.contains($0.id) }) { model.open(e, using: app) }
    }

    // MARK: Toolbar

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .navigation) {
            Button { model.goUp() } label: { Image(systemName: "chevron.up") }
                .disabled(model.currentFolder.isEmpty)
                .help("Enclosing folder")
                .accessibilityLabel("Go to parent folder")
        }
        ToolbarItemGroup(placement: .primaryAction) {
            Picker("View", selection: $model.viewMode) {
                ForEach(BrowserViewMode.allCases) { m in
                    Label(m.title, systemImage: m.systemImage).tag(m)
                }
            }
            .pickerStyle(.segmented)
            .help("Change view mode")
            .accessibilityLabel("View mode")

            Menu {
                Button(CodePageCatalog.auto.name) { model.setCodepage(CodePageCatalog.auto, using: app) }
                Divider()
                ForEach(CodePageCatalog.groupedByRegion, id: \.region) { group in
                    Section(group.region) {
                        ForEach(group.pages) { cp in
                            Button {
                                model.setCodepage(cp, using: app)
                            } label: {
                                if cp == model.codepage { Label(cp.menuTitle, systemImage: "checkmark") }
                                else { Text(cp.menuTitle) }
                            }
                        }
                    }
                }
            } label: {
                Label("Codepage", systemImage: "character.bubble")
            }
            .help(L10n.f("Filename encoding: %@", model.codepage.name))
            .accessibilityLabel(L10n.f("Filename encoding menu, current %@", model.codepage.name))

            Menu {
                Button("Extract Here (Smart)") { app.extract(url, mode: .smart, codepage: model.codepage.id, password: model.password) }
                Button("Extract Here") { app.extract(url, mode: .here, codepage: model.codepage.id, password: model.password) }
                Button(L10n.f("Extract to %@/", SmartExtract.archiveBaseName(url.lastPathComponent))) {
                    app.extract(url, mode: .named, codepage: model.codepage.id, password: model.password)
                }
                Button("Extract to...") { app.extract(url, mode: .ask, codepage: model.codepage.id, password: model.password) }
                Divider()
                Button("Extract Selected...") { model.extractSelected(using: app) }
                    .disabled(model.selection.isEmpty)
            } label: {
                Label("Extract", systemImage: "arrow.down.doc")
            } primaryAction: {
                app.extract(url, mode: .smart, codepage: model.codepage.id, password: model.password)
            }
            .help("Extract (Smart)")
            .accessibilityLabel("Extract")

            Button {
                Task { await app.queue.enqueue(.test(archive: url.path, password: model.password)); app.openJobsWindow?() }
            } label: { Label("Test", systemImage: "checkmark.shield") }
                .help("Test archive")
                .accessibilityLabel("Test archive")

            Button { model.showPreview.toggle() } label: { Label("Preview", systemImage: "eye") }
                .help("Preview Image")
                .accessibilityLabel("Toggle preview")

            if model.isEditableZip {
                Button { model.addFiles(using: app) } label: { Label("Add File", systemImage: "plus.square.on.square") }
                    .help("Add files to this archive")
                    .accessibilityLabel("Add File")
            }

            Button { model.showInspector.toggle() } label: { Label("Inspector", systemImage: "info.circle") }
                .help("Show or hide entry details")
                .accessibilityLabel("Toggle inspector")
        }
    }
}

// MARK: - Path bar (FR-007)

private struct PathBar: View {
    @ObservedObject var model: BrowserModel

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 4) {
                ForEach(Array(model.pathComponents.enumerated()), id: \.offset) { i, comp in
                    if i > 0 {
                        Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
                    }
                    Button(comp.name) { model.currentFolder = comp.path }
                        .buttonStyle(.plain)
                        .foregroundStyle(i == model.pathComponents.count - 1 ? Color.primary : Color.secondary)
                        .accessibilityLabel(L10n.f("Go to %@", comp.name))
                }
            }
            .padding(.horizontal, 12)
            .frame(height: 30)
        }
        .background(.bar)
    }
}

// MARK: - Status bar (FR-014)

private struct StatusBar: View {
    @ObservedObject var model: BrowserModel

    var body: some View {
        let selected = model.selectedEntries
        let bytes = selected.reduce(UInt64(0)) { $0 + $1.size }
        HStack {
            Text(L10n.f("%ld items", model.visible.count))
            Spacer()
            if !selected.isEmpty {
                Text(L10n.f("%ld selected, %@", selected.count, Format.bytes(bytes)))
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .frame(height: 24)
        .background(.bar)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(L10n.f("%ld items, %ld selected", model.visible.count, selected.count))
    }
}

// MARK: - Inspector (FR-012)

private struct InspectorView: View {
    let entry: ArchiveEntry?
    let count: Int

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let e = entry {
                    Label(e.displayName, systemImage: e.systemImage).font(.headline)
                    if count > 1 { Text(L10n.f("+%ld more selected", count - 1)).font(.caption).foregroundStyle(.secondary) }
                    row("Path", e.name)
                    row("Type", e.kindDescription)
                    row("Size", Format.bytes(e.size, exact: true))
                    row("Packed", Format.bytes(e.compressedSize, exact: true))
                    if let r = e.compressionRatio { row("Saved", Format.percent(r)) }
                    row("Method", e.method)
                    row("Encryption", e.encrypted ? (e.encryptionMethod ?? "Yes") : "None")
                    row("CRC", e.crc.map { String(format: "%08X", $0) } ?? "")
                    row("Attributes", e.attributes.map { String($0, radix: 16) } ?? "")
                    row("Modified", e.modified?.formatted(date: .complete, time: .standard) ?? "")
                    if let c = e.comment, !c.isEmpty { row("Comment", c) }
                } else {
                    Text("Select an item to see its details.").foregroundStyle(.secondary)
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(.background)
    }

    @ViewBuilder
    private func row(_ k: String, _ v: String) -> some View {
        if !v.isEmpty {
            VStack(alignment: .leading, spacing: 1) {
                Text(k).font(.caption).foregroundStyle(.secondary)
                Text(v).textSelection(.enabled)
            }
            .accessibilityElement(children: .combine)
        }
    }
}
