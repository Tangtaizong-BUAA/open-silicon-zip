import SwiftUI
import AppKit
import FastZipKit

/// Start screen (FR-508): two drop targets, recents, quiet footer links.
struct DashboardView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 36) {
                header
                if !model.engineAvailable { engineBanner }
                HStack(spacing: Theme.gap) {
                    DropCard(title: L10n.t("Open Archive"),
                             subtitle: L10n.t("Choose or drop an archive"),
                             systemImage: "tray.and.arrow.down",
                             onTap: openPicker,
                             onDrop: { urls in urls.forEach { model.openArchive($0) } })
                    DropCard(title: L10n.t("New Archive"),
                             subtitle: L10n.t("Choose or drop files to compress"),
                             systemImage: "archivebox",
                             onTap: { openWindow(id: "new", value: NewArchiveSeed()) },
                             onDrop: { urls in
                                 openWindow(id: "new", value: NewArchiveSeed(paths: urls.map(\.path)))
                             })
                }
                recents
                footer
            }
            .padding(.horizontal, 48)
            .padding(.vertical, 40)
            .frame(maxWidth: 820)
            .frame(maxWidth: .infinity)
        }
        .frame(minWidth: 640, minHeight: 540)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Open Silicon Zip")
                .font(.system(size: 34, weight: .semibold, design: .rounded))
            Text("Compress and extract, quickly.")
                .font(.title3)
                .foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
    }

    private var engineBanner: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 2) {
                Text(ErrorCode.engineMissing.title).font(.callout.weight(.medium))
                Text(ErrorCode.engineMissing.guidance).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Button("Preferences") { showSettings() }
        }
        .padding(12)
        .card()
    }

    private var recents: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Recent").font(.headline)
                Spacer()
                if !model.recents.isEmpty {
                    Button("Clear") { withAnimation(Theme.ease) { model.clearRecents() } }
                        .buttonStyle(.link)
                        .accessibilityLabel("Clear recent files")
                }
            }
            if model.recents.isEmpty {
                Text("Archives you open will appear here.")
                    .foregroundStyle(.secondary)
                    .padding(.vertical, 8)
            } else {
                VStack(spacing: 0) {
                    ForEach(model.recents, id: \.self) { url in
                        RecentRow(url: url) { model.openArchive(url) }
                        if url != model.recents.last { Divider().opacity(0.5) }
                    }
                }
                .card()
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 18) {
            Button("Jobs") { openWindow(id: "jobs") }
            Button("Tools") { openWindow(id: "tools") }
            Button("Passwords") { openWindow(id: "passwords") }
            Spacer()
            if model.summary.running > 0 {
                Text(L10n.f("%ld running, %@", model.summary.running, Format.rate(model.throughput)))
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .buttonStyle(.link)
        .font(.callout)
    }

    private func openPicker() {
        Task {
            for u in await FilePanels.chooseFiles(allowFolders: false, message: L10n.t("Choose archives to open")) {
                model.openArchive(u)
            }
        }
    }
}

private struct RecentRow: View {
    let url: URL
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Image(nsImage: NSWorkspace.shared.icon(forFile: url.path))
                    .resizable().frame(width: 28, height: 28)
                VStack(alignment: .leading, spacing: 1) {
                    Text(url.lastPathComponent).lineLimit(1)
                    Text(url.deletingLastPathComponent().displayPath)
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(hovering ? Color.primary.opacity(0.05) : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .contextMenu {
            Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
        }
        .accessibilityLabel(L10n.f("Open %@", url.lastPathComponent))
    }
}

/// Opens the Settings scene (macOS 13 selector; macOS 14+ also accepts it).
@MainActor
func showSettings() {
    NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
}
