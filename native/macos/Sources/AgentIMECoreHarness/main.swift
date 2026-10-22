import AgentInputMethodHostKit
import Foundation

let arguments = CommandLine.arguments
guard arguments.count >= 4 else {
    fputs("usage: AgentIMECoreHarness <shared-rime-dir> <user-rime-dir-or-storage-root> <query> [--keys <text>] [--app <bundle-id>] [--commit] [--storage-fallback <parent>]\n", stderr)
    exit(2)
}

let sharedDirectory = URL(fileURLWithPath: arguments[1], isDirectory: true)
var storageLease: RuntimeStorageLease?
let userDirectory: URL
if let fallbackIndex = arguments.firstIndex(of: "--storage-fallback"),
   arguments.indices.contains(fallbackIndex + 1) {
    do {
        let lease = try RuntimeStorageLease.resolve(
            primaryRoot: URL(fileURLWithPath: arguments[2], isDirectory: true),
            fallbackParent: URL(fileURLWithPath: arguments[fallbackIndex + 1], isDirectory: true)
        )
        storageLease = lease
        userDirectory = lease.location.userDirectory
        print("storage_degraded=\(lease.location.degraded)")
    } catch {
        fputs("failed to resolve safe runtime storage\n", stderr)
        exit(7)
    }
} else {
    userDirectory = URL(fileURLWithPath: arguments[2], isDirectory: true)
}
let query = arguments[3]
let appIdentifier: String
if let appIndex = arguments.firstIndex(of: "--app"), arguments.indices.contains(appIndex + 1) {
    appIdentifier = arguments[appIndex + 1]
} else {
    appIdentifier = "com.agentinputlayer.harness"
}
try FileManager.default.createDirectory(at: userDirectory, withIntermediateDirectories: true)
guard let session = CoreSession(
    sharedDirectory: sharedDirectory,
    userDirectory: userDirectory,
    appIdentifier: appIdentifier
) else {
    fputs("failed to create real librime session\n", stderr)
    exit(3)
}
if let fuzzyIndex = arguments.firstIndex(of: "--fuzzy"), arguments.indices.contains(fuzzyIndex + 1) {
    let mask = UInt32(arguments[fuzzyIndex + 1]) ?? 0
    let status = session.setFuzzyMaskStatus(mask)
    guard status == 0 else {
        fputs("failed to configure fuzzy mask status \(status)\n", stderr)
        exit(6)
    }
}

var latest: IMEViewModel?
for character in query {
    latest = session.process(kind: .text, text: String(character), security: .nonSecure)
    guard latest != nil else {
        fputs("process failed\n", stderr)
        exit(4)
    }
}

if let latest {
    print("marked=\(latest.markedText)")
    print("candidates=\(latest.candidates.map(\.text).joined(separator: "|"))")
    print("secondary=\(latest.secondaryCandidates.map(\.text).joined(separator: "|"))")
}
if let keysIndex = arguments.firstIndex(of: "--keys"), arguments.indices.contains(keysIndex + 1) {
    for character in arguments[keysIndex + 1] {
        guard let result = session.process(kind: .text, text: String(character), security: .nonSecure) else {
            fputs("follow-up key failed\n", stderr)
            exit(8)
        }
        print("key=\(character) consumed=\(result.consumed) commit=\(result.commitText) marked=\(result.markedText)")
        print("key_candidates=\(result.candidates.map(\.text).joined(separator: "|"))")
        print("key_secondary=\(result.secondaryCandidates.map(\.text).joined(separator: "|"))")
    }
}
if arguments.contains("--commit") {
    guard let committed = session.process(kind: .space, security: .nonSecure) else { exit(5) }
    print("commit=\(committed.commitText)")
}
session.close()
storageLease?.cleanup()
