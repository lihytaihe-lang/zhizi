import AgentInputMethodHostKit
import Foundation
import RimeShim

private let dictionaryName = "pinyin_simp"
private let temporaryRoot = FileManager.default.temporaryDirectory
    .appendingPathComponent("agent-ime-userdb-\(UUID().uuidString)", isDirectory: true)

private struct RequestItem: Codable {
    let id: String
    let text: String
    let pinyin: String
    let entryLine: String?
}

private struct UserDBItem: Codable {
    let id: String?
    let text: String
    let pinyin: String
    let entryLine: String
}

private struct Response: Codable {
    let items: [UserDBItem]
}

private enum ControlError: Error, CustomStringConvertible {
    case invalidArguments
    case invalidRequest
    case initializationFailed
    case exportFailed
    case importFailed
    case targetMissing(String)
    case deleteFailed(String)
    case rollbackFailed(String)

    var description: String {
        switch self {
        case .invalidArguments: return "invalid arguments"
        case .invalidRequest: return "invalid request"
        case .initializationFailed: return "librime initialization failed"
        case .exportFailed: return "userdb export failed"
        case .importFailed: return "userdb import failed"
        case .targetMissing(let text): return "userdb target missing: \(text)"
        case .deleteFailed(let text): return "userdb delete failed: \(text)"
        case .rollbackFailed(let detail): return "userdb rollback failed: \(detail)"
        }
    }
}

private func temporaryFile(_ suffix: String) -> URL {
    temporaryRoot.appendingPathComponent("\(UUID().uuidString)-\(suffix)")
}

private func initialize(shared: URL, user: URL) throws {
    let result = shared.path.withCString { sharedPath in
        user.path.withCString { userPath in aim_rime_initialize(sharedPath, userPath) }
    }
    guard result != 0 else { throw ControlError.initializationFailed }
}

private func backupItems() throws -> [UserDBItem] {
    let output = temporaryFile("snapshot.userdb.txt")
    defer { try? FileManager.default.removeItem(at: output) }
    let backedUp = dictionaryName.withCString { dict in
        output.path.withCString { path in aim_rime_backup_user_dict(dict, path) }
    }
    guard backedUp != 0, let document = try? String(contentsOf: output, encoding: .utf8) else {
        throw ControlError.exportFailed
    }
    let header = document.split(separator: "\n", omittingEmptySubsequences: false)
        .map(String.init).filter { $0.hasPrefix("#") }.joined(separator: "\n")
    return document.split(separator: "\n", omittingEmptySubsequences: false).compactMap { raw in
        let line = String(raw)
        guard !line.isEmpty, !line.hasPrefix("#") else { return nil }
        let columns = line.split(separator: "\t", omittingEmptySubsequences: false)
        guard columns.count >= 3 else { return nil }
        let pinyin = String(columns[0]).trimmingCharacters(in: .whitespaces)
        let text = String(columns[1]).trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, !pinyin.isEmpty else { return nil }
        let metadata = columns.dropFirst(2).joined(separator: "\t")
        guard metadata.split(separator: " ").contains(where: {
            $0.hasPrefix("c=") && Int($0.dropFirst(2)).map { $0 > 0 } == true
        }) else { return nil }
        return UserDBItem(
            id: nil, text: text, pinyin: pinyin,
            entryLine: header + "\n" + line + "\n"
        )
    }.sorted { ($0.pinyin, $0.text) < ($1.pinyin, $1.text) }
}

