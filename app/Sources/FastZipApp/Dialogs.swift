import SwiftUI
import AppKit
import FastZipKit

// MARK: - Password prompt (FR-601)

struct PasswordAnswer {
    var password: String
    var applyToAll: Bool
    var save: Bool
}

struct PasswordPromptView: View {
    let archiveName: String
    let isRetry: Bool
    let stored: [PasswordItem]
    let onResult: (PasswordAnswer?) -> Void

    @State private var password = ""
    @State private var applyToAll = false
    @State private var save = false
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Label(isRetry ? L10n.t("Wrong password") : L10n.t("Password required"), systemImage: "lock")
                    .font(.title3.weight(.semibold))
                Text(isRetry ? L10n.f("That password did not work for \"%@\". Try again.", archiveName)
                             : L10n.f("\"%@\" is encrypted.", archiveName))
                    .foregroundStyle(.secondary)
            }
            RevealableSecureField(title: L10n.t("Password"), text: $password)
                .focused($focused)
                .onSubmit(submit)
            if !stored.isEmpty {
                Menu("Use a saved password") {
                    ForEach(stored) { item in
                        Button(item.name) { password = item.secret }
                    }
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
            }
            Toggle("Apply to all archives in this job", isOn: $applyToAll)
            Toggle("Save to password manager", isOn: $save)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { onResult(nil) }
                    .keyboardShortcut(.cancelAction)
                Button("Continue", action: submit)
                    .keyboardShortcut(.defaultAction)
                    .disabled(password.isEmpty)
            }
        }
        .padding(Theme.pad)
        .frame(width: 400)
        .onAppear { focused = true }
    }

    private func submit() {
        guard !password.isEmpty else { return }
        onResult(PasswordAnswer(password: password, applyToAll: applyToAll, save: save))
    }
}

// MARK: - Conflict dialog (FR-211)

struct ConflictInfo {
    var name: String
    var existingSize: UInt64?
    var existingDate: Date?
    var incomingSize: UInt64?
    var incomingDate: Date?
    var remaining: Int = 0
}

struct ConflictAnswer {
    var policy: ConflictPolicy      // .skip / .overwrite / .rename (never .ask)
    var applyToAll: Bool
    var cancelled = false
}

struct ConflictDialogView: View {
    let info: ConflictInfo
    let onResult: (ConflictAnswer) -> Void
    @State private var applyToAll = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Label(L10n.f("\"%@\" already exists", info.name), systemImage: "exclamationmark.triangle")
                .font(.title3.weight(.semibold))
            Text("Choose what to do. Nothing is merged silently.")
                .foregroundStyle(.secondary)
            Grid(alignment: .leading, horizontalSpacing: 20, verticalSpacing: 6) {
                GridRow {
                    Text("").gridColumnAlignment(.leading)
                    Text("Existing").font(.caption).foregroundStyle(.secondary)
                    Text("Incoming").font(.caption).foregroundStyle(.secondary)
                }
                GridRow {
                    Text("Size").foregroundStyle(.secondary)
                    Text(info.existingSize.map { Format.bytes($0) } ?? "-")
                    Text(info.incomingSize.map { Format.bytes($0) } ?? "-")
                }
                GridRow {
                    Text("Modified").foregroundStyle(.secondary)
                    Text(info.existingDate.map { $0.formatted(date: .abbreviated, time: .shortened) } ?? "-")
                    Text(info.incomingDate.map { $0.formatted(date: .abbreviated, time: .shortened) } ?? "-")
                }
            }
            .padding(12)
            .card()
            .accessibilityElement(children: .combine)
            Toggle(info.remaining > 0 ? L10n.f("Apply to all (%ld more)", info.remaining) : L10n.t("Apply to all"), isOn: $applyToAll)
            HStack {
                Button("Cancel", role: .cancel) {
                    onResult(ConflictAnswer(policy: .skip, applyToAll: true, cancelled: true))
                }
                .keyboardShortcut(.cancelAction)
                Spacer()
                Button("Skip") { onResult(ConflictAnswer(policy: .skip, applyToAll: applyToAll)) }
                Button("Keep Both") { onResult(ConflictAnswer(policy: .rename, applyToAll: applyToAll)) }
                Button("Overwrite", role: .destructive) {
                    onResult(ConflictAnswer(policy: .overwrite, applyToAll: applyToAll))
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(Theme.pad)
        .frame(width: 440)
    }
}

// MARK: - Error presentation (FR-1201..1203)

enum ErrorChoice: Equatable {
    case retry, skip, abort, enterPassword, locateVolume, chooseDestination, repair, dismiss
}

struct ErrorSheetView: View {
    let error: FZError
    let context: String?
    let batch: Bool                       // show Retry / Skip / Abort
    let onResult: (ErrorChoice) -> Void
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: icon)
                    .font(.system(size: 26, weight: .light))
                    .foregroundStyle(error.code.kind == .cancelled ? Color.secondary : Color.orange)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 4) {
                    Text(error.code.title).font(.title3.weight(.semibold))
                    if let context { Text(context).font(.callout).foregroundStyle(.secondary) }
                }
            }
            Text(error.code.guidance)
            if !error.message.isEmpty {
                Text(error.message)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .card()
            }
            HStack {
                Button(copied ? L10n.t("Copied") : L10n.t("Copy Diagnostics")) {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(error.diagnostics, forType: .string)
                    copied = true
                }
                .accessibilityLabel("Copy diagnostics to clipboard")
                Spacer()
                ForEach(actions, id: \.title) { a in
                    Button(L10n.t(a.title)) { onResult(a.choice) }
                }
                Button(batch ? L10n.t("Abort") : L10n.t("OK")) { onResult(batch ? .abort : .dismiss) }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(Theme.pad)
        .frame(width: 460)
    }

    private var icon: String {
        switch error.code.kind {
        case .password: return "lock.trianglebadge.exclamationmark"
        case .volumeNeeded: return "square.stack.3d.up"
        case .broken: return "bandage"
        case .cancelled: return "xmark.circle"
        default: return "exclamationmark.triangle"
        }
    }

    private struct Action { var title: String; var choice: ErrorChoice }

    private var actions: [Action] {
        var out: [Action] = []
        for r in error.code.recoveries {
            switch r {
            case .enterPassword: out.append(Action(title: L10n.t("Enter Password"), choice: .enterPassword))
            case .locateVolume: out.append(Action(title: L10n.t("Locate Volume"), choice: .locateVolume))
            case .chooseDestination: out.append(Action(title: L10n.t("Choose Destination"), choice: .chooseDestination))
            case .repair: out.append(Action(title: L10n.t("Repair"), choice: .repair))
            case .retry: out.append(Action(title: L10n.t("Retry"), choice: .retry))
            case .skip where batch: out.append(Action(title: L10n.t("Skip"), choice: .skip))
            default: break
            }
        }
        return out
    }
}

