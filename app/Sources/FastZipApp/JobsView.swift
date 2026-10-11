import SwiftUI
import AppKit
import FastZipKit

/// Job / progress window (FR-402, 05 section 9). All actions go through the JobQueue actor.
struct JobsView: View {
    @EnvironmentObject private var app: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if app.jobs.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "checkmark.circle").font(.system(size: 30, weight: .light)).foregroundStyle(.tertiary)
                    Text("No jobs").foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 10) {
                        ForEach(app.jobs.reversed()) { job in
                            JobRow(job: job)
                                .transition(.opacity.combined(with: .move(edge: .top)))
                        }
                    }
                    .padding(16)
                    .animation(Theme.ease, value: app.jobs.map(\.id))
                }
            }
            Divider()
            footer
        }
        .frame(minWidth: 520, minHeight: 360)
        .onChange(of: app.summary) { s in
            if app.prefs.general.closeProgressOnSuccess, s.total > 0, s.isIdle, s.failed == 0, s.cancelled == 0 {
                dismiss()
            }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text("Jobs").font(.title2.weight(.semibold))
                Spacer()
                Text(statusLine).font(.callout).foregroundStyle(.secondary)
            }
            if app.summary.running > 0 {
                HStack(spacing: 6) {
                    Image(systemName: "speedometer").foregroundStyle(.secondary)
                    Text(Format.rate(app.throughput)).monospacedDigit()
                    Text("total throughput").foregroundStyle(.secondary)
                }
                .font(.caption)
                .accessibilityElement(children: .combine)
            }
        }
        .padding(16)
    }

    private var statusLine: String {
        let s = app.summary
        var parts: [String] = []
        if s.running > 0 { parts.append(L10n.f("%ld running", s.running)) }
        if s.queued > 0 { parts.append(L10n.f("%ld queued", s.queued)) }
        if s.paused > 0 { parts.append(L10n.f("%ld paused", s.paused)) }
        if s.success > 0 { parts.append(L10n.f("%ld done", s.success)) }
        if s.failed > 0 { parts.append(L10n.f("%ld failed", s.failed)) }
        if s.skipped > 0 { parts.append(L10n.f("%ld skipped", s.skipped)) }
        return parts.joined(separator: ", ")
    }

    private var footer: some View {
        HStack {
            Toggle("Precise time", isOn: $app.prefs.advanced.preciseProgressTime)
                .toggleStyle(.checkbox)
                .help("Show hh:mm:ss instead of rounded estimates")
            Spacer()
            Button("Pause All") { Task { await app.queue.pauseAll() } }
                .disabled(app.summary.running == 0)
            Button("Resume All") { Task { await app.queue.resumeAll() } }
                .disabled(app.summary.paused == 0)
            Button("Cancel All", role: .destructive) { Task { await app.queue.cancelAll() } }
                .disabled(app.summary.isIdle && app.summary.paused == 0)
            Button("Retry Failed") { Task { await app.queue.retryFailed() } }
                .disabled(app.summary.failed == 0)
            Button("Clear Finished") { Task { await app.queue.clearFinished() } }
        }
        .controlSize(.small)
        .padding(12)
    }
}

private struct JobRow: View {
    @EnvironmentObject private var app: AppModel
    @Environment(\.openWindow) private var openWindow
    let job: Job
    @State private var showLog = false

