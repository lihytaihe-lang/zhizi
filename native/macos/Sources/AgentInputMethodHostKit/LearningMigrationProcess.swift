import AgentIMECoreBridge
import Foundation

public enum LearningMigrationProcess {
    // Run before AppKit or CoreSession initialization: librime binds its directories once per process.
    public static func workerExitStatus(arguments: [String]) -> Int32? {
        guard arguments.dropFirst().first == "--migrate-learning" else { return nil }
        guard arguments.count == 4 else { return 64 }
        let shared = Array(arguments[2].utf8)
        let user = Array(arguments[3].utf8)
        let status = shared.withUnsafeBufferPointer { sharedBuffer in
            user.withUnsafeBufferPointer { userBuffer in
                aim_learning_migrate(sharedBuffer.baseAddress, sharedBuffer.count,
                                     userBuffer.baseAddress, userBuffer.count)
            }
        }
        return status == 0 ? 0 : 1
    }

    static func libraryExists(userDirectory: URL) -> Bool {
        let user = Array(userDirectory.path.utf8)
        return user.withUnsafeBufferPointer {
            aim_learning_available($0.baseAddress, $0.count) == 1
        }
    }

    @MainActor
    static func start(
        executable: URL?, sharedDirectory: URL, userDirectory: URL, timeout: TimeInterval = 120,
        completion: @escaping @MainActor @Sendable (Bool) -> Void
    ) {
        DispatchQueue.global(qos: .utility).async {
            let process = Process()
            process.executableURL = executable
            process.arguments = ["--migrate-learning", sharedDirectory.path, userDirectory.path]
            // A worker never emits dictionary contents or paths into host diagnostics.
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            let success: Bool
            do {
                try process.run()
                // A stuck export must not leave the switch pending forever: stop it and stay on Rime.
                let deadline = Date().addingTimeInterval(timeout)
                while process.isRunning && Date() < deadline {
                    Thread.sleep(forTimeInterval: 0.2)
                }
                if process.isRunning {
                    process.terminate()
                    process.waitUntilExit()
                    success = false
                } else {
                    success = process.terminationReason == .exit && process.terminationStatus == 0
                }
            } catch {
                success = false
            }
            DispatchQueue.main.async { completion(success) }
        }
    }
}
