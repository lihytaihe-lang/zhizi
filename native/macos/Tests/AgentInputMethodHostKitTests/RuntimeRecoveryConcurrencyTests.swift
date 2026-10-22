import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class RuntimeRecoveryConcurrencyTests: XCTestCase {
    func testOlderStopCannotUnlinkMarkerPublishedByOverlappingNewGeneration() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-marker-lock-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let oldStopLocked = DispatchSemaphore(value: 0)
        let allowOldStop = DispatchSemaphore(value: 0)
        let oldStopFinished = DispatchSemaphore(value: 0)
        let newStartLocked = DispatchSemaphore(value: 0)
        let newStartFinished = DispatchSemaphore(value: 0)

        let oldMarker = RuntimeRecoveryMarker(root: root) { phase in
            guard phase == .stopLockedBeforeUnlink else { return }
            oldStopLocked.signal()
            _ = allowOldStop.wait(timeout: .now() + 5)
        }
        let newMarker = RuntimeRecoveryMarker(root: root) { phase in
            guard phase == .startLocked else { return }
            newStartLocked.signal()
        }
        try oldMarker.start(attestation: fixture(
            generation: "11111111-1111-4111-8111-111111111111"
        ))

        let oldBox = UnsafeSendableBox(oldMarker)
        DispatchQueue.global(qos: .userInitiated).async {
            oldBox.value.stop()
            oldStopFinished.signal()
        }
        XCTAssertEqual(oldStopLocked.wait(timeout: .now() + 2), .success)

        let newBox = UnsafeSendableBox(newMarker)
        let startResult = LockedResultBox<Bool>()
        let newAttestation = fixture(
            generation: "22222222-2222-4222-8222-222222222222"
        )
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                startResult.set(.success(try newBox.value.start(attestation: newAttestation)))
            } catch {
                startResult.set(.failure(error))
            }
            newStartFinished.signal()
        }

        // The new publisher must still be waiting on the old stop's stable
        // inode lock. Without the lock it reaches this hook and recreates the
        // exact check-to-unlink race reported by the security review.
        XCTAssertEqual(newStartLocked.wait(timeout: .now() + 0.2), .timedOut)
        allowOldStop.signal()
        XCTAssertEqual(oldStopFinished.wait(timeout: .now() + 2), .success)
        XCTAssertEqual(newStartLocked.wait(timeout: .now() + 2), .success)
        XCTAssertEqual(newStartFinished.wait(timeout: .now() + 2), .success)

        XCTAssertEqual(try startResult.get().get(), false)
        let marker = root.appendingPathComponent("Runtime/running.marker")
        let contents = try String(contentsOf: marker, encoding: .utf8)
        XCTAssertTrue(contents.contains("22222222-2222-4222-8222-222222222222"))
        newMarker.stop()
        XCTAssertFalse(FileManager.default.fileExists(atPath: marker.path))
    }

    private func fixture(generation: String) -> RuntimeArtifactAttestation {
        RuntimeArtifactAttestation(
            formatVersion: 1,
            generation: generation,
            processID: 1,
            launchedAt: "2026-07-13T00:00:00.000Z",
            bundleChecksum: String(repeating: "a", count: 64),
            executableChecksum: String(repeating: "b", count: 64),
            bundleIdentifier: "com.hongye.facai.inputmethod.zhizi",
            version: "0.3.0",
            build: "1024",
            productCommit: String(repeating: "c", count: 40),
            productInputsSHA256: String(repeating: "d", count: 64)
        )
    }
}

private final class UnsafeSendableBox<Value>: @unchecked Sendable {
    let value: Value
    init(_ value: Value) { self.value = value }
}

private final class LockedResultBox<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Result<Value, Error>?

    func set(_ value: Result<Value, Error>) {
        lock.lock()
        self.value = value
        lock.unlock()
    }

    func get() throws -> Result<Value, Error> {
        lock.lock()
        defer { lock.unlock() }
        guard let value else { throw MissingResult() }
        return value
    }

    private struct MissingResult: Error {}
}
