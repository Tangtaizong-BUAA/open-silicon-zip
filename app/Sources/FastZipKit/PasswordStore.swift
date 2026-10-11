import Foundation
import CryptoKit
#if canImport(Security)
import Security
#endif

public struct PasswordItem: Codable, Sendable, Identifiable, Equatable {
    public var id: UUID
    public var name: String
    public var secret: String
    public var createdAt: Date
    public init(id: UUID = UUID(), name: String, secret: String, createdAt: Date = Date()) {
        self.id = id
        self.name = name
        self.secret = secret
        self.createdAt = createdAt
    }
}

public enum PasswordStoreError: Error, Equatable, LocalizedError {
    case locked
    case wrongMasterPassword
    case corrupted
    case keychain(Int32)

    public var errorDescription: String? {
        switch self {
        case .locked: return L10n.t("The password list is locked.")
        case .wrongMasterPassword: return L10n.t("Wrong master password.")
        case .corrupted: return L10n.t("The password file is damaged.")
        case .keychain(let s): return L10n.f("Keychain error %@.", String(s))
        }
    }
}

/// How the data key is obtained (FR-603 / FR-606).
public enum PasswordStoreMode: String, Codable, Sendable {
    case masterPassword
    /// A random 256-bit key held in the macOS Keychain; no master password prompt.
    case keychain
}

