import SwiftUI
import AppKit
import FastZipKit

/// Password field with a show/hide toggle (FR-105/601).
struct RevealableSecureField: View {
    let title: String
    @Binding var text: String
    @State private var revealed = false

    var body: some View {
        HStack(spacing: 6) {
            Group {
                if revealed {
                    TextField(title, text: $text)
                } else {
                    SecureField(title, text: $text)
                }
            }
            .textFieldStyle(.roundedBorder)
            .accessibilityLabel(L10n.t(title))
            Button {
                revealed.toggle()
            } label: {
                Image(systemName: revealed ? "eye.slash" : "eye")
            }
            .buttonStyle(.borderless)
            .help(revealed ? L10n.t("Hide password") : L10n.t("Show password"))
            .accessibilityLabel(revealed ? L10n.t("Hide password") : L10n.t("Show password"))
        }
    }
}

/// Large, calm drop target card used on the dashboard.
struct DropCard: View {
    let title: String
    let subtitle: String
    let systemImage: String
    let onTap: () -> Void
    let onDrop: ([URL]) -> Void
    @State private var targeted = false
    @State private var hovering = false

    var body: some View {
        Button(action: onTap) {
            VStack(spacing: 10) {
                Image(systemName: systemImage)
                    .font(.system(size: 34, weight: .light))
                    .foregroundStyle(targeted || hovering ? Color.accentColor : Color.secondary)
                Text(L10n.t(title)).font(.title3.weight(.medium))
                Text(L10n.t(subtitle)).font(.callout).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, minHeight: 170)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .card(highlighted: targeted)
        .scaleEffect(targeted ? 1.015 : 1)
        .animation(Theme.ease, value: targeted)
        .onHover { hovering = $0 }
        .dropDestination(for: URL.self) { urls, _ in
            onDrop(urls)
            return !urls.isEmpty
        } isTargeted: { targeted = $0 }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(L10n.t(title))
        .accessibilityHint(L10n.f("%@. You can also drop files here.", L10n.t(subtitle)))
        .accessibilityAddTraits(.isButton)
    }
}

extension URL {
    var displayPath: String { (path as NSString).abbreviatingWithTildeInPath }
}

@MainActor
enum FilePanels {
    static func chooseFolder(prompt: String = "Choose", message: String? = nil, startingAt: URL? = nil) async -> URL? {
        await withCheckedContinuation { (c: CheckedContinuation<URL?, Never>) in
            let p = NSOpenPanel()
            p.directoryURL = startingAt
            p.canChooseDirectories = true
            p.canChooseFiles = false
            p.canCreateDirectories = true
            p.prompt = L10n.t(prompt)
            if let message { p.message = message }
            p.begin { r in c.resume(returning: r == .OK ? p.url : nil) }
        }
    }

    static func chooseFiles(allowFolders: Bool = true, message: String? = nil) async -> [URL] {
        await withCheckedContinuation { (c: CheckedContinuation<[URL], Never>) in
            let p = NSOpenPanel()
            p.canChooseDirectories = allowFolders
            p.canChooseFiles = true
            p.allowsMultipleSelection = true
            if let message { p.message = message }
            p.begin { r in c.resume(returning: r == .OK ? p.urls : []) }
        }
    }

    static func saveFile(suggested: String) async -> URL? {
        await withCheckedContinuation { (c: CheckedContinuation<URL?, Never>) in
            let p = NSSavePanel()
            p.nameFieldStringValue = suggested
            p.begin { r in c.resume(returning: r == .OK ? p.url : nil) }
        }
    }
}

/// Presents SwiftUI content as a sheet on the key window (or a floating panel) and
/// suspends until the content reports a result. Used for password, conflict and error prompts
/// so they appear once, app-wide, regardless of which window is frontmost.
@MainActor
enum Presenter {
    static func present<T, V: View>(defaultValue: T, @ViewBuilder _ build: (@escaping (T) -> Void) -> V) async -> T {
        await withCheckedContinuation { (cont: CheckedContinuation<T, Never>) in
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 460, height: 320),
                                  styleMask: [.titled], backing: .buffered, defer: false)
            window.titleVisibility = .hidden
            window.titlebarAppearsTransparent = true
            let box = ResumeBox<T>(cont)
            let finish: (T) -> Void = { [weak window] value in
                guard let window else { box.resume(value); return }
                if let parent = window.sheetParent {
                    parent.endSheet(window)
                } else {
                    window.orderOut(nil)
                }
                box.resume(value)
            }
            let host = NSHostingController(rootView: build(finish))
            host.sizingOptions = [.preferredContentSize]
            window.contentViewController = host
            if let parent = NSApp.keyWindow ?? NSApp.mainWindow {
                parent.beginSheet(window)
            } else {
                window.center()
                window.makeKeyAndOrderFront(nil)
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }
}

/// Guarantees a continuation is resumed exactly once.
@MainActor
final class ResumeBox<T> {
    private var cont: CheckedContinuation<T, Never>?
    init(_ c: CheckedContinuation<T, Never>) { cont = c }
    func resume(_ v: T) {
        cont?.resume(returning: v)
        cont = nil
    }
}