private func listItems(user: URL) throws -> [UserDBItem] {
    let database = user.appendingPathComponent("pinyin_simp.userdb", isDirectory: true)
    guard FileManager.default.fileExists(atPath: database.path) else { return [] }
    let output = temporaryFile("export.txt")
    defer { try? FileManager.default.removeItem(at: output) }
    let count = dictionaryName.withCString { dict in
        output.path.withCString { path in aim_rime_export_user_dict(dict, path) }
    }
    guard count >= 0, let document = try? String(contentsOf: output, encoding: .utf8) else {
        throw ControlError.exportFailed
    }
    return document.split(separator: "\n", omittingEmptySubsequences: false).compactMap { raw in
        let line = String(raw)
        guard !line.isEmpty, !line.hasPrefix("#") else { return nil }
        let columns = line.split(separator: "\t", omittingEmptySubsequences: false)
        guard columns.count >= 3,
              let commits = Int(columns[2].trimmingCharacters(in: .whitespaces)), commits > 0 else {
            return nil
        }
        let text = String(columns[0]).trimmingCharacters(in: .whitespaces)
        let pinyin = String(columns[1]).trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, !pinyin.isEmpty else { return nil }
        // `list` is also the logical UserDB inspection contract. Preserve the
        // stable semantic value (commit count), not LevelDB bytes or volatile
        // backup headers, so probes can distinguish value changes from
        // additions/removals without touching the database implementation.
        return UserDBItem(
            id: nil, text: text, pinyin: pinyin, entryLine: String(commits)
        )
    }.sorted { ($0.pinyin, $0.text) < ($1.pinyin, $1.text) }
}

private func query(_ session: CoreSession, pinyin: String) throws -> IMEViewModel {
    var result: IMEViewModel?
    for character in pinyin.filter({ $0.isASCII && $0.isLetter }) {
        result = session.process(kind: .text, text: String(character), security: .nonSecure)
    }
    guard let result else { throw ControlError.invalidRequest }
    return result
}

private func delete(_ item: RequestItem, shared: URL, user: URL) throws {
    guard let session = CoreSession(
        sharedDirectory: shared, userDirectory: user,
        appIdentifier: "com.agentinputlayer.userdb-maintenance"
    ) else { throw ControlError.initializationFailed }
    defer { session.close() }
    var view = try query(session, pinyin: item.pinyin)
    for _ in 0..<256 {
        if let index = view.candidates.firstIndex(where: { $0.text == item.text }) {
            for _ in 0..<index {
                guard let next = session.process(kind: .down, security: .nonSecure) else {
                    throw ControlError.deleteFailed(item.text)
                }
                view = next
            }
            guard let deleted = session.process(
                kind: .deleteCandidate, modifiers: 1 << 1, security: .nonSecure
            ), deleted.status == 0 else { throw ControlError.deleteFailed(item.text) }
            return
        }
        guard !view.isLastPage,
              let next = session.process(kind: .pageDown, security: .nonSecure) else { break }
        view = next
    }
    throw ControlError.targetMissing(item.text)
}

/// Entries whose reading cannot be typed back into a candidate (a learned
/// `shen m` → 神呒 splits as an abbreviation instead) are marked deleted the
/// way Rime's own importer does: a negative commit count for the exact
/// text/reading pair.
private func markDeleted(_ item: RequestItem, user: URL) throws {
    let tableImport = temporaryFile("delete.txt")
    defer { try? FileManager.default.removeItem(at: tableImport) }
    try "\(item.text)\t\(item.pinyin)\t-1\n".write(
        to: tableImport, atomically: true, encoding: .utf8
    )
    let imported = dictionaryName.withCString { dict in
        tableImport.path.withCString { path in aim_rime_import_user_dict(dict, path) }
    }
    guard imported == 1 else { throw ControlError.deleteFailed(item.text) }
    guard try !listItems(user: user).contains(where: {
        $0.text == item.text && $0.pinyin == item.pinyin
    }) else { throw ControlError.deleteFailed(item.text) }
}

