import AppKit
import SwiftUI
import FastZipKit

/// Finder windows are independent of the dashboard's lifetime and scene setup.
@MainActor
final class QuickActionWindows {
    private var windows: [UUID: QuickWindow] = [:]

    private func present<V: View>(_ view: V, title: String, id: UUID = UUID()) -> QuickWindow {
        let controller = QuickWindow(view: view, title: title)
        windows[id] = controller
        controller.onClose = { [weak self] in self?.windows.removeValue(forKey: id) }
        controller.showWindow(nil)
        controller.window?.makeKeyAndOrderFront(nil)
        return controller
    }

    func chooseCompression(paths: [String], app: AppModel) {
        guard let first = paths.first else { return }
        let id = UUID()
        let view = QuickCompressionView(destination: URL(fileURLWithPath: first).deletingLastPathComponent()) { [weak self, weak app] format, destination in
            self?.windows[id]?.close()
            if let format, let app {
                app.quickCompress(paths: paths, format: format, deleteSources: false, each: false,
                                  preset: nil, rule: nil, destination: destination)
            }
        }
        _ = present(view, title: L10n.t("Compress"), id: id)
        NSApp.activate(ignoringOtherApps: true)
    }

    func progress(title: String, app: AppModel) -> QuickProgress {
        let state = QuickProgress(title: title)
        let controller = present(QuickProgressView(state: state, app: app), title: title)
        controller.window?.standardWindowButton(.closeButton)?.isEnabled = false
        state.onFinished = { [weak controller] in controller?.close() }
        state.onNeedsAttention = { [weak controller] in
            controller?.window?.standardWindowButton(.closeButton)?.isEnabled = true
        }
        return state
    }
}

@MainActor
private final class QuickWindow: NSWindowController, NSWindowDelegate {
    var onClose: (() -> Void)?

    init<V: View>(view: V, title: String) {
        let window = NSPanel(contentRect: .zero, styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = title
        window.isReleasedWhenClosed = false
        window.hidesOnDeactivate = false
        window.contentViewController = NSHostingController(rootView: view)
        super.init(window: window)
        window.delegate = self
        window.center()
    }

    required init?(coder: NSCoder) { fatalError("Not supported") }
    func windowWillClose(_ notification: Notification) { onClose?() }
}

@MainActor
final class QuickProgress: ObservableObject {
    let title: String
    @Published var jobs: [Job] = []
    @Published var preparing = true
    @Published var error: String?
    var ids: Set<JobID> = []
    var onFinished: (() -> Void)?
    var onNeedsAttention: (() -> Void)?
    private var closing = false

    init(title: String) { self.title = title }

    func update(_ snapshot: [Job]) {
        jobs = snapshot.filter { ids.contains($0.id) }
        guard !preparing else { return }
        if error != nil || jobs.contains(where: { $0.state == .failed || $0.state == .cancelled }) {
            onNeedsAttention?()
        } else if jobs.count == ids.count && jobs.allSatisfy({ $0.state.isTerminal }) && !closing {
            closing = true
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .milliseconds(800))
                self?.onFinished?()
            }
        }
    }

    func finish(_ snapshot: [Job], error: String? = nil) {
        self.error = error
        preparing = false
        update(snapshot)
    }
}

private struct QuickCompressionView: View {
    @State var destination: URL
    @State private var format = ArchiveFormat.zip
    let completion: (ArchiveFormat?, URL) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Picker("Archive format", selection: $format) {
                Text("ZIP").tag(ArchiveFormat.zip)
                Text("7z").tag(ArchiveFormat.sevenZ)
            }
            .pickerStyle(.segmented)
            VStack(alignment: .leading, spacing: 8) {
                Text("Destination").font(.caption).foregroundStyle(.secondary)
                HStack {
                    Image(systemName: "folder")
                    Text(destination.displayPath).lineLimit(1).truncationMode(.middle)
                        .help(destination.path)
                    Spacer(minLength: 8)
                    Button("Browse…") {
                        Task { @MainActor in
                            if let folder = await FilePanels.chooseFolder(startingAt: destination) { destination = folder }
                        }
                    }
                }
            }
            HStack {
                Spacer()
                Button("Cancel") { completion(nil, destination) }.keyboardShortcut(.cancelAction)
                Button("Compress") { completion(format, destination) }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 400)
    }
}

private struct QuickProgressView: View {
    @ObservedObject var state: QuickProgress
    @ObservedObject var app: AppModel

    private var active: Job? { state.jobs.first { !$0.state.isTerminal } ?? state.jobs.last }
    private var failure: String? { state.error ?? state.jobs.first(where: { $0.state == .failed })?.error?.localizedDescription }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(state.title).font(.headline).lineLimit(1).truncationMode(.middle)
            if let failure {
                Text(failure).font(.callout).foregroundStyle(.red).textSelection(.enabled)
            } else if state.preparing || active?.state == .queued {
                ProgressView().progressViewStyle(.linear)
                Text("Preparing…").font(.caption).foregroundStyle(.secondary)
            } else if let job = active {
                ProgressView(value: job.state == .success ? 1 : job.progress.fraction)
                HStack {
                    Text(job.state.displayName)
                    Spacer()
                    if job.state == .running {
                        Text(Format.percent(job.progress.fraction))
                        Text(Format.rate(job.progress.throughput))
                    }
                }.font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            }
            HStack {
                Spacer()
                if state.preparing {
                    EmptyView()
                } else if state.jobs.contains(where: { !$0.state.isTerminal }) {
                    Button("Cancel") {
                        Task { for id in state.ids { await app.queue.cancel(id) } }
                    }
                } else if failure != nil || state.jobs.contains(where: { $0.state == .cancelled }) {
                    Button("Close") { state.onFinished?() }.keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(20)
        .frame(width: 400)
    }
}