    private var precise: Bool { app.prefs.advanced.preciseProgressTime }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: job.kind.systemImage)
                    .frame(width: 22)
                    .foregroundStyle(.secondary)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 1) {
                    Text(job.title).font(.headline).lineLimit(1)
                    Text(L10n.t(subtitle)).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
                Spacer()
                StateBadge(state: job.state)
                controls
            }
            if job.state == .running || job.state == .paused {
                SlimProgress(fraction: job.progress.fraction, tint: job.state == .paused ? .orange : .accentColor)
                HStack {
                    Text(Format.percent(job.progress.fraction))
                    Text(Format.rate(job.progress.throughput))
                    Spacer()
                    Text(L10n.f("Elapsed %@", Format.duration(job.progress.elapsed, precise: precise)))
                    Text(L10n.f("Remaining %@", Format.duration(job.progress.eta, precise: precise)))
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .accessibilityElement(children: .combine)
            }
            if let error = job.error, job.state == .failed {
                VStack(alignment: .leading, spacing: 4) {
                    Text(error.code.title).font(.callout.weight(.medium))
                    Text(error.code.guidance).font(.caption).foregroundStyle(.secondary)
                    if !error.message.isEmpty {
                        Text(error.message).font(.caption.monospaced()).foregroundStyle(.tertiary).textSelection(.enabled)
                    }
                    HStack {
                        ForEach(Array(error.code.recoveries.enumerated()), id: \.offset) { _, r in
                            recoveryButton(r, error: error)
                        }
                    }
                    .controlSize(.small)
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color.orange.opacity(0.08)))
            }
            if job.kind == .hash || job.kind == .test, !job.hashes.isEmpty || job.state == .success {
                results
            }
            if showLog { logView }
        }
        .padding(14)
        .card()
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(job.kind.displayName) \(job.title), \(job.state.displayName)")
        .contextMenu {
            Button(showLog ? L10n.t("Hide Log") : L10n.t("Show Log")) { showLog.toggle() }
            Button("Export Log...") { exportLog() }
            if job.state == .success, case .create(let c) = job.command {
                Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: c.output)]) }
            }
        }
    }

    private var subtitle: String {
        switch job.state {
        case .running, .paused:
            let p = job.progress
            let phase = p.phase.isEmpty ? job.kind.displayName : L10n.t(p.phase.capitalized)
            return p.current.isEmpty ? phase : "\(phase): \(p.current)"
        case .success: return L10n.f("Completed in %@", Format.duration(job.progress.elapsed, precise: precise))
        case .queued: return "Waiting"
        default: return job.kind.displayName
        }
    }

    @ViewBuilder
    private var controls: some View {
        HStack(spacing: 4) {
            switch job.state {
            case .running:
                iconButton("pause.fill", "Pause") { Task { await app.queue.pause(job.id) } }
                iconButton("xmark", "Cancel") { Task { await app.queue.cancel(job.id) } }
            case .paused:
                iconButton("play.fill", "Resume") { Task { await app.queue.resume(job.id) } }
                iconButton("xmark", "Cancel") { Task { await app.queue.cancel(job.id) } }
            case .queued:
                iconButton("forward.end.fill", "Skip") { Task { await app.queue.skip(job.id) } }
                iconButton("xmark", "Cancel") { Task { await app.queue.cancel(job.id) } }
            case .failed, .cancelled:
                iconButton("arrow.clockwise", "Retry") { Task { await app.queue.retry(job.id) } }
            default:
                EmptyView()
            }
        }
    }

    private func iconButton(_ symbol: String, _ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: symbol).frame(width: 18, height: 18) }
            .buttonStyle(.borderless)
            .help(L10n.t(label))
            .accessibilityLabel("\(L10n.t(label)) \(job.title)")
    }

    @ViewBuilder
    private func recoveryButton(_ r: ErrorCode.Recovery, error: FZError) -> some View {
        switch r {
        case .retry: Button("Retry") { Task { await app.queue.retry(job.id) } }
        case .enterPassword: Button("Enter Password...") { Task { await app.retryWithPassword(job) } }
        case .copyDiagnostics:
            Button("Copy Diagnostics") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(error.diagnostics + "\n" + job.log.joined(separator: "\n"), forType: .string)
            }
        case .openLog: Button("Show Log") { showLog = true }
        case .repair:
            Button("Repair...") { openWindow(id: "tools") }
        case .skip, .chooseDestination, .locateVolume, .abort: EmptyView()
        }
    }

    @ViewBuilder
    private var results: some View {
        if job.kind == .test {
            Label(job.state == .success ? L10n.t("No errors found") : L10n.t("Waiting for result"), systemImage: "checkmark.seal")
                .font(.callout).foregroundStyle(job.state == .success ? Color.green : Color.secondary)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(job.hashes) { h in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(URL(fileURLWithPath: h.path).lastPathComponent).font(.caption.weight(.medium))
                        ForEach(h.digests.sorted(by: { $0.key < $1.key }), id: \.key) { k, v in
                            Text("\(k.uppercased())  \(v)").font(.system(.caption2, design: .monospaced)).textSelection(.enabled)
                        }
                    }
                }
            }
        }
    }

    private var logView: some View {
        ScrollView {
            Text(job.log.isEmpty ? L10n.t("No log output.") : job.log.joined(separator: "\n"))
                .font(.system(.caption2, design: .monospaced))
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
        }
        .frame(maxHeight: 120)
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.cardFill))
    }

    private func exportLog() {
        Task { @MainActor in
            guard let url = await FilePanels.saveFile(suggested: "\(job.title)-log.txt") else { return }
            var text = "Job: \(job.title)\nKind: \(job.kind.rawValue)\nState: \(job.state.rawValue)\nAttempt: \(job.attempt)\n"
            if let e = job.error { text += e.diagnostics + "\n" }
            text += "\n" + job.log.joined(separator: "\n")
            try? text.write(to: url, atomically: true, encoding: .utf8)
        }
    }
}

private struct StateBadge: View {
    let state: JobState

    var body: some View {
        Text(state.displayName)
            .font(.caption2.weight(.medium))
            .padding(.horizontal, 8).padding(.vertical, 3)
            .background(Capsule().fill(color.opacity(0.14)))
            .foregroundStyle(color)
            .accessibilityHidden(true)
    }

    private var color: Color {
        switch state {
        case .success: return .green
        case .failed: return .red
        case .running: return .accentColor
        case .paused: return .orange
        default: return .secondary
        }
    }
}
