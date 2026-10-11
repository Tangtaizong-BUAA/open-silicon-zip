import SwiftUI
import AppKit
import FastZipKit

/// Password manager (FR-602/603/606). The store is an actor; every call is awaited off the main thread.
struct PasswordManagerView: View {
    @EnvironmentObject private var app: AppModel

    private enum Phase { case loading, create, locked, unlocked }

    @State private var phase: Phase = .loading
    @State private var items: [PasswordItem] = []
    @State private var master = ""
    @State private var confirm = ""
    @State private var useKeychain = false
    @State private var error: String?
    @State private var editing: PasswordItem?
    @State private var confirmReset = false
    @State private var busy = false

    var body: some View {
        Group {
            switch phase {
            case .loading: ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            case .create: createView
            case .locked: lockedView
            case .unlocked: listView
            }
        }
        .task { await refreshPhase() }
        .frame(minWidth: 460, minHeight: 360)
    }

    // MARK: Phases

    private var createView: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Protect your saved passwords", systemImage: "lock.shield").font(.title3.weight(.semibold))
            Text("Choose how the list is encrypted on this Mac.").foregroundStyle(.secondary)
            Picker("Protection", selection: $useKeychain) {
                Text("Master password").tag(false)
                Text("macOS Keychain").tag(true)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .accessibilityLabel("Protection method")
            if !useKeychain {
                RevealableSecureField(title: L10n.t("Master password"), text: $master)
                RevealableSecureField(title: L10n.t("Confirm master password"), text: $confirm)
                Text("If you forget it, the only way back in is to reset, which erases the list.")
                    .font(.caption).foregroundStyle(.secondary)
            } else {
                Text("A random key is kept in your Keychain. No password to remember, but the list is tied to this Mac and user.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if let error { Text(error).foregroundStyle(.red).font(.caption) }
            HStack {
                Spacer()
                Button("Create") { create() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(busy || (!useKeychain && (master.count < 8 || master != confirm)))
            }
        }
        .padding(Theme.pad)
    }

    private var lockedView: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Passwords are locked", systemImage: "lock").font(.title3.weight(.semibold))
            RevealableSecureField(title: L10n.t("Master password"), text: $master).onSubmit { unlock() }
            if let error { Text(error).foregroundStyle(.red).font(.caption) }
            HStack {
                Button("Reset...", role: .destructive) { confirmReset = true }
                Spacer()
                Button("Unlock") { unlock() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(busy || master.isEmpty)
            }
        }
        .padding(Theme.pad)
        .confirmationDialog("Erase all saved passwords?", isPresented: $confirmReset) {
            Button("Erase and Reset", role: .destructive) { reset() }
        } message: {
            Text("This permanently deletes the encrypted list. It cannot be undone.")
        }
    }

    private var listView: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Passwords").font(.title2.weight(.semibold))
                Spacer()
                Button { editing = PasswordItem(name: "", secret: "") } label: { Label("Add", systemImage: "plus") }
                    .accessibilityLabel("Add password")
                Button("Lock") { lock() }
            }
            .padding(16)
            Divider()
            if items.isEmpty {
                Text("No saved passwords").foregroundStyle(.secondary).frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List {
                    ForEach(items) { item in
                        HStack {
                            Image(systemName: "key").foregroundStyle(.secondary)
                            VStack(alignment: .leading) {
                                Text(item.name)
                                Text(String(repeating: "\u{2022}", count: min(12, max(4, item.secret.count))))
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Copy") {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(item.secret, forType: .string)
                            }
                            .controlSize(.small)
                            .accessibilityLabel(L10n.f("Copy password %@", item.name))
                            Button("Edit") { editing = item }.controlSize(.small)
                            Button(role: .destructive) { delete(item) } label: { Image(systemName: "trash") }
                                .buttonStyle(.borderless)
                                .accessibilityLabel(L10n.f("Delete %@", item.name))
                        }
                    }
                }
            }
        }
        .sheet(item: $editing) { item in
            PasswordEditor(item: item) { saved in
                editing = nil
                if let saved { upsert(saved) }
            }
        }
    }

    // MARK: Actions

    private func refreshPhase() async {
        if await app.passwordStore.isUnlocked {
            await reload()
        } else if await app.passwordStore.exists {
            if await app.passwordStore.mode() == .keychain {
                do { try await app.passwordStore.unlockWithKeychain(); await reload() }
                catch { phase = .locked; self.error = L10n.t("Could not read the Keychain key.") }
            } else {
                phase = .locked
            }
        } else {
            phase = .create
        }
    }

    private func reload() async {
        items = (try? await app.passwordStore.all()) ?? []
        phase = .unlocked
    }

    private func create() {
        busy = true
        let m = master
        let kc = useKeychain
        Task {
            defer { busy = false }
            do {
                if kc { try await app.passwordStore.createKeychainBacked() }
                else { try await app.passwordStore.create(masterPassword: m) }
                master = ""; confirm = ""; error = nil
                await reload()
            } catch { self.error = error.localizedDescription }
        }
    }

    private func unlock() {
        busy = true
        let m = master
        Task {
            defer { busy = false }
            do {
                try await app.passwordStore.unlock(masterPassword: m)
                master = ""; error = nil
                await reload()
            } catch { self.error = error.localizedDescription }
        }
    }

    private func lock() {
        Task {
            await app.passwordStore.lock()
            items = []
            phase = .locked
        }
    }

    private func reset() {
        Task {
            await app.passwordStore.reset()
            items = []; error = nil; master = ""
            phase = .create
        }
    }

    private func upsert(_ item: PasswordItem) {
        Task {
            do { try await app.passwordStore.upsert(item) } catch { self.error = error.localizedDescription }
            await reload()
        }
    }

    private func delete(_ item: PasswordItem) {
        Task {
            try? await app.passwordStore.delete(id: item.id)
            await reload()
        }
    }
}

private struct PasswordEditor: View {
    @State var item: PasswordItem
    let onDone: (PasswordItem?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(item.name.isEmpty ? L10n.t("New Password") : L10n.t("Edit Password")).font(.title3.weight(.semibold))
            TextField("Name (for example the archive or project)", text: $item.name)
                .textFieldStyle(.roundedBorder)
                .accessibilityLabel("Name")
            RevealableSecureField(title: L10n.t("Password"), text: $item.secret)
            HStack {
                Spacer()
                Button("Cancel") { onDone(nil) }.keyboardShortcut(.cancelAction)
                Button("Save") { onDone(item) }
                    .keyboardShortcut(.defaultAction)
                    .disabled(item.name.trimmingCharacters(in: .whitespaces).isEmpty || item.secret.isEmpty)
            }
        }
        .padding(Theme.pad)
        .frame(width: 400)
    }
}
