import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class LexiconPackInventoryTests: XCTestCase {
    private func scratch() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("lexicon-packs-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }

    private let shell = "---\nname: agent_pack_poetry\nversion: \"0.1.0\"\nsort: by_weight\n...\n"
    private let full = "---\nname: agent_pack_poetry\nversion: \"0.1.0\"\nsort: by_weight\n...\n\n哀江头\tai jiang tou\t200\n"

    private func layer(_ id: String, group: LexiconCatalog.Group = .pack, rows: Int = 100) throws -> LexiconCatalog.Layer {
        let json = #"{"formatVersion":1,"layers":[{"id":"\#(id)","name":"x","description":"d","group":"\#(group.rawValue)","defaultEnabled":false,"rows":\#(rows)}]}"#
        return try XCTUnwrap(try LexiconCatalog.decode(Data(json.utf8)).layers.first)
    }

    private func install(_ id: String, in user: URL, dict: String? = nil) throws {
        try (dict ?? full).write(to: user.appendingPathComponent("\(id).dict.yaml"), atomically: true, encoding: .utf8)
        let packs = user.appendingPathComponent("lexicon_packs")
        try FileManager.default.createDirectory(at: packs, withIntermediateDirectories: true)
        try "哀江头\t\(id)\n".write(to: packs.appendingPathComponent("\(id).membership.tsv"), atomically: true, encoding: .utf8)
    }

    func testHeaderOnlyShellHasNoWordsAndAFullDictionaryDoes() throws {
        let root = try scratch()
        let shellFile = root.appendingPathComponent("shell.yaml"), fullFile = root.appendingPathComponent("full.yaml")
        try shell.write(to: shellFile, atomically: true, encoding: .utf8)
        try full.write(to: fullFile, atomically: true, encoding: .utf8)
        XCTAssertFalse(LexiconPackInventory.hasWords(shellFile))
        XCTAssertTrue(LexiconPackInventory.hasWords(fullFile))
        try (shell + "# only a comment\n   \n").write(to: shellFile, atomically: true, encoding: .utf8)
        XCTAssertFalse(LexiconPackInventory.hasWords(shellFile), "comments and blank lines are not words")
        XCTAssertFalse(LexiconPackInventory.hasWords(root.appendingPathComponent("missing.yaml")))
    }

    func testStateFollowsBundleAndUserDirectory() throws {
        let bundle = try scratch(), user = try scratch()
        let poetry = try layer("agent_pack_poetry")
        let inventory = LexiconPackInventory(bundledRime: bundle, userRime: user)
        try shell.write(to: bundle.appendingPathComponent("agent_pack_poetry.dict.yaml"), atomically: true, encoding: .utf8)
        XCTAssertEqual(inventory.state(of: poetry), .notInstalled, "a shell in the bundle and nothing downloaded")
        try "哀江头\tagent_pack_poetry\n".write(to: user.appendingPathComponent("stray.tsv"), atomically: true, encoding: .utf8)
        XCTAssertEqual(inventory.state(of: poetry), .notInstalled)
        try install("agent_pack_poetry", in: user)
        XCTAssertEqual(inventory.state(of: poetry), .installed)
        try FileManager.default.removeItem(at: user.appendingPathComponent("lexicon_packs/agent_pack_poetry.membership.tsv"))
        XCTAssertEqual(inventory.state(of: poetry), .notInstalled, "a dictionary without its membership table is not an install")
        try full.write(to: bundle.appendingPathComponent("agent_pack_poetry.dict.yaml"), atomically: true, encoding: .utf8)
        XCTAssertEqual(inventory.state(of: poetry), .bundled, "words in the app bundle need no download")
        XCTAssertEqual(inventory.state(of: try layer("agent_general", group: .core)), .bundled)
        XCTAssertEqual(inventory.state(of: try layer("anything", rows: 0)), .bundled, "an empty layer has nothing to download")
        XCTAssertEqual(LexiconPackInventory(bundledRime: nil, userRime: user).state(of: poetry), .bundled, "no bundle given: nothing is judged")
        XCTAssertEqual(inventory.state(of: try layer("not_in_bundle")), .notInstalled, "a layer the bundle lacks entirely")
    }

    func testFingerprintChangesWithInstallsAndRemovals() throws {
        let user = try scratch()
        let inventory = LexiconPackInventory(bundledRime: nil, userRime: user)
        let empty = inventory.fingerprint()
        XCTAssertEqual(empty, "")
        try install("agent_pack_poetry", in: user)
        let one = inventory.fingerprint()
        XCTAssertNotEqual(one, empty)
        XCTAssertEqual(inventory.fingerprint(), one, "stable while nothing changes")
        try install("agent_pack_poetry", in: user, dict: full + "爱说南朝狂客\tai shuo nan chao kuang ke\t200\n")
        XCTAssertNotEqual(inventory.fingerprint(), one, "a new dictionary version is a change")
        try FileManager.default.removeItem(at: user.appendingPathComponent("lexicon_packs/agent_pack_poetry.membership.tsv"))
        XCTAssertEqual(inventory.fingerprint(), "", "no membership table, no install")
    }

    func testDownloadedPacksFollowTheCoresReadingRules() throws {
        let user = try scratch()
        let inventory = LexiconPackInventory(bundledRime: nil, userRime: user)
        XCTAssertEqual(inventory.downloadedPacks(), [])
        let packs = user.appendingPathComponent("lexicon_packs")
        try FileManager.default.createDirectory(at: packs, withIntermediateDirectories: true)
        func leave(_ name: String, _ contents: String, mode: Int = 0o600) throws {
            try contents.write(to: packs.appendingPathComponent(name), atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: mode], ofItemAtPath: packs.appendingPathComponent(name).path)
        }
        try leave("agent_pack_poetry.membership.tsv", "# 注释\n\n哀江头\tagent_pack_poetry\n更漏子\tagent_pack_poetry\n坏层\tBAD\n无表\t词\n")
        XCTAssertEqual(inventory.downloadedPacks(), [DownloadedLexiconPack(id: "agent_pack_poetry", rows: 2)],
                       "comments and lines without valid layers carry no word")
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: packs.appendingPathComponent("agent_pack_poetry.membership.tsv").path)
        XCTAssertEqual(inventory.downloadedPacks(), [], "a file others may read is not installed")
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: packs.appendingPathComponent("agent_pack_poetry.membership.tsv").path)
        try leave("agent_pack_open.membership.tsv", "词\tagent_idioms\n", mode: 0o644)
        try leave("BAD.membership.tsv", "词\tagent_idioms\n")
        try leave("notes.txt", "词\tagent_idioms\n")
        try FileManager.default.createDirectory(at: packs.appendingPathComponent("agent_pack_dir.membership.tsv"), withIntermediateDirectories: true)
        let elsewhere = user.appendingPathComponent("elsewhere.tsv")
        try "链词\tagent_idioms\n".write(to: elsewhere, atomically: true, encoding: .utf8)
        try FileManager.default.createSymbolicLink(at: packs.appendingPathComponent("agent_pack_link.membership.tsv"), withDestinationURL: elsewhere)
        XCTAssertEqual(inventory.downloadedPacks().map(\.id), ["agent_pack_poetry"], "only the intact private file counts")
    }

    func testOnlyTheFirstEightDownloadedPacksAreListed() throws {
        let user = try scratch()
        let inventory = LexiconPackInventory(bundledRime: nil, userRime: user)
        let packs = user.appendingPathComponent("lexicon_packs")
        try FileManager.default.createDirectory(at: packs, withIntermediateDirectories: true)
        for index in 1...9 {
            let url = packs.appendingPathComponent("a_pack_\(index).membership.tsv")
            try "词\(index)\tagent_idioms\n".write(to: url, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        }
        XCTAssertEqual(inventory.downloadedPacks().map(\.id).last, "a_pack_8", "the ninth file by name is ignored, the way the core reads")
    }

    func testWindowOffersToGetMissingPacksAndSwitchesForInstalledOnes() throws {
        _ = NSApplication.shared
        let bundle = try scratch(), user = try scratch()
        let catalogURL = user.appendingPathComponent("catalog.json")
        let json = #"{"formatVersion":1,"layers":[{"id":"base","name":"基础词库","description":"d","group":"core","defaultEnabled":true,"rows":1000},{"id":"agent_pack_poetry","name":"诗词","description":"d","group":"pack","defaultEnabled":false,"rows":14880},{"id":"agent_pack_cad","name":"CAD 术语","description":"d","group":"pack","defaultEnabled":false,"rows":687}]}"#
        try Data(json.utf8).write(to: catalogURL)
        for id in ["agent_pack_poetry", "agent_pack_cad"] {
            try shell.write(to: bundle.appendingPathComponent("\(id).dict.yaml"), atomically: true, encoding: .utf8)
        }
        try install("agent_pack_poetry", in: user)
        let controller = LexiconWindowController(catalogURL: catalogURL, userDirectory: user, bundledRime: bundle)
        controller.show()
        defer { controller.close() }
        let window = try XCTUnwrap(NSApp.windows.first { $0.title == "词库" && $0.isVisible })
        func all(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(all) }
        let views = all(try XCTUnwrap(window.contentView))
        let switchViews = views.compactMap { $0 as? NSSwitch }
        XCTAssertEqual(switchViews.compactMap { $0.identifier?.rawValue }, ["agent_pack_poetry"], "only the installed pack has a switch")
        XCTAssertEqual(switchViews.first?.state, .on, "the download is the first appearance of the default-off pack: it starts on")
        let buttons = views.compactMap { $0 as? NSButton }.compactMap { $0.identifier?.rawValue }
        XCTAssertEqual(buttons, ["get-agent_pack_cad"], "the missing pack offers to be fetched")
        let texts = views.compactMap { ($0 as? NSTextField)?.stringValue }
        XCTAssertTrue(texts.contains("未下载"), "\(texts)")
        XCTAssertTrue(texts.contains { $0.hasPrefix("已安装") }, "\(texts)")
        XCTAssertTrue(texts.contains { $0.contains("已启用 2 个词库") }, "the downloaded pack counts, the missing one does not: \(texts)")
    }
}