/// Encrypted-at-rest list of named passwords (FR-602/603/604/606).
///
/// File format (JSON): `{version, mode, kdf, iterations, salt, sealed}` where `sealed`
/// is an AES-256-GCM combined box (nonce || ciphertext || tag) of the JSON item list.
///
/// KDF: PBKDF2-HMAC-SHA256 implemented over CryptoKit's HMAC as a stand-in.
/// The target is Argon2id (memory-hard); swap `deriveKey` and bump `version`
/// when the Argon2 library is vendored. Decoding keeps working for old versions
/// because the KDF name is stored in the file.
public actor PasswordStore {
    struct Envelope: Codable {
        var version: Int
        var mode: PasswordStoreMode
        var kdf: String
        var iterations: Int
        var salt: Data
        var sealed: Data
    }

    public static let keychainService = "org.opensiliconzip.mac.passwordstore"
    public static let iterations = 210_000

    private let fileURL: URL
    private var key: SymmetricKey?
    private var items: [PasswordItem] = []
    private var envelopeCache: Envelope?

    public init(fileURL: URL = PasswordStore.defaultFileURL) {
        self.fileURL = fileURL
    }

    public static var defaultFileURL: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory())
        return base.appendingPathComponent("OpenSiliconZip", isDirectory: true).appendingPathComponent("passwords.fzpw")
    }

    // MARK: State

    public var exists: Bool { FileManager.default.fileExists(atPath: fileURL.path) }
    public var isUnlocked: Bool { key != nil }

    public func mode() -> PasswordStoreMode? {
        guard let data = try? Data(contentsOf: fileURL),
              let env = try? JSONDecoder().decode(Envelope.self, from: data) else { return nil }
        return env.mode
    }

    // MARK: Lifecycle

    /// Creates a new empty store protected by a master password.
    public func create(masterPassword: String, iterations: Int = PasswordStore.iterations) throws {
        guard (1...2_000_000).contains(iterations) else { throw PasswordStoreError.corrupted }
        let salt = Self.randomBytes(16)
        key = Self.deriveKey(password: masterPassword, salt: salt, iterations: iterations)
        items = []
        try persist(mode: .masterPassword, salt: salt, iterations: iterations)
    }

    /// Creates a new empty store whose key lives in the Keychain.
    public func createKeychainBacked() throws {
        let raw = Self.randomBytes(32)
        try Self.keychainStore(raw)
        key = SymmetricKey(data: raw)
        items = []
        try persist(mode: .keychain, salt: Self.randomBytes(16), iterations: 0)
    }

    public func unlock(masterPassword: String) throws {
        let env = try loadEnvelope()
        guard env.mode == .masterPassword else { throw PasswordStoreError.corrupted }
        let k = Self.deriveKey(password: masterPassword, salt: env.salt, iterations: env.iterations)
        try openBox(env, with: k)
    }

    public func unlockWithKeychain() throws {
        let env = try loadEnvelope()
        guard env.mode == .keychain else { throw PasswordStoreError.corrupted }
        let raw = try Self.keychainLoad()
        try openBox(env, with: SymmetricKey(data: raw))
    }

    public func lock() {
        key = nil
        items = []
    }

    /// FR-603: reset wipes the store (caller must confirm with the user first).
    public func reset() {
        try? FileManager.default.removeItem(at: fileURL)
        #if canImport(Security)
        Self.keychainDelete()
        #endif
        key = nil
        items = []
        envelopeCache = nil
    }

    // MARK: Items

    public func all() throws -> [PasswordItem] {
        guard key != nil else { throw PasswordStoreError.locked }
        return items.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    public func item(id: UUID) throws -> PasswordItem? {
        try all().first { $0.id == id }
    }

    public func upsert(_ item: PasswordItem) throws {
        guard key != nil else { throw PasswordStoreError.locked }
        if let i = items.firstIndex(where: { $0.id == item.id }) { items[i] = item } else { items.append(item) }
        try rewrite()
    }

    public func delete(id: UUID) throws {
        guard key != nil else { throw PasswordStoreError.locked }
        items.removeAll { $0.id == id }
        try rewrite()
    }

    /// FR-604 auto-password: secrets to try in order (most recently created first).
    public func autoPasswordCandidates() throws -> [String] {
        try all().sorted { $0.createdAt > $1.createdAt }.map(\.secret)
    }

    // MARK: Persistence

    private func loadEnvelope() throws -> Envelope {
        guard let data = try? Data(contentsOf: fileURL) else { throw PasswordStoreError.corrupted }
        guard let env = try? JSONDecoder().decode(Envelope.self, from: data) else { throw PasswordStoreError.corrupted }
        guard env.version == 1, env.kdf == "pbkdf2-hmac-sha256", env.salt.count == 16,
              env.sealed.count >= 28, env.sealed.count <= 16 * 1024 * 1024,
              (env.mode == .keychain ? env.iterations == 0 : (1...2_000_000).contains(env.iterations)) else {
            throw PasswordStoreError.corrupted
        }
        envelopeCache = env
        return env
    }

    private func openBox(_ env: Envelope, with k: SymmetricKey) throws {
        do {
            let box = try AES.GCM.SealedBox(combined: env.sealed)
            let plain = try AES.GCM.open(box, using: k)
            let decoder = JSONDecoder()
            decoder.dateDecodingStrategy = .iso8601
            items = try decoder.decode([PasswordItem].self, from: plain)
            key = k
        } catch {
            throw env.mode == .masterPassword ? PasswordStoreError.wrongMasterPassword : PasswordStoreError.corrupted
        }
    }

    private func rewrite() throws {
        guard let env = envelopeCache else { throw PasswordStoreError.corrupted }
        try persist(mode: env.mode, salt: env.salt, iterations: env.iterations)
    }

    private func persist(mode: PasswordStoreMode, salt: Data, iterations: Int) throws {
        guard let key else { throw PasswordStoreError.locked }
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        let plain = try encoder.encode(items)
        let box = try AES.GCM.seal(plain, using: key)
        guard let combined = box.combined else { throw PasswordStoreError.corrupted }
        let env = Envelope(version: 1, mode: mode, kdf: "pbkdf2-hmac-sha256", iterations: iterations,
                           salt: salt, sealed: combined)
        let out = JSONEncoder()
        out.outputFormatting = [.prettyPrinted, .sortedKeys]
        try FileManager.default.createDirectory(at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try out.encode(env).write(to: fileURL, options: .atomic)
        envelopeCache = env
    }

    // MARK: Crypto helpers

    static func randomBytes(_ n: Int) -> Data {
        Data((0..<n).map { _ in UInt8.random(in: 0...255) })   // SystemRandomNumberGenerator is CSPRNG-backed
    }

    /// PBKDF2-HMAC-SHA256, 32-byte output (single block). Stand-in for Argon2id.
    static func deriveKey(password: String, salt: Data, iterations: Int) -> SymmetricKey {
        let pk = SymmetricKey(data: Data(password.utf8))
        var block = salt
        block.append(contentsOf: [0, 0, 0, 1])
        var u = Data(HMAC<SHA256>.authenticationCode(for: block, using: pk))
        var t = [UInt8](u)
        if iterations > 1 {
            for _ in 2...iterations {
                u = Data(HMAC<SHA256>.authenticationCode(for: u, using: pk))
                var i = 0
                for b in u {
                    t[i] ^= b
                    i += 1
                }
            }
        }
        return SymmetricKey(data: Data(t))
    }

    // MARK: Keychain (FR-606)

    static func keychainStore(_ raw: Data) throws {
        #if canImport(Security)
        keychainDelete()
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: "data-key",
            kSecValueData as String: raw,
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else { throw PasswordStoreError.keychain(status) }
        #else
        throw PasswordStoreError.keychain(-1)
        #endif
    }

    static func keychainLoad() throws -> Data {
        #if canImport(Security)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: "data-key",
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else { throw PasswordStoreError.keychain(status) }
        return data
        #else
        throw PasswordStoreError.keychain(-1)
        #endif
    }

    #if canImport(Security)
    static func keychainDelete() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: "data-key",
        ]
        SecItemDelete(query as CFDictionary)
    }
    #endif
}