@MainActor
enum Dialogs {
    /// Plain confirmation sheet on the key window (used by archive-edit actions).
    static func confirm(title: String, message: String) async -> Bool {
        await withCheckedContinuation { (c: CheckedContinuation<Bool, Never>) in
            let a = NSAlert()
            a.messageText = title
            a.informativeText = message
            a.alertStyle = .warning
            a.addButton(withTitle: NSLocalizedString("Delete", comment: ""))
            a.addButton(withTitle: NSLocalizedString("Cancel", comment: ""))
            let resume: (NSApplication.ModalResponse) -> Void = { c.resume(returning: $0 == .alertFirstButtonReturn) }
            if let w = NSApp.keyWindow {
                a.beginSheetModal(for: w) { resume($0) }
            } else {
                resume(a.runModal())
            }
        }
    }

    /// Single-field text input sheet (archive-entry rename).
    static func askText(title: String, message: String, initial: String) async -> String? {
        await withCheckedContinuation { (c: CheckedContinuation<String?, Never>) in
            let a = NSAlert()
            a.messageText = title
            a.informativeText = message
            let f = NSTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 24))
            f.stringValue = initial
            a.accessoryView = f
            a.window.initialFirstResponder = f
            a.addButton(withTitle: NSLocalizedString("Rename", comment: ""))
            a.addButton(withTitle: NSLocalizedString("Cancel", comment: ""))
            let resume: (NSApplication.ModalResponse) -> Void = { r in
                c.resume(returning: r == .alertFirstButtonReturn ? (f.stringValue.isEmpty ? nil : f.stringValue) : nil)
            }
            if let w = NSApp.keyWindow {
                a.beginSheetModal(for: w) { resume($0) }
            } else {
                resume(a.runModal())
            }
        }
    }

    static func askPassword(archive: String, retry: Bool, stored: [PasswordItem]) async -> PasswordAnswer? {
        await Presenter.present(defaultValue: nil as PasswordAnswer?) { done in
            PasswordPromptView(archiveName: archive, isRetry: retry, stored: stored, onResult: done)
        }
    }

    static func resolveConflict(_ info: ConflictInfo) async -> ConflictAnswer {
        await Presenter.present(defaultValue: ConflictAnswer(policy: .skip, applyToAll: true, cancelled: true)) { done in
            ConflictDialogView(info: info, onResult: done)
        }
    }

    @discardableResult
    static func showError(_ error: FZError, context: String? = nil, batch: Bool = false) async -> ErrorChoice {
        await Presenter.present(defaultValue: ErrorChoice.dismiss) { done in
            ErrorSheetView(error: error, context: context, batch: batch, onResult: done)
        }
    }
}
