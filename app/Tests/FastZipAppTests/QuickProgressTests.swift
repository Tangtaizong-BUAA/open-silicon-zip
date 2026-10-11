import XCTest
import FastZipKit
@testable import FastZipApp

final class QuickProgressTests: XCTestCase {
    @MainActor func testPreparationDoesNotDismissAndFailureStaysVisible() async throws {
        let state = QuickProgress(title: "test")
        var closed = false
        var attention = false
        state.onFinished = { closed = true }
        state.onNeedsAttention = { attention = true }
        state.update([])
        try await Task.sleep(for: .seconds(1))
        XCTAssertFalse(closed)
        state.finish([], error: "cannot read archive")
        try await Task.sleep(for: .seconds(1))
        XCTAssertFalse(closed)
        XCTAssertTrue(attention)
    }

    @MainActor func testBatchWaitsForEveryTrackedJob() async throws {
        let state = QuickProgress(title: "batch")
        var first = Job(title: "one", command: .test(archive: "one.zip", password: nil))
        var second = Job(title: "two", command: .test(archive: "two.zip", password: nil))
        state.ids = [first.id, second.id]
        var closed = false
        state.onFinished = { closed = true }
        first.state = .success
        second.state = .running
        state.finish([first, second])
        try await Task.sleep(for: .seconds(1))
        XCTAssertFalse(closed)
        second.state = .success
        state.update([first, second])
        try await Task.sleep(for: .seconds(1))
        XCTAssertTrue(closed)
    }
}
