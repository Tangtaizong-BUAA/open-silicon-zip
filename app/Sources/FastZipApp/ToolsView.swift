import SwiftUI
import AppKit
import FastZipKit

/// Tools window: Test (FR-701), Repair (FR-702), Checksum (FR-704), Recovery wizard shell (FR-703/705).
/// Each tool enqueues a normal job, so progress, cancel and errors behave like everywhere else.
struct ToolsView: View {
    private enum Tool: String, CaseIterable, Identifiable {
        case test = "Test", repair = "Repair", checksum = "Checksum", recovery = "Recovery"
        var id: String { rawValue }
        var icon: String {
            switch self {
            case .test: return "checkmark.shield"
            case .repair: return "wrench.and.screwdriver"
            case .checksum: return "number"
            case .recovery: return "key"
            }
        }
    }

    @State private var tool: Tool = .test

    var body: some View {
        VStack(spacing: 0) {
            Picker("Tool", selection: $tool) {
                ForEach(Tool.allCases) { Label($0.rawValue, systemImage: $0.icon).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(16)
            .accessibilityLabel("Tool")
            Divider()
            Group {
                switch tool {
                case .test: TestTool()
                case .repair: RepairTool()
                case .checksum: ChecksumTool()
                case .recovery: RecoveryWizard()
                }
            }
            .padding(Theme.pad)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .frame(minWidth: 600, minHeight: 480)
    }
}

/// Picks one or more files via panel or drop.
private struct FilePicker: View {
    let prompt: String
    let allowFolders: Bool
    @Binding var urls: [URL]
    @State private var targeted = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(urls.isEmpty ? L10n.t(prompt) : L10n.f("%ld selected", urls.count)).foregroundStyle(.secondary)
                Spacer()
                Button("Choose...") {
                    Task { let u = await FilePanels.chooseFiles(allowFolders: allowFolders); if !u.isEmpty { urls = u } }
                }
            }
            if !urls.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(urls.prefix(6), id: \.self) { Text($0.displayPath).font(.caption).lineLimit(1).truncationMode(.middle) }
                    if urls.count > 6 { Text(L10n.f("and %ld more", urls.count - 6)).font(.caption).foregroundStyle(.secondary) }
                }
            }
        }
        .padding(12)
        .card(highlighted: targeted)
        .dropDestination(for: URL.self) { dropped, _ in urls = dropped; return !dropped.isEmpty } isTargeted: { targeted = $0 }
    }
}

/// Shows the latest job that a tool started.
private struct JobStatus: View {
    @EnvironmentObject private var app: AppModel
    let jobID: JobID?
    var successText = "Done"

    var body: some View {
        if let id = jobID, let job = app.jobs.first(where: { $0.id == id }) {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text(job.state.displayName).font(.headline)
                    Spacer()
                    if job.state == .running { Button("Cancel") { Task { await app.queue.cancel(id) } } }
                }
                if job.state == .running || job.state == .paused {
                    SlimProgress(fraction: job.progress.fraction)
                    Text(job.progress.current).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                if job.state == .success { Label(L10n.t(successText), systemImage: "checkmark.seal").foregroundStyle(.green) }
                if let e = job.error, job.state == .failed {
                    Text(e.code.title).font(.callout.weight(.medium))
                    Text(e.code.guidance).font(.caption).foregroundStyle(.secondary)
                    if e.code.kind == .password {
                        Button("Enter Password...") { Task { await app.retryWithPassword(job) } }
                    }
                }
            }
            .padding(12)
            .card()
            .accessibilityElement(children: .contain)
        }
    }
}

private struct TestTool: View {
    @EnvironmentObject private var app: AppModel
    @State private var urls: [URL] = []
    @State private var ids: [JobID] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Test archives").font(.title3.weight(.semibold))
            Text("Verifies every entry's checksum without extracting anything.").foregroundStyle(.secondary)
            FilePicker(prompt: L10n.t("Choose or drop archives"), allowFolders: false, urls: $urls)
            Button("Test") {
                Task {
                    var new: [JobID] = []
                    for u in urls { new.append(await app.queue.enqueue(.test(archive: u.path, password: nil))) }
                    ids = new
                }
            }
            .disabled(urls.isEmpty)
            ForEach(ids, id: \.self) { JobStatus(jobID: $0, successText: "No errors found") }
        }
    }
}