private func restore(_ items: [RequestItem]) throws -> [UserDBItem] {
    let snapshots = try items.map { item -> String in
        guard let snapshot = item.entryLine,
              snapshot.utf8.count <= 64 * 1024,
              snapshot.contains("#@/db_name\tpinyin_simp"),
              snapshot.split(separator: "\n").filter({ !$0.hasPrefix("#") }).count == 1 else {
            throw ControlError.invalidRequest
        }
        return snapshot
    }
    for (item, snapshot) in zip(items, snapshots) {
        guard let row = snapshot.split(separator: "\n").map(String.init).first(where: {
            !$0.hasPrefix("#") && !$0.isEmpty
        }) else { throw ControlError.invalidRequest }
        let columns = row.split(separator: "\t", omittingEmptySubsequences: false)
        guard columns.count >= 3,
              snapshot.contains("#@/db_type\tuserdb"),
              let commitsToken = columns[2].split(separator: " ").first(where: { $0.hasPrefix("c=") }),
              let commits = Int(commitsToken.dropFirst(2)), commits > 0 else {
            throw ControlError.invalidRequest
        }
        let tableImport = temporaryFile("revive.txt")
        defer { try? FileManager.default.removeItem(at: tableImport) }
        let code = String(columns[0]).trimmingCharacters(in: .whitespaces)
        let text = String(columns[1])
        guard code == item.pinyin, text == item.text else { throw ControlError.invalidRequest }
        try "\(text)\t\(code)\t\(commits)\n".write(
            to: tableImport, atomically: true, encoding: .utf8
        )
        let revived = dictionaryName.withCString { dict in
            tableImport.path.withCString { path in aim_rime_import_user_dict(dict, path) }
        }
        guard revived == 1 else { throw ControlError.importFailed }

        let input = temporaryFile("restore.userdb.txt")
        defer { try? FileManager.default.removeItem(at: input) }
        try snapshot.write(to: input, atomically: true, encoding: .utf8)
        let restored = input.path.withCString { path in aim_rime_restore_user_dict(path) }
        guard restored != 0 else { throw ControlError.importFailed }
    }
    return zip(items, snapshots).map { item, snapshot in
        UserDBItem(id: item.id, text: item.text, pinyin: item.pinyin, entryLine: snapshot)
    }
}

private func run() throws {
    let arguments = CommandLine.arguments
    guard arguments.count >= 4 else { throw ControlError.invalidArguments }
    let operation = arguments[1]
    let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
    let user = URL(fileURLWithPath: arguments[3], isDirectory: true)
    try FileManager.default.createDirectory(
        at: temporaryRoot, withIntermediateDirectories: false,
        attributes: [.posixPermissions: 0o700]
    )
    defer { try? FileManager.default.removeItem(at: temporaryRoot) }
    try initialize(shared: shared, user: user)

    let response: Response
    switch operation {
    case "list":
        guard arguments.count == 4 else { throw ControlError.invalidArguments }
        response = Response(items: try listItems(user: user))
    case "delete":
        let data = FileHandle.standardInput.readDataToEndOfFile()
        guard arguments.count == 4,
              let request = try? JSONDecoder().decode([RequestItem].self, from: data),
              !request.isEmpty else { throw ControlError.invalidRequest }
        let before = try backupItems()
        var deleted: [UserDBItem] = []
        do {
            for item in request {
                guard let original = before.first(where: {
                    $0.text == item.text && $0.pinyin == item.pinyin
                }) else { throw ControlError.targetMissing(item.text) }
                do {
                    try delete(item, shared: shared, user: user)
                } catch ControlError.targetMissing {
                    try markDeleted(item, user: user)
                }
                deleted.append(UserDBItem(
                    id: item.id, text: item.text, pinyin: item.pinyin,
                    entryLine: original.entryLine
                ))
            }
        } catch {
            let rollback = deleted.map {
                RequestItem(
                    id: $0.id ?? "rollback", text: $0.text, pinyin: $0.pinyin,
                    entryLine: $0.entryLine
                )
            }
            if !rollback.isEmpty {
                do {
                    _ = try restore(rollback)
                } catch let rollbackError {
                    throw ControlError.rollbackFailed(
                        "original=\(error); rollback=\(rollbackError)"
                    )
                }
            }
            throw error
        }
        response = Response(items: deleted)
    case "restore":
        let data = FileHandle.standardInput.readDataToEndOfFile()
        guard arguments.count == 4,
              let request = try? JSONDecoder().decode([RequestItem].self, from: data),
              !request.isEmpty else { throw ControlError.invalidRequest }
        response = Response(items: try restore(request))
    default:
        throw ControlError.invalidArguments
    }
    let data = try JSONEncoder().encode(response)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

do {
    try run()
} catch {
    FileHandle.standardError.write(Data("AgentIMEUserDBCtl: \(error)\n".utf8))
    exit(2)
}
