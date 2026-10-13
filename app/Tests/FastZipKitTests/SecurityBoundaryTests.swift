import XCTest
import Darwin
@testable import FastZipKit

final class SecurityBoundaryTests: XCTestCase {
    private func directory() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    func testImportedSettingsCannotChooseAnExecutable() throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = PreferencesStore(fileURL: dir.appendingPathComponent("local.json"))
        var local = Preferences()
        local.advanced.enginePath = "/trusted/fz"
        try store.save(local)
        var imported = Preferences()
        imported.advanced.enginePath = "/attacker/program"
        imported.compression.excludeGlobs = ["*.tmp"]
        let input = dir.appendingPathComponent("import.json")
        try PreferencesStore.encode(imported).write(to: input)
        XCTAssertEqual(try store.importFile(input).advanced.enginePath, "/trusted/fz")
        XCTAssertEqual(store.load().advanced.enginePath, "/trusted/fz")
        XCTAssertEqual(store.load().compression.excludeGlobs, ["*.tmp"])
        let fresh = PreferencesStore(fileURL: dir.appendingPathComponent("fresh.json"))
        XCTAssertNil(try fresh.importFile(input).advanced.enginePath)
    }

    func testExternalPathsCannotEscapeConfiguredRootsThroughLinksOrPrefixMatches() throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let allowed = dir.appendingPathComponent("allowed")
        let outside = dir.appendingPathComponent("allowed-other")
        try FileManager.default.createDirectory(at: allowed, withIntermediateDirectories: false)
        try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: false)
        try FileManager.default.createSymbolicLink(at: allowed.appendingPathComponent("escape"), withDestinationURL: outside)
        var config = FinderConfig()
        config.accessibleFolders = [allowed.path]
        XCTAssertNoThrow(try ExternalRequestPolicy.validate([allowed.appendingPathComponent("file").path], config: config))
        XCTAssertThrowsError(try ExternalRequestPolicy.validate([outside.appendingPathComponent("secret").path], config: config))
        XCTAssertThrowsError(try ExternalRequestPolicy.validate([allowed.appendingPathComponent("escape/secret").path], config: config))
    }

    func testMixedInputOrderCannotChooseCompressionDestination() {
        let paths = ["/attacker/drop/item", "/protected/documents/secret"]
        XCTAssertNil(ExternalRequestPolicy.commonSourceDirectory(paths))
        XCTAssertNil(ExternalRequestPolicy.commonSourceDirectory(Array(paths.reversed())))
        XCTAssertEqual(ExternalRequestPolicy.commonSourceDirectory(["/one/a", "/one/b"])?.path, "/one")
    }

    func testExternalDeleteFalseOverridesDestructivePreset() {
        var preset = CreateSettings()
        preset.options.deleteSources = true
        XCTAssertFalse(ExternalRequestPolicy.compressionSettings(preset, deleteSources: false).options.deleteSources)
        preset.options.deleteSources = false
        XCTAssertTrue(ExternalRequestPolicy.compressionSettings(preset, deleteSources: true).options.deleteSources)
    }

    func testDeepArchivePathsAreRejectedBeforeIndexExpansion() throws {
        let path = Array(repeating: "a", count: 30_000).joined(separator: "/")
        XCTAssertThrowsError(try ArchiveIndex(entries: [ArchiveEntry(name: path)]))
        let index = try ArchiveIndex(entries: [ArchiveEntry(name: "one/two/file", size: 12, compressedSize: 8)])
        XCTAssertEqual(index.totalSize, 12)
        XCTAssertEqual(index.items(in: "one").first?.size, 12)
        XCTAssertEqual(index.root.first?.children?.first?.name, "two")
    }

    func testZip64SizeOverflowThrowsInsteadOfTrapping() {
        XCTAssertThrowsError(try ArchiveIndex(entries: [
            ArchiveEntry(name: "folder/a", size: .max), ArchiveEntry(name: "folder/b", size: 1),
        ]))
        XCTAssertThrowsError(try ArchiveIndex(entries: [
            ArchiveEntry(name: "a", compressedSize: .max), ArchiveEntry(name: "b", compressedSize: 1),
        ]))
    }

    func testMetadataCollectionCountIsBounded() {
        let entries = Array(repeating: ArchiveEntry(name: "a"), count: ArchiveListingBudget.maximumEntries + 1)
        XCTAssertThrowsError(try ArchiveIndex(entries: entries))
    }

    @MainActor
    func testTemporaryPlaintextPermissionsAndExplicitRelease() throws {
        let parent = try directory()
        defer { try? FileManager.default.removeItem(at: parent) }
        let store = try TemporaryExtractionStore(parent: parent)
        let temporary = try store.createDirectory()
        let file = temporary.appendingPathComponent("private.txt")
        try Data("plaintext".utf8).write(to: file)
        let attributes = try FileManager.default.attributesOfItem(atPath: temporary.path)
        XCTAssertEqual((attributes[.posixPermissions] as? NSNumber)?.intValue, 0o700)
        store.removeFile(file)
        XCTAssertFalse(FileManager.default.fileExists(atPath: temporary.path))
        let second = try store.createDirectory()
        store.cleanUp()
        XCTAssertFalse(FileManager.default.fileExists(atPath: second.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: store.root.path))
        XCTAssertThrowsError(try store.createDirectory())
    }

    @MainActor
    func testCleanupReclaimsAbandonedSessionAndPreservesActiveSession() throws {
        let parent = try directory()
        defer { try? FileManager.default.removeItem(at: parent) }
        let abandoned = parent.appendingPathComponent("OpenSiliconZip-Plaintext-ABC123")
        try FileManager.default.createDirectory(at: abandoned, withIntermediateDirectories: false)
        try Data().write(to: abandoned.appendingPathComponent(".lease"))
        try Data("secret".utf8).write(to: abandoned.appendingPathComponent("plaintext"))
        let active = try TemporaryExtractionStore(parent: parent)
        defer { active.cleanUp() }
        XCTAssertFalse(FileManager.default.fileExists(atPath: abandoned.path))
        let other = try TemporaryExtractionStore(parent: parent)
        defer { other.cleanUp() }
        XCTAssertTrue(FileManager.default.fileExists(atPath: active.root.path))
        let target = try directory()
        defer { try? FileManager.default.removeItem(at: target) }
        let fake = parent.appendingPathComponent("OpenSiliconZip-Plaintext-DEF456")
        try FileManager.default.createSymbolicLink(at: fake, withDestinationURL: target)
        let third = try TemporaryExtractionStore(parent: parent)
        defer { third.cleanUp() }
        XCTAssertTrue(FileManager.default.fileExists(atPath: target.path))
    }

    func testCancellationCanWaitForAnUncooperativeChildToExit() async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let executable = dir.appendingPathComponent("fz")
        try "#!/bin/sh\ntrap '' TERM\nprintf '{\"event\":\"progress\"}\\n'\nwhile :; do :; done\n"
            .write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
        let engine = ProcessEngineClient(executableURL: executable)
        let run = engine.start(.test(archive: "/synthetic", password: nil))
        for try await _ in run.events { break }
        run.cancel()
        await run.waitForTermination()
        XCTAssertFalse(EngineProcesses.isBusy)
    }

    func testProcessRejectsOversizedListingBeforeYieldingEntry() async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let executable = dir.appendingPathComponent("fz")
        let path = Array(repeating: "a", count: 65).joined(separator: "/")
        let script = "#!/bin/sh\nprintf '%s\\n' '{\"event\":\"entry\",\"name\":\"\(path)\"}' '{\"event\":\"done\",\"ok\":true}'\n"
        try script.write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
        let run = ProcessEngineClient(executableURL: executable).start(.list(archive: "/synthetic", password: nil, codepage: nil))
        do {
            for try await event in run.events {
                if case .entry = event { XCTFail("Producer admitted an unsafe entry") }
            }
            XCTFail("Listing should fail")
        } catch let error as FZError {
            XCTAssertEqual(error.code, .resourceLimit)
        }
        await run.waitForTermination()
    }
}
