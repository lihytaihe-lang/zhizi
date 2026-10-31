import Darwin
import Foundation

/// Which optional layers have their words on this Mac. The public build ships the professional dictionaries as empty
/// shells; the management console downloads the real ones into the Rime user directory
/// (`<id>.dict.yaml` plus `lexicon_packs/<id>.membership.tsv`). The input method itself never touches the network.
enum LexiconPackState: Equatable {
    /// The words are inside the app bundle.
    case bundled
    /// Downloaded into the user directory.
    case installed
    /// Neither: the layer is listed but has no words here.
    case notInstalled
}

/// A pack in the download directory. The membership table is all a downloaded pack carries in the user directory,
/// so its data lines are the only row count there is; the name and description come from the catalog, if it lists
/// the pack at all.
struct DownloadedLexiconPack: Equatable {
    let id: String
    let rows: Int
}

struct LexiconPackInventory {
    let bundledRime: URL?
    let userRime: URL

    static let packsDirectoryName = "lexicon_packs"
    static let membershipSuffix = ".membership.tsv"
    /// The core's limits for pack tables (`lexicon_layers.rs`); a file beyond them counts as not installed there,
    /// so it must not be listed here either.
    static let maximumPackBytes: Int64 = 16 * 1024 * 1024
    static let maximumPackFiles = 8

    /// True when a data row follows the YAML `...` terminator. A header-only shell has none. Only the start of the file is read: the
    /// header is short, and a file whose header runs past the window is treated as having words.
    static func hasWords(_ url: URL) -> Bool {
        guard let data = try? Data(contentsOf: url, options: .mappedIfSafe) else { return false }
        let window = data.prefix(64 * 1024)
        var lines = window.split(separator: 0x0A, omittingEmptySubsequences: false)[...]
        // the last piece of a cut window may be half a line
        if data.count > window.count, !lines.isEmpty { lines = lines.dropLast() }
        guard let end = lines.firstIndex(where: { $0.elementsEqual("...".utf8) }) else { return data.count > window.count }
        for line in lines[lines.index(after: end)...] where !line.allSatisfy({ $0 == 0x20 || $0 == 0x09 || $0 == 0x0D }) && line.first != 0x23 {
            return true
        }
        return data.count > window.count
    }

    func state(of layer: LexiconCatalog.Layer) -> LexiconPackState {
        guard layer.group != .core, layer.rows > 0 else { return .bundled }
        let file = "\(layer.id).dict.yaml"
        if let bundledRime {
            let url = bundledRime.appendingPathComponent(file)
            if FileManager.default.fileExists(atPath: url.path) {
                if Self.hasWords(url) { return .bundled }
            } else {
                // a layer the bundle does not carry at all
                return userHasPack(layer.id) ? .installed : .notInstalled
            }
        } else {
            return .bundled
        }
        return userHasPack(layer.id) ? .installed : .notInstalled
    }

    private func userHasPack(_ id: String) -> Bool {
        let dict = userRime.appendingPathComponent("\(id).dict.yaml")
        let membership = userRime.appendingPathComponent(Self.packsDirectoryName)
            .appendingPathComponent("\(id)\(Self.membershipSuffix)")
        let manager = FileManager.default
        return manager.fileExists(atPath: dict.path) && manager.fileExists(atPath: membership.path)
    }

    /// The packs the running core would read from `lexicon_packs/` (the way `lexicon_layers.rs` loads them):
    /// membership tables with a well-formed id that are private regular files, at most the first
    /// `maximumPackFiles` by name.
    func downloadedPacks() -> [DownloadedLexiconPack] {
        let directory = userRime.appendingPathComponent(Self.packsDirectoryName)
        let names = ((try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? [])
            .compactMap { name -> String? in
                guard name.hasSuffix(Self.membershipSuffix) else { return nil }
                return LexiconChoices.validID(String(name.dropLast(Self.membershipSuffix.count))) ? name : nil
            }
            .sorted()
        var packs: [DownloadedLexiconPack] = []
        for name in names.prefix(Self.maximumPackFiles) {
            let url = directory.appendingPathComponent(name)
            var status = stat()
            guard lstat(url.path, &status) == 0,
                  status.st_mode & S_IFMT == S_IFREG,
                  status.st_uid == geteuid(),
                  status.st_nlink == 1,
                  status.st_mode & 0o077 == 0,
                  status.st_size <= Self.maximumPackBytes,
                  let table = try? String(contentsOf: url, encoding: .utf8) else { continue }
            packs.append(DownloadedLexiconPack(
                id: String(name.dropLast(Self.membershipSuffix.count)),
                rows: Self.membershipRows(table)
            ))
        }
        return packs
    }

    /// Data lines of a membership table, the shape `lexicon_layers.rs` parses: `word<TAB>layer[,layer]` with every
    /// layer id valid; `#` and blank lines carry no word.
    static func membershipRows(_ table: String) -> Int {
        var rows = 0
        for raw in table.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.hasSuffix("\r") ? raw.dropLast() : raw[...]
            guard !line.isEmpty, !line.hasPrefix("#"),
                  let separator = line.firstIndex(of: "\t") else { continue }
            let layers = line[line.index(after: separator)...].split(separator: ",").map(String.init)
            guard !line[..<separator].isEmpty, !layers.isEmpty,
                  layers.allSatisfy(LexiconChoices.validID) else { continue }
            rows += 1
        }
        return rows
    }

    /// Name, size and modification time of every installed pack file. Any change means the running engine is out of date.
    func fingerprint() -> String {
        var parts: [String] = []
        let manager = FileManager.default
        let packs = userRime.appendingPathComponent(Self.packsDirectoryName)
        let names = ((try? manager.contentsOfDirectory(atPath: packs.path)) ?? [])
            .filter { $0.hasSuffix(Self.membershipSuffix) }.sorted()
        for name in names {
            let id = String(name.dropLast(Self.membershipSuffix.count))
            for url in [packs.appendingPathComponent(name), userRime.appendingPathComponent("\(id).dict.yaml")] {
                if let attributes = try? manager.attributesOfItem(atPath: url.path) {
                    let size = (attributes[.size] as? NSNumber)?.int64Value ?? -1
                    let time = (attributes[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
                    parts.append("\(url.lastPathComponent):\(size):\(time)")
                } else {
                    parts.append("\(url.lastPathComponent):missing")
                }
            }
        }
        return parts.joined(separator: "|")
    }
}

/// What the running process last deployed. Every controller owns its own Rime session, but the deployed schema is
/// process-wide, so the fingerprint it was built from is process-wide too.
final class LexiconPackDeployment: @unchecked Sendable {
    static let shared = LexiconPackDeployment()
    private let lock = NSLock()
    private var deployed: String?

    /// The fingerprint the engine was built from; the first caller fixes it from the disk as it is when the engine starts.
    func deployedFingerprint(current: @autoclosure () -> String) -> String {
        lock.lock()
        defer { lock.unlock() }
        if let deployed { return deployed }
        let value = current()
        deployed = value
        return value
    }

    func record(_ fingerprint: String) {
        lock.lock()
        deployed = fingerprint
        lock.unlock()
    }

    func resetForTesting() {
        lock.lock()
        deployed = nil
        lock.unlock()
    }
}
