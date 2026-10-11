import Foundation
import CryptoKit
import Darwin

// Private key material is never printed. This helper does not use Keychain.
do {
    let args = CommandLine.arguments
    guard args.count >= 3 else { throw Failure("Usage: update-key.swift create KEY_FILE | verify PUBLIC_KEY FILE SIGNATURE") }
    if args[1] == "create" {
        let url = URL(fileURLWithPath: args[2]).standardizedFileURL
        guard !FileManager.default.fileExists(atPath: url.path) else { throw Failure("Key already exists; refusing to replace it") }
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let key = Curve25519.Signing.PrivateKey()
        let fd = open(url.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o600)
        guard fd >= 0 else { throw Failure("Cannot create private key file") }
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        try handle.write(contentsOf: Data((key.rawRepresentation.base64EncodedString() + "\n").utf8))
        try handle.synchronize()
        try handle.close()
        print(key.publicKey.rawRepresentation.base64EncodedString())
    } else if args[1] == "verify", args.count == 5,
              let rawKey = Data(base64Encoded: args[2]), let signature = Data(base64Encoded: args[4]) {
        let key = try Curve25519.Signing.PublicKey(rawRepresentation: rawKey)
        guard key.isValidSignature(signature, for: try Data(contentsOf: URL(fileURLWithPath: args[3]))) else { throw Failure("Invalid signature") }
    } else { throw Failure("Invalid arguments") }
} catch { fputs("\(error)\n", stderr); exit(1) }

struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ message: String) { description = message }
}
