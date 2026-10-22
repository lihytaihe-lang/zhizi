import AgentInputMethodHostKit
import Foundation

let arguments = CommandLine.arguments
guard arguments.count == 3, let expectedVersion = UInt64(arguments[2]) else {
    fputs("usage: AgentIMESnapshotHarness <snapshot-root> <expected-version>\n", stderr)
    exit(2)
}
let root = URL(fileURLWithPath: arguments[1], isDirectory: true)
guard let snapshot = SnapshotReader(root: root).loadCurrentOrFallback(), snapshot.version == expectedVersion else {
    fputs("snapshot unavailable or version mismatch\n", stderr)
    exit(3)
}
print("version=\(snapshot.version) checksum=\(snapshot.checksum) file=\(snapshot.fileName) entries=\(snapshot.entryCount)")