private struct RepairTool: View {
    @EnvironmentObject private var app: AppModel
    @State private var urls: [URL] = []
    @State private var id: JobID?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Repair a ZIP").font(.title3.weight(.semibold))
            Text("Salvages readable entries into a new file. The original is never modified.").foregroundStyle(.secondary)
            FilePicker(prompt: L10n.t("Choose or drop a damaged .zip"), allowFolders: false, urls: $urls)
            Button("Repair") {
                guard let src = urls.first else { return }
                let out = src.deletingLastPathComponent()
                    .appendingPathComponent(src.deletingPathExtension().lastPathComponent + " (repaired).zip")
                Task { id = await app.queue.enqueue(.repair(archive: src.path, output: out.path)) }
            }
            .disabled(urls.isEmpty)
            JobStatus(jobID: id, successText: "Repaired copy saved next to the original")
        }
    }
}

private struct ChecksumTool: View {
    @EnvironmentObject private var app: AppModel
    @State private var urls: [URL] = []
    @State private var id: JobID?
    @State private var expected = ""

    private var job: Job? { app.jobs.first { $0.id == id } }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Checksums").font(.title3.weight(.semibold))
            Text("CRC32, CRC64, MD5, SHA-1 and SHA-256 for any files.").foregroundStyle(.secondary)
            FilePicker(prompt: L10n.t("Choose or drop files"), allowFolders: false, urls: $urls)
            HStack {
                Button("Calculate") {
                    Task { id = await app.queue.enqueue(.hash(files: urls.map(\.path)), title: L10n.t("Checksums")) }
                }
                .disabled(urls.isEmpty)
                TextField("Expected value to compare (optional)", text: $expected)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(.body, design: .monospaced))
                    .accessibilityLabel("Expected checksum")
            }
            JobStatus(jobID: id)
            if let job {
                ScrollView {
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(job.hashes) { h in
                            VStack(alignment: .leading, spacing: 3) {
                                Text(URL(fileURLWithPath: h.path).lastPathComponent).font(.callout.weight(.medium))
                                ForEach(h.digests.sorted(by: { $0.key < $1.key }), id: \.key) { k, v in
                                    HStack {
                                        Text(k.uppercased()).frame(width: 64, alignment: .leading).foregroundStyle(.secondary)
                                        Text(v).textSelection(.enabled)
                                        Spacer()
                                        matchBadge(v)
                                    }
                                    .font(.system(.caption, design: .monospaced))
                                }
                            }
                        }
                    }
                }
                if !job.hashes.isEmpty {
                    Button("Copy All") {
                        let text = job.hashes.map { h in
                            h.path + "\n" + h.digests.sorted(by: { $0.key < $1.key }).map { "  \($0.key.uppercased()) \($0.value)" }.joined(separator: "\n")
                        }.joined(separator: "\n")
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(text, forType: .string)
                    }
                }
            }
        }
    }

    /// FR-704: report match / partial match for a pasted expected value.
    @ViewBuilder
    private func matchBadge(_ value: String) -> some View {
        let e = expected.trimmingCharacters(in: .whitespaces).lowercased()
        if !e.isEmpty {
            let v = value.lowercased()
            if v == e {
                Label("Match", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
            } else if e.count >= 6 && (v.hasPrefix(e) || v.hasSuffix(e)) {
                Label("Partial match", systemImage: "circle.lefthalf.filled").foregroundStyle(.orange)
            }
        }
    }
}

/// Recovery wizard shell (FR-703/705): Open > Inspect > Settings > Run > Result.
/// Run uses RecoveryRunner to stream engine results without blocking the UI.
private struct RecoveryWizard: View {
    @EnvironmentObject private var app: AppModel
    private enum Step: Int, CaseIterable { case open, inspect, settings, run, result
        var title: String { L10n.t(["Open", "Inspect", "Settings", "Run", "Result"][rawValue]) }
    }

    @State private var step: Step = .open
    @State private var urls: [URL] = []
    @State private var entries: [ArchiveEntry] = []
    @State private var error: FZError?
    @State private var digits = true
    @State private var lower = true
    @State private var upper = false
    @State private var special = false
    @State private var space = false
    @State private var custom = ""
    @State private var prefix = ""
    @State private var minLen = 1.0
    @State private var maxLen = 6.0
    @State private var runner: RecoveryRunner?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 6) {
                ForEach(Step.allCases, id: \.rawValue) { s in
                    Text(s.title)
                        .font(.caption.weight(s == step ? .semibold : .regular))
                        .padding(.horizontal, 10).padding(.vertical, 4)
                        .background(Capsule().fill(s == step ? Color.accentColor.opacity(0.15) : .clear))
                        .foregroundStyle(s.rawValue <= step.rawValue ? Color.primary : Color.secondary)
                }
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel(L10n.f("Step %ld of 5: %@", step.rawValue + 1, step.title))

            switch step {
            case .open:
                Text("Choose the encrypted archive you lost the password for.").foregroundStyle(.secondary)
                FilePicker(prompt: L10n.t("Choose or drop an archive"), allowFolders: false, urls: $urls)
            case .inspect:
                if let error {
                    Text(error.code.title).font(.headline)
                    Text(error.code.guidance).font(.caption).foregroundStyle(.secondary)
                } else if entries.isEmpty {
                    ProgressView("Reading entries...")
                } else {
                    Text(L10n.f("%ld entries, %ld encrypted", entries.count, entries.filter(\.encrypted).count)).font(.headline)
                    List(entries.prefix(200)) { e in
                        HStack {
                            Text(e.name).lineLimit(1)
                            Spacer()
                            Text(e.encrypted ? (e.encryptionMethod ?? "Encrypted") : "Not encrypted").foregroundStyle(.secondary)
                        }
                    }
                    .frame(maxHeight: 200)
                }
            case .settings:
                Text("Character set").font(.headline)
                HStack(spacing: 16) {
                    Toggle("0-9", isOn: $digits); Toggle("a-z", isOn: $lower); Toggle("A-Z", isOn: $upper)
                    Toggle("Symbols", isOn: $special); Toggle("Space", isOn: $space)
                }
                .toggleStyle(.checkbox)
                TextField("Custom characters (optional)", text: $custom).textFieldStyle(.roundedBorder)
                TextField("Known prefix (optional)", text: $prefix).textFieldStyle(.roundedBorder)
                HStack {
                    Text(L10n.f("Length %ld to %ld", Int(minLen), Int(maxLen)))
                    Slider(value: $minLen, in: 1...12, step: 1).onChange(of: minLen) { if $0 > maxLen { maxLen = $0 } }
                    Slider(value: $maxLen, in: 1...12, step: 1).onChange(of: maxLen) { if $0 < minLen { minLen = $0 } }
                }
            case .run:
                VStack(alignment: .leading, spacing: 10) {
                    if let r = runner {
                        switch r.state {
                        case .running, .paused:
                            if let st = r.stats {
                                HStack(spacing: 18) {
                                    stat("Tested", Int(st.tested).formatted())
                                    stat("Speed", Int(st.perSecond).formatted() + "/s")
                                    stat("Elapsed", Format.duration(st.elapsed, precise: true))
                                    stat("Remaining", st.eta > 0 ? Format.duration(st.eta, precise: true) : "—")
                                }
                            } else {
                                Text("Recovering the password...").foregroundStyle(.secondary)
                            }
                            HStack {
                                Button(r.state == .paused ? L10n.t("Resume All") : L10n.t("Pause All")) {
                                    r.state == .paused ? r.resume() : r.pause()
                                }
                                Button("Cancel") { r.cancel() }
                            }
                        case .finished(let pw):
                            Label("Password found", systemImage: "checkmark.circle.fill")
                                .foregroundStyle(.green).font(.headline)
                            HStack {
                                Text(pw).font(.title3.monospaced()).textSelection(.enabled)
                                Button("Copy") { NSPasteboard.general.setString(pw, forType: .string) }
                                if let u = urls.first { Button("Open") { NSWorkspace.shared.open(u) } }
                            }
                        case .exhausted:
                            Label("Password not found in the search space", systemImage: "exclamationmark.circle")
                                .foregroundStyle(.orange).font(.headline)
                        case .cancelled:
                            Label("Cancelled", systemImage: "xmark.circle").font(.headline)
                        case .failed(let m):
                            Label(m, systemImage: "exclamationmark.triangle")
                                .foregroundStyle(.red).font(.callout)
                        case .idle:
                            Text("Press Start to begin.").foregroundStyle(.secondary)
                        }
                    } else {
                        Text("Recovering the password...").foregroundStyle(.secondary)
                    }
                }
                .padding(12)
                .card()
            case .result:
                if let r = runner, case .finished(let pw) = r.state {
                    VStack(alignment: .leading, spacing: 12) {
                        Label("The recovered password is...", systemImage: "key.fill").font(.headline)
                        Text(pw).font(.system(.title2, design: .monospaced)).textSelection(.enabled)
                        HStack {
                            Button("Copy") { NSPasteboard.general.setString(pw, forType: .string) }
                            if let u = urls.first { Button("Open") { NSWorkspace.shared.open(u) } }
                        }
                    }
                    .padding(12)
                    .card()
                } else if let r = runner {
                    Label(r.state == .exhausted ? L10n.t("Password not found in the search space") : L10n.t("Recovery ended"),
                          systemImage: "exclamationmark.circle")
                        .foregroundStyle(.orange).font(.headline)
                        .padding(12).card()
                } else {
                    Text("Run the recovery first.").foregroundStyle(.secondary)
                }
            }

            Spacer()
            HStack {
                Button("Back") { go(-1) }.disabled(step == .open)
                Spacer()
                if step == .run, let r = runner, !r.state.isTerminal {
                    EmptyView()
                } else if step == .run {
                    Button("START") { startRun() }
                        .keyboardShortcut(.defaultAction)
                        .buttonStyle(.borderedProminent)
                } else {
                    Button(step == .result ? L10n.t("Done") : L10n.t("Next")) { go(1) }
                        .keyboardShortcut(.defaultAction)
                        .disabled(step == .open && urls.isEmpty)
                }
            }
        }
    }

    private func startRun() {
        guard let url = urls.first, let exe = app.engineExecutableURL else { return }
        let r = RecoveryRunner(executableURL: exe)
        runner = r
        r.start(archive: url.path,
                charset: RecoveryRunner.charsetArguments(digits: digits, lower: lower, upper: upper,
                                                        special: special, space: space, custom: custom),
                prefix: prefix, minLength: Int(minLen), maxLength: Int(maxLen))
        Task { @MainActor in
            while let rr = runner, !rr.state.isTerminal {
                try? await Task.sleep(nanoseconds: 300_000_000)
            }
            if case .finished = runner?.state { step = .result }
        }
    }

    private func stat(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(L10n.t(label)).font(.caption2).foregroundStyle(.secondary)
            Text(value).font(.callout.monospacedDigit())
        }
    }

    private func go(_ delta: Int) {
        guard let next = Step(rawValue: step.rawValue + delta) else {
            step = .open
            return
        }
        step = next
        if next == .inspect, let url = urls.first {
            entries = []
            error = nil
            Task {
                do { entries = try await app.engine.listEntries(archive: url.path) }
                catch let e as FZError { error = e }
                catch { self.error = FZError(code: .io, message: error.localizedDescription) }
            }
        }
    }
}
