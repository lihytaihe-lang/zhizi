import AppKit
import AgentIMECoreBridge
import CryptoKit
import Security
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

final class HostSupportTests: XCTestCase {
    func testSyntheticPerformanceClientRetainsOnlyABoundedDocumentSuffix() {
        let client = SyntheticTextInputClient(appIdentifier: "com.agentinputlayer.synthetic-test")
        let chunk = String(repeating: "中文🙂abc", count: 200)
        let insertions = 2_000
        for _ in 0..<insertions {
            client.insertText(chunk, replacementRange: NSRange(location: NSNotFound, length: 0))
        }

        let expectedLength = chunk.utf16.count * insertions
        XCTAssertEqual(client.length(), expectedLength)
        XCTAssertEqual(client.selectedRange(), NSRange(location: expectedLength, length: 0))
        XCTAssertLessThanOrEqual(
            client.retainedDocumentUTF16CountForTesting,
            SyntheticTextInputClient.maximumRetainedDocumentUTF16Count
        )
        XCTAssertNil(client.string(from: NSRange(location: 0, length: 1), actualRange: nil))

        client.insertText("终点🙂", replacementRange: NSRange(location: NSNotFound, length: 0))
        let tailLength = "终点🙂".utf16.count
        let tailRange = NSRange(location: client.length() - tailLength, length: tailLength)
        XCTAssertEqual(client.string(from: tailRange, actualRange: nil), "终点🙂")
        XCTAssertEqual(client.attributedSubstring(from: tailRange)?.string, "终点🙂")
    }

    func testMinusAndEqualsUseTheSamePagingContractAsPageKeys() throws {
        let mapper = EventMapper()
        func event(
            keyCode: UInt16,
            characters: String,
            ignoringModifiers: String? = nil,
            modifiers: NSEvent.ModifierFlags = []
        ) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown,
                location: .zero,
                modifierFlags: modifiers,
                timestamp: 0,
                windowNumber: 0,
                context: nil,
                characters: characters,
                charactersIgnoringModifiers: ignoringModifiers ?? characters,
                isARepeat: false,
                keyCode: keyCode
            ))
        }
        XCTAssertEqual(mapper.map(try event(keyCode: 27, characters: "-"))?.kind, .pageUp)
        XCTAssertEqual(mapper.map(try event(keyCode: 24, characters: "="))?.kind, .pageDown)
        XCTAssertEqual(mapper.map(try event(keyCode: 116, characters: ""))?.kind, .pageUp)
        XCTAssertEqual(mapper.map(try event(keyCode: 121, characters: ""))?.kind, .pageDown)
        let underscore = mapper.map(try event(
            keyCode: 27, characters: "_", ignoringModifiers: "-", modifiers: .shift
        ))
        XCTAssertEqual(underscore?.kind, .text)
        XCTAssertEqual(underscore?.text, "_")
        let plus = mapper.map(try event(
            keyCode: 24, characters: "+", ignoringModifiers: "=", modifiers: .shift
        ))
        XCTAssertEqual(plus?.kind, .text)
        XCTAssertEqual(plus?.text, "+")
    }

    func testShiftedPrintableCharactersReachTheCoreWithoutLosingShiftMeaning() throws {
        func shifted(_ characters: String, ignoring: String, keyCode: UInt16) throws -> MappedIMEEvent {
            let event = try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: .shift, timestamp: 1,
                windowNumber: 0, context: nil, characters: characters,
                charactersIgnoringModifiers: ignoring, isARepeat: false, keyCode: keyCode
            ))
            return try XCTUnwrap(EventMapper().map(event))
        }

        for fixture in [
            (text: "!", ignoring: "1", keyCode: UInt16(18)),
            (text: "\"", ignoring: "'", keyCode: UInt16(39)),
            (text: ":", ignoring: ";", keyCode: UInt16(41)),
            (text: "(", ignoring: "9", keyCode: UInt16(25)),
            (text: "<", ignoring: ",", keyCode: UInt16(43)),
            (text: ">", ignoring: ".", keyCode: UInt16(47)),
        ] {
            let mapped = try shifted(fixture.text, ignoring: fixture.ignoring, keyCode: fixture.keyCode)
            XCTAssertEqual(mapped.kind, .text)
            XCTAssertEqual(mapped.text, fixture.text)
            XCTAssertNotEqual(mapped.modifiers & UInt32(AIM_MOD_SHIFT), 0)
        }
    }

    func testShiftWithSystemModifierNeverEmitsModeToggle() throws {
        let mapper = EventMapper()
        func flags(_ keyCode: UInt16, _ modifiers: NSEvent.ModifierFlags) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(
                with: .flagsChanged, location: .zero, modifierFlags: modifiers, timestamp: 0,
                windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "",
                isARepeat: false, keyCode: keyCode
            ))
        }
        XCTAssertNil(mapper.map(try flags(56, [.shift, .command])))
        XCTAssertNil(mapper.map(try flags(56, [])))
        XCTAssertNil(mapper.map(try flags(56, [.shift])))
        XCTAssertNil(mapper.map(try flags(55, [.shift, .command])))
        XCTAssertNil(mapper.map(try flags(56, [])))
        XCTAssertNil(mapper.map(try flags(56, [.shift])))
        XCTAssertEqual(mapper.map(try flags(56, []))?.kind, .shiftTap)
    }

    func testUnknownNonPrintableKeyMapsToUnsupportedAction() throws {
        let event = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "",
            isARepeat: false, keyCode: 110
        ))
        XCTAssertEqual(EventMapper().map(event)?.kind, .unsupported)
    }

    func testOnlyExactControlDeleteMapsToCandidateDeletion() throws {
        func deleteEvent(_ modifiers: NSEvent.ModifierFlags) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: modifiers, timestamp: 1,
                windowNumber: 0, context: nil, characters: "\u{7f}",
                charactersIgnoringModifiers: "\u{7f}", isARepeat: false, keyCode: 51
            ))
        }

        XCTAssertEqual(EventMapper().map(try deleteEvent([.control]))?.kind, .deleteCandidate)
        XCTAssertEqual(EventMapper().map(try deleteEvent([.control, .capsLock]))?.kind, .deleteCandidate)
        XCTAssertEqual(EventMapper().map(try deleteEvent([.control, .shift]))?.kind, .blacklistCandidate)
        for modifiers: NSEvent.ModifierFlags in [
            [.control, .command], [.control, .option],
        ] {
            let mapped = try XCTUnwrap(EventMapper().map(deleteEvent(modifiers)))
            XCTAssertEqual(mapped.kind, .backspace)
            XCTAssertNotEqual(mapped.modifiers & UInt32(AIM_MOD_CONTROL), 0)
        }
    }

    func testEventMapperCarriesTheCGAutorepeatSignal() throws {
        func backspace(isAutorepeat: Bool) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: [], timestamp: 1,
                windowNumber: 0, context: nil, characters: "\u{7f}",
                charactersIgnoringModifiers: "\u{7f}", isARepeat: isAutorepeat, keyCode: 51
            ))
        }

        XCTAssertFalse(try XCTUnwrap(EventMapper().map(backspace(isAutorepeat: false))).isAutorepeat)
        XCTAssertTrue(try XCTUnwrap(EventMapper().map(backspace(isAutorepeat: true))).isAutorepeat)
    }

    func testEventDeliveryIDUsesStableCGMetadataAndFailsClosedWithoutTimestamp() throws {
        let appKitKeyEvent = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 1,
            windowNumber: 0, context: nil, characters: "1", charactersIgnoringModifiers: "1",
            isARepeat: false, keyCode: 18
        ))
        XCTAssertNotNil(EventMapper().map(appKitKeyEvent))

        let first = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [.shift], timestamp: 9.87654321,
            windowNumber: 0, context: nil, characters: "!", charactersIgnoringModifiers: "1",
            isARepeat: false, keyCode: 18
        ))
        let identifier = EventDeliveryIDFactory.identifier(for: first)
        XCTAssertNotEqual(identifier, 0)
        XCTAssertEqual(EventDeliveryIDFactory.identifier(for: first), identifier)
        XCTAssertEqual(EventMapper().map(first)?.deliveryID, identifier)
        XCTAssertNotEqual(EventDeliveryIDFactory.identifier(for: first, candidateIndex: 0), 0)

        let different = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [.shift], timestamp: 9.876543211,
            windowNumber: 0, context: nil, characters: "!", charactersIgnoringModifiers: "1",
            isARepeat: false, keyCode: 18
        ))
        XCTAssertNotEqual(EventDeliveryIDFactory.identifier(for: different), identifier)

        let missingTimestamp = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [.shift], timestamp: 0,
            windowNumber: 0, context: nil, characters: "!", charactersIgnoringModifiers: "1",
            isARepeat: false, keyCode: 18
        ))
        XCTAssertEqual(EventDeliveryIDFactory.identifier(for: missingTimestamp), 0)

        let mouse = try XCTUnwrap(NSEvent.mouseEvent(
            with: .leftMouseUp, location: NSPoint(x: 12, y: 34), modifierFlags: [],
            timestamp: 12.5, windowNumber: 1, context: nil,
            eventNumber: 99, clickCount: 1, pressure: 0
        ))
        let mouseIdentifier = EventDeliveryIDFactory.identifier(for: mouse, candidateIndex: 0)
        XCTAssertNotEqual(mouseIdentifier, 0)
        XCTAssertEqual(EventDeliveryIDFactory.identifier(for: mouse, candidateIndex: 0), mouseIdentifier)
        XCTAssertNotEqual(EventDeliveryIDFactory.identifier(for: mouse, candidateIndex: 1), mouseIdentifier)
        XCTAssertNil(EventMapper().map(mouse))
    }

    func testIncompatibleUserDBIsQuarantinedAndLastValidBackupRestored() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-userdb-manager-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let database = root.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
        try FileManager.default.createDirectory(at: database, withIntermediateDirectories: true)
        try Data("MANIFEST-000001\n".utf8).write(to: database.appendingPathComponent("CURRENT"))
        try Data("fixture".utf8).write(to: database.appendingPathComponent("MANIFEST-000001"))
        let initial = try UserDBManager.prepare(root: root)
        XCTAssertTrue(initial.backedUp)
        let rootMode = try FileManager.default.attributesOfItem(atPath: root.path)[.posixPermissions] as? NSNumber
        let rimeMode = try FileManager.default.attributesOfItem(atPath: root.appendingPathComponent("Rime").path)[.posixPermissions] as? NSNumber
        XCTAssertEqual(rootMode?.intValue, 0o700)
        XCTAssertEqual(rimeMode?.intValue, 0o700)
        try Data("../invalid\n".utf8).write(to: database.appendingPathComponent("CURRENT"), options: .atomic)
        let recovered = try UserDBManager.prepare(root: root)
        XCTAssertTrue(recovered.quarantined)
        XCTAssertTrue(recovered.restored)
        XCTAssertTrue(UserDBManager.isStructurallyValid(database))
        let quarantine = root.appendingPathComponent("IncompatibleUserDB")
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: quarantine.path).count, 1)
    }

    func testMissingUserDBAfterInterruptedQuarantineRestoresNewestValidatedBackup() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-userdb-interrupted-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let database = root.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
        try writeValidUserDB(database, marker: "pre-interruption-database")
        let initial = try UserDBManager.prepare(root: root)
        XCTAssertTrue(initial.backedUp)

        // Model a process exit immediately after quarantine move and before
        // the restore stage is copied into the live database path.
        let interruptedQuarantine = root.appendingPathComponent(
            "IncompatibleUserDB/interrupted/pinyin_simp.userdb", isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: interruptedQuarantine.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try FileManager.default.moveItem(at: database, to: interruptedQuarantine)
        XCTAssertFalse(FileManager.default.fileExists(atPath: database.path))

        let recovered = try UserDBManager.prepare(root: root)
        XCTAssertFalse(recovered.quarantined)
        XCTAssertTrue(recovered.restored)
        XCTAssertTrue(recovered.backedUp)
        XCTAssertTrue(UserDBManager.isStructurallyValid(database))
        XCTAssertEqual(
            try String(contentsOf: database.appendingPathComponent("TEST_MARKER"), encoding: .utf8),
            "pre-interruption-database"
        )
        XCTAssertEqual(
            try String(
                contentsOf: interruptedQuarantine.appendingPathComponent("TEST_MARKER"),
                encoding: .utf8
            ),
            "pre-interruption-database"
        )
    }

    func testUserDBManagerRejectsDatabaseSymlinkWithoutMovingTarget() throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-userdb-symlink-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let root = base.appendingPathComponent("root", isDirectory: true)
        let rime = root.appendingPathComponent("Rime", isDirectory: true)
        let external = base.appendingPathComponent("external.userdb", isDirectory: true)
        try FileManager.default.createDirectory(at: rime, withIntermediateDirectories: true)
        try writeValidUserDB(external, marker: "external-database")
        let externalMarker = external.appendingPathComponent("TEST_MARKER")
        try FileManager.default.setAttributes([.posixPermissions: 0o750], ofItemAtPath: external.path)
        try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: externalMarker.path)
        let externalModeBefore = try posixMode(external)
        let markerModeBefore = try posixMode(externalMarker)
        let database = rime.appendingPathComponent("pinyin_simp.userdb", isDirectory: true)
        try FileManager.default.createSymbolicLink(at: database, withDestinationURL: external)

        XCTAssertThrowsError(try UserDBManager.prepare(root: root))
        XCTAssertTrue(try database.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink == true)
        XCTAssertEqual(
            try String(contentsOf: externalMarker, encoding: .utf8),
            "external-database"
        )
        XCTAssertEqual(try posixMode(external), externalModeBefore)
        XCTAssertEqual(try posixMode(externalMarker), markerModeBefore)
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("IncompatibleUserDB").path))
    }

    func testUserDBManagerRejectsCurrentSymlinkWithoutQuarantiningDatabase() throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-userdb-current-link-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let root = base.appendingPathComponent("root", isDirectory: true)
        let database = root.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
        try FileManager.default.createDirectory(at: database, withIntermediateDirectories: true)
        try Data("fixture".utf8).write(to: database.appendingPathComponent("MANIFEST-000001"))
        try FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        let externalCurrent = base.appendingPathComponent("external-CURRENT")
        try Data("MANIFEST-000001\n".utf8).write(to: externalCurrent)
        try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: externalCurrent.path)
        let externalModeBefore = try posixMode(externalCurrent)
        try FileManager.default.createSymbolicLink(
            at: database.appendingPathComponent("CURRENT"), withDestinationURL: externalCurrent
        )

        XCTAssertThrowsError(try UserDBManager.prepare(root: root))
        XCTAssertTrue(FileManager.default.fileExists(atPath: database.appendingPathComponent("MANIFEST-000001").path))
        XCTAssertEqual(try String(contentsOf: externalCurrent, encoding: .utf8), "MANIFEST-000001\n")
        XCTAssertEqual(try posixMode(externalCurrent), externalModeBefore)
    }

    func testUserDBManagerRejectsSymlinkedBackupBeforeMutatingCurrentDatabase() throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-userdb-backup-link-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let root = base.appendingPathComponent("root", isDirectory: true)
        let database = root.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
        let backupRoot = root.appendingPathComponent("UserDBBackups", isDirectory: true)
        let externalBackup = base.appendingPathComponent("external-backup", isDirectory: true)
        try writeValidUserDB(database, marker: "current-database")
        try writeValidUserDB(
            externalBackup.appendingPathComponent("pinyin_simp.userdb", isDirectory: true),
            marker: "external-backup"
        )
        let externalMarker = externalBackup.appendingPathComponent("pinyin_simp.userdb/TEST_MARKER")
        try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: externalMarker.path)
        let externalModeBefore = try posixMode(externalMarker)
        try FileManager.default.createDirectory(at: backupRoot, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(
            at: backupRoot.appendingPathComponent("linked-backup"),
            withDestinationURL: externalBackup
        )

        XCTAssertThrowsError(try UserDBManager.prepare(root: root))
        XCTAssertEqual(
            try String(contentsOf: database.appendingPathComponent("TEST_MARKER"), encoding: .utf8),
            "current-database"
        )
        XCTAssertEqual(
            try String(
                contentsOf: externalMarker,
                encoding: .utf8
            ),
            "external-backup"
        )
        XCTAssertEqual(try posixMode(externalMarker), externalModeBefore)
    }

    func testUserDBManagerRejectsHardLinkedCurrentAndManifestWithoutMutatingExternalFiles() throws {
        let fixtures: [(leaf: String, contents: String)] = [
            ("CURRENT", "MANIFEST-000001\n"),
            ("MANIFEST-000001", "fixture"),
            ("000001.ldb", "leveldb-table-fixture"),
        ]

        for fixture in fixtures {
            let base = FileManager.default.temporaryDirectory
                .appendingPathComponent("agent-ime-userdb-hardlink-\(fixture.leaf)-\(UUID().uuidString)", isDirectory: true)
            defer { try? FileManager.default.removeItem(at: base) }
            let root = base.appendingPathComponent("root", isDirectory: true)
            let database = root.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
            let external = base.appendingPathComponent("external-\(fixture.leaf)")
            try FileManager.default.createDirectory(at: database, withIntermediateDirectories: true)
            try Data(fixture.contents.utf8).write(to: external)
            try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: external.path)
            try FileManager.default.linkItem(at: external, to: database.appendingPathComponent(fixture.leaf))

            if fixture.leaf != "CURRENT" {
                try Data("MANIFEST-000001\n".utf8).write(to: database.appendingPathComponent("CURRENT"))
            }
            if fixture.leaf != "MANIFEST-000001" {
                try Data("fixture".utf8).write(to: database.appendingPathComponent("MANIFEST-000001"))
            }

            let contentBefore = try Data(contentsOf: external)
            let modeBefore = try XCTUnwrap(
                FileManager.default.attributesOfItem(atPath: external.path)[.posixPermissions] as? NSNumber
            )
            let linkCountBefore = try XCTUnwrap(
                FileManager.default.attributesOfItem(atPath: external.path)[.referenceCount] as? NSNumber
            )
            XCTAssertEqual(linkCountBefore.uint64Value, 2)

            XCTAssertThrowsError(try UserDBManager.prepare(root: root))
            XCTAssertEqual(try Data(contentsOf: external), contentBefore)
            let modeAfter = try XCTUnwrap(
                FileManager.default.attributesOfItem(atPath: external.path)[.posixPermissions] as? NSNumber
            )
            XCTAssertEqual(modeAfter.uint16Value, modeBefore.uint16Value)
            let linkCountAfter = try XCTUnwrap(
                FileManager.default.attributesOfItem(atPath: external.path)[.referenceCount] as? NSNumber
            )
            XCTAssertEqual(linkCountAfter.uint64Value, linkCountBefore.uint64Value)
            XCTAssertTrue(FileManager.default.fileExists(atPath: database.path))
            XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("IncompatibleUserDB").path))
        }
    }

    func testUserDBManagerRejectsAnyHardLinkedBackupFileBeforeMutatingCurrentDatabase() throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-userdb-backup-hardlink-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let root = base.appendingPathComponent("root", isDirectory: true)
        let current = root.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
        let backup = root.appendingPathComponent(
            "UserDBBackups/fixture/pinyin_simp.userdb", isDirectory: true
        )
        try writeValidUserDB(current, marker: "current-database")
        try writeValidUserDB(backup, marker: "backup-database")

        let external = base.appendingPathComponent("external-leveldb-table")
        try Data("external-backup-table".utf8).write(to: external)
        try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: external.path)
        let linked = backup.appendingPathComponent("000001.ldb")
        try FileManager.default.linkItem(at: external, to: linked)
        let externalContentsBefore = try Data(contentsOf: external)
        let externalModeBefore = try posixMode(external)
        let externalLinksBefore = try referenceCount(external)

        XCTAssertThrowsError(try UserDBManager.prepare(root: root))
        XCTAssertEqual(
            try String(contentsOf: current.appendingPathComponent("TEST_MARKER"), encoding: .utf8),
            "current-database"
        )
        XCTAssertEqual(try Data(contentsOf: external), externalContentsBefore)
        XCTAssertEqual(try posixMode(external), externalModeBefore)
        XCTAssertEqual(try referenceCount(external), externalLinksBefore)
        XCTAssertTrue(FileManager.default.fileExists(atPath: linked.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("IncompatibleUserDB").path))
    }

    func testUserDBManagerRejectsAncestorSymlinkWithoutTouchingExternalTree() throws {
        let base = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-userdb-ancestor-link-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        try FileManager.default.createDirectory(at: base, withIntermediateDirectories: false)

        let externalParent = base.appendingPathComponent("external-parent", isDirectory: true)
        let externalSentinel = externalParent.appendingPathComponent("sentinel")
        try FileManager.default.createDirectory(at: externalParent, withIntermediateDirectories: false)
        try Data("must-not-change".utf8).write(to: externalSentinel)
        try FileManager.default.setAttributes([.posixPermissions: 0o750], ofItemAtPath: externalParent.path)
        try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: externalSentinel.path)
        let parentMode = try posixMode(externalParent)
        let sentinelMode = try posixMode(externalSentinel)
        let ancestor = base.appendingPathComponent("linked-ancestor", isDirectory: true)
        try FileManager.default.createSymbolicLink(at: ancestor, withDestinationURL: externalParent)

        let escapedRoot = ancestor.appendingPathComponent("would-be-root", isDirectory: true)
        XCTAssertThrowsError(try UserDBManager.prepare(root: escapedRoot))
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: externalParent.appendingPathComponent("would-be-root").path
        ))
        XCTAssertEqual(try Data(contentsOf: externalSentinel), Data("must-not-change".utf8))
        XCTAssertEqual(try posixMode(externalParent), parentMode)
        XCTAssertEqual(try posixMode(externalSentinel), sentinelMode)
    }

    func testUserDBManagerRejectsOversizedSparseFileBeforeBackupOrQuarantine() throws {
        let base = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-userdb-oversize-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let root = base.appendingPathComponent("root", isDirectory: true)
        let database = root.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
        try writeValidUserDB(database, marker: "current-database")
        let sparse = database.appendingPathComponent("oversized.ldb")
        XCTAssertTrue(FileManager.default.createFile(atPath: sparse.path, contents: Data()))
        let handle = try FileHandle(forWritingTo: sparse)
        try handle.truncate(
            atOffset: UInt64(LocalSecureTreeLimits.userDB.maximumSingleFileBytes + 1)
        )
        try handle.close()

        XCTAssertThrowsError(try UserDBManager.prepare(root: root))
        XCTAssertEqual(
            try String(contentsOf: database.appendingPathComponent("TEST_MARKER"), encoding: .utf8),
            "current-database"
        )
        let backups = root.appendingPathComponent("UserDBBackups", isDirectory: true)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: backups.path), [])
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: root.appendingPathComponent("IncompatibleUserDB").path
        ))
    }

    func testUserDBDescriptorOperationsFailClosedAfterAncestorReplacement() throws {
        let base = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-userdb-parent-replacement-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let parent = base.appendingPathComponent("parent", isDirectory: true)
        let root = parent.appendingPathComponent("root", isDirectory: true)
        try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true)
        let securedRoot = try LocalSecureDirectory.openOrCreateFinal(root, mode: mode_t(0o700))

        let displacedParent = base.appendingPathComponent("displaced-parent", isDirectory: true)
        try FileManager.default.moveItem(at: parent, to: displacedParent)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let replacementSentinel = root.appendingPathComponent("replacement-sentinel")
        try Data("replacement-must-survive".utf8).write(to: replacementSentinel)
        let sentinelMode = try posixMode(replacementSentinel)

        XCTAssertThrowsError(try UserDBManager.prepare(rootDirectory: securedRoot))
        XCTAssertEqual(
            try String(contentsOf: replacementSentinel, encoding: .utf8),
            "replacement-must-survive"
        )
        XCTAssertEqual(try posixMode(replacementSentinel), sentinelMode)
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: root.appendingPathComponent("Rime").path
        ))
    }

    func testLocalStoragePathCanonicalizationAllowsOnlySystemTemporaryAliases() throws {
        XCTAssertEqual(
            try LocalSecureDirectory.canonicalAbsoluteURL(
                URL(fileURLWithPath: "/tmp/agent-ime-fixture", isDirectory: true)
            ).path,
            "/private/tmp/agent-ime-fixture"
        )
        XCTAssertEqual(
            try LocalSecureDirectory.canonicalAbsoluteURL(
                URL(fileURLWithPath: "/var/folders/fixture", isDirectory: true)
            ).path,
            "/private/var/folders/fixture"
        )

        let base = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-local-path-link-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let real = base.appendingPathComponent("real", isDirectory: true)
        let link = base.appendingPathComponent("user-link", isDirectory: true)
        try FileManager.default.createDirectory(at: real, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: real)
        XCTAssertThrowsError(try LocalSecureDirectory.openExisting(link))
    }

    func testRuntimeStorageRejectsSymlinkedFallbackAncestorWithoutExternalMutation() throws {
        let base = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-runtime-fallback-link-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        try FileManager.default.createDirectory(at: base, withIntermediateDirectories: false)
        let primary = base.appendingPathComponent("primary")
        try Data("not-a-directory".utf8).write(to: primary)
        let externalParent = base.appendingPathComponent("external-parent", isDirectory: true)
        let sentinel = externalParent.appendingPathComponent("sentinel")
        try FileManager.default.createDirectory(at: externalParent, withIntermediateDirectories: false)
        try Data("must-survive".utf8).write(to: sentinel)
        try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: sentinel.path)
        let contentsBefore = try Data(contentsOf: sentinel)
        let modeBefore = try posixMode(sentinel)
        let linkedAncestor = base.appendingPathComponent("linked-parent", isDirectory: true)
        try FileManager.default.createSymbolicLink(at: linkedAncestor, withDestinationURL: externalParent)

        XCTAssertThrowsError(try RuntimeStorageLease.resolve(
            primaryRoot: primary,
            fallbackParent: linkedAncestor.appendingPathComponent("fallback", isDirectory: true)
        ))
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: externalParent.appendingPathComponent("fallback").path
        ))
        XCTAssertEqual(try Data(contentsOf: sentinel), contentsBefore)
        XCTAssertEqual(try posixMode(sentinel), modeBefore)
    }

    func testRuntimeStorageAcceptsTheSystemTmpAliasAsPrivateTmp() throws {
        let fixtureRoot = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-runtime-system-tmp-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: fixtureRoot) }
        try FileManager.default.createDirectory(at: fixtureRoot, withIntermediateDirectories: false)
        let rejectedPrimary = fixtureRoot.appendingPathComponent("primary")
        try Data("not-a-directory".utf8).write(to: rejectedPrimary)

        let lease = try RuntimeStorageLease.resolve(
            primaryRoot: rejectedPrimary,
            fallbackParent: URL(fileURLWithPath: "/tmp", isDirectory: true)
        )
        XCTAssertTrue(lease.location.degraded)
        XCTAssertTrue(lease.location.rootDirectory.path.hasPrefix("/private/tmp/agent-ime-ephemeral."))
        let root = lease.location.rootDirectory
        lease.cleanup()
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.path))
    }

    func testRuntimeStorageUsesPrivateEphemeralFallbackForRejectedPrimaryUserDB() throws {
        for fixture in ["database-symlink", "current-hardlink"] {
            let base = FileManager.default.temporaryDirectory.appendingPathComponent(
                "agent-ime-runtime-storage-\(fixture)-\(UUID().uuidString)", isDirectory: true
            )
            defer { try? FileManager.default.removeItem(at: base) }
            let primary = base.appendingPathComponent("primary", isDirectory: true)
            let database = primary.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
            let fallbackParent = base.appendingPathComponent("fallback", isDirectory: true)
            try FileManager.default.createDirectory(
                at: primary.appendingPathComponent("Rime", isDirectory: true),
                withIntermediateDirectories: true
            )
            try FileManager.default.createDirectory(at: fallbackParent, withIntermediateDirectories: true)
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: fallbackParent.path)

            let external: URL
            if fixture == "database-symlink" {
                external = base.appendingPathComponent("external.userdb", isDirectory: true)
                try writeValidUserDB(external, marker: "external-userdb")
                try FileManager.default.createSymbolicLink(at: database, withDestinationURL: external)
            } else {
                try FileManager.default.createDirectory(at: database, withIntermediateDirectories: true)
                external = base.appendingPathComponent("external-CURRENT")
                try Data("MANIFEST-000001\n".utf8).write(to: external)
                try FileManager.default.linkItem(
                    at: external,
                    to: database.appendingPathComponent("CURRENT")
                )
                try Data("fixture".utf8).write(
                    to: database.appendingPathComponent("MANIFEST-000001")
                )
            }
            let sentinel = fixture == "database-symlink"
                ? external.appendingPathComponent("TEST_MARKER")
                : external
            try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: sentinel.path)
            let sentinelData = try Data(contentsOf: sentinel)
            let sentinelMode = try posixMode(sentinel)
            let sentinelLinks = try referenceCount(sentinel)

            let lease = try RuntimeStorageLease.resolve(
                primaryRoot: primary,
                fallbackParent: fallbackParent
            )
            XCTAssertTrue(lease.location.degraded)
            XCTAssertNotEqual(lease.location.userDirectory.standardizedFileURL, database.deletingLastPathComponent())
            let canonicalFallback = try LocalSecureDirectory.canonicalAbsoluteURL(fallbackParent)
            XCTAssertTrue(lease.location.rootDirectory.path.hasPrefix(canonicalFallback.path + "/"))
            XCTAssertEqual(try posixMode(lease.location.rootDirectory), 0o700)
            XCTAssertEqual(try posixMode(lease.location.userDirectory), 0o700)
            XCTAssertEqual(try Data(contentsOf: sentinel), sentinelData)
            XCTAssertEqual(try posixMode(sentinel), sentinelMode)
            XCTAssertEqual(try referenceCount(sentinel), sentinelLinks)

            let ephemeralRoot = lease.location.rootDirectory
            lease.cleanup()
            XCTAssertFalse(FileManager.default.fileExists(atPath: ephemeralRoot.path))
            XCTAssertEqual(try Data(contentsOf: sentinel), sentinelData)
            XCTAssertEqual(try posixMode(sentinel), sentinelMode)
            XCTAssertEqual(try referenceCount(sentinel), sentinelLinks)
        }
    }

    func testRuntimeStorageCleanupDoesNotDeleteAReplacedFallbackDirectory() throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent(
            "agent-ime-runtime-storage-replaced-\(UUID().uuidString)", isDirectory: true
        )
        defer { try? FileManager.default.removeItem(at: base) }
        let primary = base.appendingPathComponent("primary", isDirectory: true)
        let external = base.appendingPathComponent("external.userdb", isDirectory: true)
        let fallbackParent = base.appendingPathComponent("fallback", isDirectory: true)
        try FileManager.default.createDirectory(
            at: primary.appendingPathComponent("Rime", isDirectory: true),
            withIntermediateDirectories: true
        )
        try writeValidUserDB(external, marker: "external-userdb")
        try FileManager.default.createSymbolicLink(
            at: primary.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true),
            withDestinationURL: external
        )
        try FileManager.default.createDirectory(at: fallbackParent, withIntermediateDirectories: true)
        let lease = try RuntimeStorageLease.resolve(
            primaryRoot: primary,
            fallbackParent: fallbackParent
        )
        XCTAssertTrue(lease.location.degraded)

        let original = lease.location.rootDirectory
        let displaced = fallbackParent.appendingPathComponent("displaced", isDirectory: true)
        try FileManager.default.moveItem(at: original, to: displaced)
        try FileManager.default.createDirectory(at: original, withIntermediateDirectories: false)
        let foreignSentinel = original.appendingPathComponent("foreign-owner")
        try Data("must-survive".utf8).write(to: foreignSentinel)

        lease.cleanup()
        XCTAssertEqual(try String(contentsOf: foreignSentinel, encoding: .utf8), "must-survive")
        XCTAssertTrue(FileManager.default.fileExists(atPath: displaced.path))
    }

    func testRuntimeStorageCleanupKeepsStableParentAfterParentPathReplacement() throws {
        let base = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-runtime-parent-replacement-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let primary = base.appendingPathComponent("primary", isDirectory: true)
        let fallbackParent = base.appendingPathComponent("fallback", isDirectory: true)
        let rejectedTarget = base.appendingPathComponent("rejected.userdb", isDirectory: true)
        let rejectedDatabase = primary.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
        try FileManager.default.createDirectory(
            at: rejectedDatabase.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try writeValidUserDB(rejectedTarget, marker: "rejected")
        try FileManager.default.createSymbolicLink(at: rejectedDatabase, withDestinationURL: rejectedTarget)
        try FileManager.default.createDirectory(at: fallbackParent, withIntermediateDirectories: true)

        let lease = try RuntimeStorageLease.resolve(
            primaryRoot: primary,
            fallbackParent: fallbackParent
        )
        XCTAssertTrue(lease.location.degraded)
        let leafName = lease.location.rootDirectory.lastPathComponent
        let displacedParent = base.appendingPathComponent("displaced-fallback", isDirectory: true)
        try FileManager.default.moveItem(at: fallbackParent, to: displacedParent)
        try FileManager.default.createDirectory(at: fallbackParent, withIntermediateDirectories: false)
        let replacementSentinel = fallbackParent.appendingPathComponent("foreign-sentinel")
        try Data("foreign-parent".utf8).write(to: replacementSentinel)
        let contentsBefore = try Data(contentsOf: replacementSentinel)
        let modeBefore = try posixMode(replacementSentinel)

        lease.cleanup()
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: displacedParent.appendingPathComponent(leafName).path
        ))
        XCTAssertEqual(try Data(contentsOf: replacementSentinel), contentsBefore)
        XCTAssertEqual(try posixMode(replacementSentinel), modeBefore)
    }

    func testRuntimeStorageCleanupRejectsMarkerSymlinkAndHardlink() throws {
        for fixture in ["marker-symlink", "marker-hardlink"] {
            let base = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
                .appendingPathComponent("agent-ime-runtime-\(fixture)-\(UUID().uuidString)", isDirectory: true)
            defer { try? FileManager.default.removeItem(at: base) }
            let primary = base.appendingPathComponent("primary", isDirectory: true)
            let fallbackParent = base.appendingPathComponent("fallback", isDirectory: true)
            let rejectedTarget = base.appendingPathComponent("rejected.userdb", isDirectory: true)
            let rejectedDatabase = primary.appendingPathComponent(
                "Rime/pinyin_simp.userdb", isDirectory: true
            )
            try FileManager.default.createDirectory(
                at: rejectedDatabase.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try writeValidUserDB(rejectedTarget, marker: "rejected")
            try FileManager.default.createSymbolicLink(
                at: rejectedDatabase,
                withDestinationURL: rejectedTarget
            )
            try FileManager.default.createDirectory(at: fallbackParent, withIntermediateDirectories: true)
            let lease = try RuntimeStorageLease.resolve(
                primaryRoot: primary,
                fallbackParent: fallbackParent
            )
            let root = lease.location.rootDirectory
            let marker = root.appendingPathComponent(".agent-ime-ephemeral-owner")
            let external = base.appendingPathComponent("external-marker")
            try Data("foreign-marker-must-survive".utf8).write(to: external)
            try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: external.path)
            try FileManager.default.removeItem(at: marker)
            if fixture == "marker-symlink" {
                try FileManager.default.createSymbolicLink(at: marker, withDestinationURL: external)
            } else {
                try FileManager.default.linkItem(at: external, to: marker)
            }
            let contentsBefore = try Data(contentsOf: external)
            let modeBefore = try posixMode(external)
            let linksBefore = try referenceCount(external)

            lease.cleanup()
            XCTAssertTrue(FileManager.default.fileExists(atPath: root.path))
            XCTAssertEqual(try Data(contentsOf: external), contentsBefore)
            XCTAssertEqual(try posixMode(external), modeBefore)
            XCTAssertEqual(try referenceCount(external), linksBefore)
        }
    }

    func testRuntimeStorageCleanupRejectsReplacedChildSymlinkAndHardlink() throws {
        for fixture in ["replaced-rime", "rime-symlink", "regular-hardlink"] {
            let base = FileManager.default.temporaryDirectory.appendingPathComponent(
                "agent-ime-runtime-storage-cleanup-\(fixture)-\(UUID().uuidString)",
                isDirectory: true
            )
            defer { try? FileManager.default.removeItem(at: base) }
            let primary = base.appendingPathComponent("primary", isDirectory: true)
            let rejectedDatabase = primary.appendingPathComponent(
                "Rime/pinyin_simp.userdb", isDirectory: true
            )
            let rejectedTarget = base.appendingPathComponent("rejected.userdb", isDirectory: true)
            let fallbackParent = base.appendingPathComponent("fallback", isDirectory: true)
            try FileManager.default.createDirectory(
                at: rejectedDatabase.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try writeValidUserDB(rejectedTarget, marker: "rejected-target")
            try FileManager.default.createSymbolicLink(
                at: rejectedDatabase,
                withDestinationURL: rejectedTarget
            )
            try FileManager.default.createDirectory(at: fallbackParent, withIntermediateDirectories: true)
            let lease = try RuntimeStorageLease.resolve(
                primaryRoot: primary,
                fallbackParent: fallbackParent
            )
            XCTAssertTrue(lease.location.degraded)
            let ephemeralRoot = lease.location.rootDirectory
            let external = base.appendingPathComponent("external-\(fixture)", isDirectory: true)
            let sentinel: URL

            switch fixture {
            case "replaced-rime":
                let displaced = fallbackParent.appendingPathComponent("owned-rime", isDirectory: true)
                try FileManager.default.moveItem(at: lease.location.userDirectory, to: displaced)
                try FileManager.default.createDirectory(
                    at: lease.location.userDirectory,
                    withIntermediateDirectories: false
                )
                sentinel = lease.location.userDirectory.appendingPathComponent("foreign-sentinel")
                try Data("foreign-directory".utf8).write(to: sentinel)
            case "rime-symlink":
                try FileManager.default.createDirectory(at: external, withIntermediateDirectories: true)
                sentinel = external.appendingPathComponent("foreign-sentinel")
                try Data("foreign-symlink-target".utf8).write(to: sentinel)
                try FileManager.default.removeItem(at: lease.location.userDirectory)
                try FileManager.default.createSymbolicLink(
                    at: lease.location.userDirectory,
                    withDestinationURL: external
                )
            default:
                sentinel = external
                try Data("foreign-hardlink-target".utf8).write(to: sentinel)
                try FileManager.default.linkItem(
                    at: sentinel,
                    to: lease.location.userDirectory.appendingPathComponent("foreign-hardlink")
                )
            }
            try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: sentinel.path)
            let contentsBefore = try Data(contentsOf: sentinel)
            let modeBefore = try posixMode(sentinel)
            let linksBefore = try referenceCount(sentinel)

            lease.cleanup()
            XCTAssertTrue(FileManager.default.fileExists(atPath: ephemeralRoot.path))
            XCTAssertEqual(try Data(contentsOf: sentinel), contentsBefore)
            XCTAssertEqual(try posixMode(sentinel), modeBefore)
            XCTAssertEqual(try referenceCount(sentinel), linksBefore)
        }
    }

    private func writeValidUserDB(_ database: URL, marker: String) throws {
        try FileManager.default.createDirectory(at: database, withIntermediateDirectories: true)
        try Data("MANIFEST-000001\n".utf8).write(to: database.appendingPathComponent("CURRENT"))
        try Data("fixture".utf8).write(to: database.appendingPathComponent("MANIFEST-000001"))
        try Data(marker.utf8).write(to: database.appendingPathComponent("TEST_MARKER"))
    }

    private func posixMode(_ url: URL) throws -> UInt16 {
        let value = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? NSNumber
        )
        return value.uint16Value
    }

    private func referenceCount(_ url: URL) throws -> UInt64 {
        let value = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: url.path)[.referenceCount] as? NSNumber
        )
        return value.uint64Value
    }

    private func ownerID(_ url: URL) throws -> uid_t {
        let value = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: url.path)[.ownerAccountID] as? NSNumber
        )
        return uid_t(value.uint32Value)
    }

    private func makePrivateDirectory(_ url: URL) throws {
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: url.path)
    }

    private func writePrivateFile(_ data: Data, to url: URL) throws {
        try data.write(to: url)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    private func snapshotFixture(version: UInt64, entry: String = "a\t甲\tfixture\t1") -> (Data, String, String) {
        let data = Data("AIMSNAP1\nversion=\(version)\nentry_count=1\n\(entry)\n".utf8)
        let checksum = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        return (data, checksum, "snapshot-\(version)-\(checksum).aimsnap")
    }

    func testDetailedDiagnosticsExpireAndRecoveryMarkerDetectsUncleanRun() throws {
        let suite = "agent-ime-diagnostics-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let directory = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-runtime-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let recorder = DiagnosticsRecorder(directory: directory.appendingPathComponent("logs"), defaults: defaults)
        let now = Date(timeIntervalSince1970: 1_000)
        recorder.enableDetailed(for: 60, now: now)
        XCTAssertTrue(recorder.isDetailedEnabled(at: now.addingTimeInterval(59)))
        XCTAssertFalse(recorder.isDetailedEnabled(at: now.addingTimeInterval(61)))

        let oldLogs = directory.appendingPathComponent("old-logs", isDirectory: true)
        try FileManager.default.createDirectory(at: oldLogs, withIntermediateDirectories: true)
        let oldFile = oldLogs.appendingPathComponent("events-old.jsonl")
        try Data("metadata-only\n".utf8).write(to: oldFile)
        try FileManager.default.setAttributes(
            [.modificationDate: Date().addingTimeInterval(-8 * 24 * 60 * 60)],
            ofItemAtPath: oldFile.path
        )
        let retentionRecorder = DiagnosticsRecorder(directory: oldLogs, defaults: defaults)
        XCTAssertFalse(FileManager.default.fileExists(atPath: oldFile.path))
        retentionRecorder.recordSystem(eventKind: "preview-fixture")
        retentionRecorder.flush()
        XCTAssertEqual(retentionRecorder.exportPreview().count, 1)
        retentionRecorder.clear()
        XCTAssertTrue(retentionRecorder.exportPreview().isEmpty)

        let marker = RuntimeRecoveryMarker(root: directory)
        XCTAssertFalse(try marker.start())
        let ownedMarkerURL = directory.appendingPathComponent("Runtime/running.marker")
        XCTAssertEqual(try posixMode(ownedMarkerURL), 0o600)
        XCTAssertEqual(try ownerID(ownedMarkerURL), geteuid())
        XCTAssertEqual(try posixMode(directory.appendingPathComponent("Runtime")), 0o700)
        XCTAssertEqual(try ownerID(directory.appendingPathComponent("Runtime")), geteuid())
        XCTAssertTrue(try marker.start())
        marker.stop()
        XCTAssertFalse(try marker.start())
        marker.stop()

        let failedRoot = directory.appendingPathComponent("failed-runtime-marker", isDirectory: true)
        let foreignMarker = failedRoot.appendingPathComponent("Runtime/running.marker", isDirectory: true)
        try FileManager.default.createDirectory(at: foreignMarker, withIntermediateDirectories: true)
        let foreignSentinel = foreignMarker.appendingPathComponent("foreign-owner")
        try Data("foreign-runtime-marker".utf8).write(to: foreignSentinel)
        let failedMarker = RuntimeRecoveryMarker(root: failedRoot)
        XCTAssertThrowsError(try failedMarker.start())
        failedMarker.stop()
        XCTAssertTrue(FileManager.default.fileExists(atPath: foreignMarker.path))
        XCTAssertEqual(try String(contentsOf: foreignSentinel, encoding: .utf8), "foreign-runtime-marker")

        let replacedRoot = directory.appendingPathComponent("replaced-runtime-marker", isDirectory: true)
        let replacedMarkerURL = replacedRoot.appendingPathComponent("Runtime/running.marker")
        let replacedMarker = RuntimeRecoveryMarker(root: replacedRoot)
        XCTAssertFalse(try replacedMarker.start())
        let foreignMarkerData = Data("replacement-owned-by-another-process".utf8)
        try foreignMarkerData.write(to: replacedMarkerURL, options: .atomic)
        replacedMarker.stop()
        XCTAssertEqual(try Data(contentsOf: replacedMarkerURL), foreignMarkerData)

        let replacedTreeRoot = directory.appendingPathComponent("replaced-runtime-tree", isDirectory: true)
        let replacedTreeURL = replacedTreeRoot.appendingPathComponent("Runtime/running.marker", isDirectory: true)
        let replacedTreeMarker = RuntimeRecoveryMarker(root: replacedTreeRoot)
        XCTAssertFalse(try replacedTreeMarker.start())
        try FileManager.default.removeItem(at: replacedTreeURL)
        try FileManager.default.createDirectory(at: replacedTreeURL, withIntermediateDirectories: false)
        let replacedTreeSentinel = replacedTreeURL.appendingPathComponent("foreign-tree-owner")
        try Data("foreign-runtime-tree".utf8).write(to: replacedTreeSentinel)
        replacedTreeMarker.stop()
        XCTAssertTrue(FileManager.default.fileExists(atPath: replacedTreeURL.path))
        XCTAssertEqual(try String(contentsOf: replacedTreeSentinel, encoding: .utf8), "foreign-runtime-tree")
    }

    func testRuntimeRecoveryRejectsMarkerLinksOversizeAndRuntimeSymlink() throws {
        for hardLink in [false, true] {
            let parent = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
                .appendingPathComponent("agent-ime-marker-link-\(UUID().uuidString)", isDirectory: true)
            defer { try? FileManager.default.removeItem(at: parent) }
            let root = parent.appendingPathComponent("Root", isDirectory: true)
            let runtime = root.appendingPathComponent("Runtime", isDirectory: true)
            try makePrivateDirectory(runtime)
            let sentinel = parent.appendingPathComponent("sentinel")
            try writePrivateFile(Data("marker-sentinel".utf8), to: sentinel)
            let markerURL = runtime.appendingPathComponent("running.marker")
            if hardLink {
                try FileManager.default.linkItem(at: sentinel, to: markerURL)
            } else {
                try FileManager.default.createSymbolicLink(at: markerURL, withDestinationURL: sentinel)
            }

            let marker = RuntimeRecoveryMarker(root: root)
            XCTAssertThrowsError(try marker.start())
            marker.stop()
            XCTAssertEqual(try Data(contentsOf: sentinel), Data("marker-sentinel".utf8))
            XCTAssertEqual(try posixMode(sentinel), 0o600)
            XCTAssertTrue(FileManager.default.fileExists(atPath: markerURL.path))
        }

        let oversizedParent = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-marker-oversize-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: oversizedParent) }
        let oversizedRoot = oversizedParent.appendingPathComponent("Root", isDirectory: true)
        let oversizedRuntime = oversizedRoot.appendingPathComponent("Runtime", isDirectory: true)
        try makePrivateDirectory(oversizedRuntime)
        let oversizedURL = oversizedRuntime.appendingPathComponent("running.marker")
        let oversizedData = Data(repeating: 0x61, count: 64 * 1024 + 1)
        try writePrivateFile(oversizedData, to: oversizedURL)
        let oversizedMarker = RuntimeRecoveryMarker(root: oversizedRoot)
        XCTAssertThrowsError(try oversizedMarker.start())
        oversizedMarker.stop()
        XCTAssertEqual(try Data(contentsOf: oversizedURL), oversizedData)
        XCTAssertEqual(try posixMode(oversizedURL), 0o600)

        let runtimeLinkParent = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-runtime-link-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: runtimeLinkParent) }
        let runtimeLinkRoot = runtimeLinkParent.appendingPathComponent("Root", isDirectory: true)
        let external = runtimeLinkParent.appendingPathComponent("external", isDirectory: true)
        try makePrivateDirectory(runtimeLinkRoot)
        try makePrivateDirectory(external)
        let sentinel = external.appendingPathComponent("sentinel")
        try writePrivateFile(Data("runtime-directory-sentinel".utf8), to: sentinel)
        try FileManager.default.createSymbolicLink(
            at: runtimeLinkRoot.appendingPathComponent("Runtime", isDirectory: true),
            withDestinationURL: external
        )
        let runtimeLinkMarker = RuntimeRecoveryMarker(root: runtimeLinkRoot)
        XCTAssertThrowsError(try runtimeLinkMarker.start())
        runtimeLinkMarker.stop()
        XCTAssertEqual(try Data(contentsOf: sentinel), Data("runtime-directory-sentinel".utf8))
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: external.appendingPathComponent("running.marker").path
        ))
    }

    func testUTF8ToUTF16CursorConversionHandlesChineseAndEmoji() {
        let text = "a中😀b"
        XCTAssertEqual(utf16CursorOffset(in: text, utf8Offset: 1), 1)
        XCTAssertEqual(utf16CursorOffset(in: text, utf8Offset: 4), 2)
        XCTAssertEqual(utf16CursorOffset(in: text, utf8Offset: 8), 4)
    }

    @MainActor
    func testCandidateOriginFollowsCaretScreenWithZeroWidthAnchor() {
        let primary = (
            frame: NSRect(x: 0, y: 0, width: 1920, height: 1080),
            visibleFrame: NSRect(x: 0, y: 25, width: 1920, height: 1030)
        )
        let secondary = (
            frame: NSRect(x: 1920, y: 0, width: 1920, height: 1080),
            visibleFrame: NSRect(x: 1920, y: 25, width: 1920, height: 1030)
        )
        // A bare insertion caret is zero-width and never intersects a screen
        // rect; the panel previously fell back to this process's main screen
        // and appeared on the wrong display.
        let origin = CandidatePanelController.origin(
            anchor: NSRect(x: 2400, y: 500, width: 0, height: 18),
            windowSize: NSSize(width: 400, height: 80),
            screenFrames: [primary, secondary]
        )
        XCTAssertGreaterThanOrEqual(origin.x, secondary.visibleFrame.minX)
        XCTAssertLessThanOrEqual(origin.x, secondary.visibleFrame.maxX - 400)
        XCTAssertGreaterThanOrEqual(origin.y, secondary.visibleFrame.minY)
        XCTAssertLessThanOrEqual(origin.y, secondary.visibleFrame.maxY - 80)
    }

    @MainActor
    func testCandidateOriginAvoidsVisibleFrameEdges() {
        let origin = CandidatePanelController.origin(
            anchor: NSRect(x: 980, y: 5, width: 10, height: 20),
            windowSize: NSSize(width: 400, height: 80),
            screens: []
        )
        XCTAssertTrue(origin.x.isFinite)
        XCTAssertTrue(origin.y.isFinite)
    }

    @MainActor
    func testCandidatePanelReportsAppliedCandidatesAndClearsOnHide() {
        let panel = CandidatePanelController(presentationEnabled: false)
        var selectedIndex: Int?
        panel.onSelect = { index, _ in selectedIndex = index }
        let disabledPresentationFrame = panel.frame
        let candidates = (0..<12).map {
            IMECandidate(text: "候选\($0)", comment: "houxuan")
        }
        panel.update(
            candidates: candidates,
            highlighted: 0,
            anchor: NSRect(x: 100, y: 100, width: 1, height: 20),
            secure: false
        )
        XCTAssertEqual(panel.appliedCandidateCount, 5)
        XCTAssertEqual(panel.allocatedCandidateButtonCount, 5)
        XCTAssertEqual(panel.hiddenCandidateButtonCount, 0)
        XCTAssertEqual(panel.visibleRowCountForTesting, 1)
        XCTAssertEqual(panel.frame, disabledPresentationFrame)
        XCTAssertTrue(panel.performCandidateActionForTesting(at: 0))
        XCTAssertEqual(selectedIndex, 0)
        XCTAssertFalse(panel.performCandidateActionForTesting(at: 10))
        for count in 0...5 {
            panel.update(
                candidates: Array(candidates.prefix(count)),
                highlighted: max(0, count - 1),
                anchor: NSRect(x: 100, y: 100, width: 1, height: 20),
                secure: false
            )
            XCTAssertEqual(panel.appliedCandidateCount, count)
            XCTAssertEqual(panel.allocatedCandidateButtonCount, 5)
            XCTAssertEqual(panel.hiddenCandidateButtonCount, 0)
            XCTAssertEqual(panel.enabledCandidateButtonCount, count)
            XCTAssertEqual(panel.frame, disabledPresentationFrame)
        }
        panel.hide()
        XCTAssertEqual(panel.appliedCandidateCount, 0)
        XCTAssertEqual(panel.frame, disabledPresentationFrame)
    }

    @MainActor
    func testSecondaryCandidatesUseSixThroughZeroHideMetadataAndRemainMouseSelectable() {
        let panel = CandidatePanelController(presentationEnabled: false, theme: .emergencySystem)
        var selectedIndex: Int?
        panel.onSelectSecondary = { index, _ in selectedIndex = index }
        panel.update(
            candidates: [IMECandidate(text: "拟哈", comment: "")],
            highlighted: 0,
            secondaryCandidates: [
                IMECandidate(text: "你好", comment: "纠错 · nihao", readingKey: "nihao")
            ],
            anchor: NSRect(x: 100, y: 100, width: 1, height: 20),
            secure: false
        )
        XCTAssertEqual(panel.visibleCandidateTitlesForTesting, ["1 拟哈"])
        XCTAssertEqual(panel.visibleSecondaryTitlesForTesting, ["6 你好"])
        XCTAssertFalse(panel.visibleSecondaryTitlesForTesting[0].contains("纠错"))
        XCTAssertEqual(panel.visibleRowCountForTesting, 2)
        XCTAssertTrue(panel.performSecondaryActionForTesting(at: 0))
        XCTAssertEqual(selectedIndex, 0)
    }

    @MainActor
    func testSecondaryRowIsDynamicByContentWidthAndCollapsesWhenEmpty() {
        let panel = CandidatePanelController(presentationEnabled: false)
        let long = (0..<5).map {
            IMECandidate(text: "第\($0)个很长很长的临时候选内容", comment: "纠错 · hidden")
        }
        panel.update(
            candidates: [IMECandidate(text: "你", comment: "")], highlighted: 0,
            secondaryCandidates: long,
            anchor: NSRect(x: 100, y: 100, width: 1, height: 20), secure: false
        )
        XCTAssertGreaterThan(panel.visibleSecondaryTitlesForTesting.count, 0)
        XCTAssertLessThan(panel.visibleSecondaryTitlesForTesting.count, 5)
        XCTAssertEqual(panel.visibleRowCountForTesting, 2)
        XCTAssertTrue(panel.visibleSecondaryTitlesForTesting.allSatisfy { !$0.contains("hidden") })

        panel.update(
            candidates: [IMECandidate(text: "你", comment: "")], highlighted: 0,
            secondaryCandidates: [],
            anchor: NSRect(x: 100, y: 100, width: 1, height: 20), secure: false
        )
        XCTAssertTrue(panel.visibleSecondaryTitlesForTesting.isEmpty)
        XCTAssertEqual(panel.visibleRowCountForTesting, 1)
    }

    @MainActor
    func testCandidateThemeSeparatesIndexTextAndPreservesVoiceOverAndOrder() {
        let panel = CandidatePanelController(presentationEnabled: false, theme: .emergencySystem)
        let candidates = ["你好", "你", "拟好", "年后", "霓虹"].map {
            IMECandidate(text: $0, comment: "", readingKey: "nihao")
        }
        panel.update(
            candidates: candidates,
            highlighted: 0,
            preedit: "nihao",
            anchor: NSRect(x: 100, y: 100, width: 1, height: 20),
            secure: false
        )

        XCTAssertEqual(panel.visibleCandidateIndicesForTesting, ["1", "2", "3", "4", "5"])
        XCTAssertEqual(panel.visibleCandidateTextsForTesting, candidates.map(\.text))
        XCTAssertEqual(
            panel.visibleCandidateTitlesForTesting,
            ["1 你好", "2 你", "3 拟好", "4 年后", "5 霓虹"]
        )
        XCTAssertEqual(
            panel.candidateAccessibilityLabelsForTesting,
            [
                "候选 1：你好", "候选 2：你", "候选 3：拟好",
                "候选 4：年后", "候选 5：霓虹",
            ]
        )
        XCTAssertEqual(panel.preeditAccessibilityLabelForTesting, "正在输入：nihao")
        XCTAssertEqual(panel.themeIdentifierForTesting, .system)
        let systemMetrics = panel.themeMetricsForTesting
        let systemSecondaryColor = panel.secondaryCandidateColorForTesting
        let systemHighlightFill = panel.highlightFillColorForTesting
        let systemHighlightedText = panel.highlightedCandidateColorForTesting

        panel.setTheme(.xuanmo)
        XCTAssertEqual(panel.themeIdentifierForTesting, .xuanmo)
        XCTAssertNotEqual(panel.themeMetricsForTesting, systemMetrics)
        XCTAssertNotEqual(panel.secondaryCandidateColorForTesting, systemSecondaryColor)
        XCTAssertNotEqual(panel.highlightFillColorForTesting, systemHighlightFill)
        XCTAssertNotEqual(panel.highlightedCandidateColorForTesting, systemHighlightedText)
        XCTAssertEqual(panel.visibleCandidateTextsForTesting, candidates.map(\.text))
        XCTAssertEqual(panel.highlightedCandidateTitleForTesting, "1 你好")
        XCTAssertEqual(
            panel.candidateAccessibilityLabelsForTesting.first,
            "候选 1：你好"
        )

        XCTAssertEqual(panel.preeditAccessibilityLabelForTesting, "正在输入：nihao")
    }

    @MainActor
    func testCandidateThemeSwitchLatchesUntilCompositionBoundary() {
        let panel = CandidatePanelController(presentationEnabled: false, theme: .emergencySystem)
        panel.requestTheme(.xuanmo, compositionActive: true)
        XCTAssertEqual(panel.themeIdentifierForTesting, .system)
        XCTAssertEqual(panel.pendingThemeIdentifierForTesting, .xuanmo)

        panel.update(
            candidates: [IMECandidate(text: "候选", comment: "")],
            highlighted: 0,
            anchor: NSRect(x: 100, y: 100, width: 1, height: 20),
            secure: false
        )
        XCTAssertEqual(panel.themeIdentifierForTesting, .system)
        panel.hide()
        XCTAssertEqual(panel.themeIdentifierForTesting, .xuanmo)
        XCTAssertNil(panel.pendingThemeIdentifierForTesting)

        panel.requestTheme(.shuitun, compositionActive: true)
        XCTAssertEqual(panel.themeIdentifierForTesting, .xuanmo)
        XCTAssertEqual(panel.pendingThemeIdentifierForTesting, .shuitun)
        panel.hide()
        XCTAssertEqual(panel.themeIdentifierForTesting, .shuitun)
        XCTAssertNil(panel.pendingThemeIdentifierForTesting)
    }

    @MainActor
    func testAllCatalogThemesKeepVoiceOverAndMultiScreenPlacementAcrossAppearances() {
        let screenFrames = [
            (frame: NSRect(x: 0, y: 0, width: 1512, height: 982),
             visibleFrame: NSRect(x: 0, y: 40, width: 1512, height: 918)),
            (frame: NSRect(x: 1512, y: 0, width: 2560, height: 1440),
             visibleFrame: NSRect(x: 1512, y: 24, width: 2560, height: 1376)),
        ]
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSAppearance(named: appearance)?.performAsCurrentDrawingAppearance {
                for identifier in CandidateThemeIdentifier.allCases {
                    let theme = CandidateTheme.resolve(identifier)
                    let panel = CandidatePanelController(presentationEnabled: false, theme: theme)
                    panel.update(
                        candidates: [IMECandidate(text: "你好", comment: "")],
                        highlighted: 0,
                        preedit: "nihao",
                        anchor: NSRect(x: 2000, y: 700, width: 1, height: 20),
                        secure: false
                    )
                    XCTAssertEqual(panel.candidateAccessibilityLabelsForTesting, ["候选 1：你好"])
                    XCTAssertEqual(panel.preeditAccessibilityLabelForTesting, "正在输入：nihao")
                    let origin = CandidatePanelController.origin(
                        anchor: NSRect(x: 2000, y: 700, width: 1, height: 20),
                        windowSize: NSSize(width: 420, height: 80),
                        screenFrames: screenFrames
                    )
                    XCTAssertGreaterThanOrEqual(origin.x, screenFrames[1].visibleFrame.minX)
                    XCTAssertLessThanOrEqual(origin.x + 420, screenFrames[1].visibleFrame.maxX)
                }
            }
        }
    }

    @MainActor
    func testCatalogContainsAllBuiltInSkinsAndSolidAndDynamicAppearanceContracts() throws {
        XCTAssertEqual(
            CandidateThemeIdentifier.allCases,
            [.system, .xuanmo, .shuitun, .youzi, .juzimao, .chaiquan, .baitu, .yueye, .luoying, .fenghong, .denglong, .songxue, .nihong]
        )
        XCTAssertEqual(CandidateSkinCatalog.displayNames[.system]?.zh, "简洁")
        XCTAssertEqual(CandidateSkinCatalog.displayNames[.xuanmo]?.zh, "玄墨")

        func rgba(_ color: NSColor, appearance: NSAppearance.Name) throws -> [CGFloat] {
            var resolved: NSColor?
            NSAppearance(named: appearance)?.performAsCurrentDrawingAppearance {
                resolved = color.usingColorSpace(.sRGB)
            }
            let value = try XCTUnwrap(resolved)
            return [value.redComponent, value.greenComponent, value.blueComponent, value.alphaComponent]
        }

        for identifier in [CandidateThemeIdentifier.xuanmo] {
            let background = try XCTUnwrap(CandidateTheme.resolve(identifier).panelBackground)
            XCTAssertEqual(
                try rgba(background, appearance: .aqua),
                try rgba(background, appearance: .darkAqua)
            )
            var renderedBackgrounds: [[CGFloat]] = []
            for appearance in [NSAppearance.Name.aqua, .darkAqua] {
                NSAppearance(named: appearance)?.performAsCurrentDrawingAppearance {
                    let panel = CandidatePanelController(
                        presentationEnabled: false,
                        theme: CandidateTheme.resolve(identifier)
                    )
                    if let layerBackground = panel.panelLayerBackgroundForTesting,
                       let components = try? rgba(layerBackground, appearance: appearance) {
                        renderedBackgrounds.append(components)
                    }
                }
            }
            XCTAssertEqual(renderedBackgrounds.count, 2)
            XCTAssertEqual(renderedBackgrounds[0], renderedBackgrounds[1])
        }
        // The default skin follows the system appearance: no fixed background, an accent that differs between light and dark.
        XCTAssertNil(CandidateTheme.system.panelBackground)
        XCTAssertNotEqual(
            try rgba(CandidateTheme.system.highlightedCandidateColor, appearance: .aqua),
            try rgba(CandidateTheme.system.highlightedCandidateColor, appearance: .darkAqua)
        )

        let xuanmoPanel = CandidatePanelController(
            presentationEnabled: false,
            theme: .xuanmo
        )
        XCTAssertNotNil(xuanmoPanel.panelBackgroundForTesting)
        XCTAssertEqual(xuanmoPanel.panelBorderWidthForTesting, 0.5)
        XCTAssertTrue(xuanmoPanel.panelHasShadowForTesting)
    }

    @MainActor
    func testDefaultAndInkSkinValues() {
        // The default: the clean glass card with a pinyin row, a bare-number-with-dot index and room on the right for the mark.
        let system = CandidateTheme.system
        XCTAssertEqual(system.candidateFont.pointSize, 16)
        XCTAssertEqual(system.indexFont.pointSize, 10)
        XCTAssertEqual(system.rowHeight, 28)
        XCTAssertEqual(system.capsuleCornerRadius, 6)
        XCTAssertEqual(system.panelCornerRadius, 12)
        XCTAssertEqual(system.edgeInsets.right, 46)
        XCTAssertTrue(system.preeditRow)
        XCTAssertEqual(system.indexSuffix, ".")
        XCTAssertTrue(system.usesGlass)
        XCTAssertNil(system.panelBackground)
        XCTAssertTrue(system.shadowEnabled)
        XCTAssertGreaterThan(system.highlightFillColor.alphaComponent, 0.05, "the first candidate sits on a soft plate; a bare coloured word looked odd (owner 2026-10-15)")

        // The dark one: a fixed ink card with a hairline border, the same layout and mark.
        let ink = CandidateTheme.xuanmo
        XCTAssertEqual(ink.rowHeight, system.rowHeight)
        XCTAssertEqual(ink.edgeInsets.right, 46)
        XCTAssertNotNil(ink.panelBackground)
        XCTAssertEqual(ink.panelBorderWidth, 0.5)
        XCTAssertTrue(ink.preeditRow)
    }

    @MainActor
    func testSameIdentifierDifferentThemeValuesRefreshPanelAndCandidateSlots() {
        let original = CandidateTheme.emergencySystem
        let changed = CandidateTheme(
            identifier: original.identifier,
            candidateFont: .systemFont(ofSize: 19, weight: .bold),
            indexFont: original.indexFont,
            candidateColor: original.candidateColor,
            secondaryCandidateColor: original.secondaryCandidateColor,
            indexColor: original.indexColor,
            highlightedCandidateColor: original.highlightedCandidateColor,
            highlightFillColor: original.highlightFillColor,
            highlightBorderColor: original.highlightBorderColor,
            rowHeight: original.rowHeight + 2,
            horizontalPadding: original.horizontalPadding,
            verticalPadding: original.verticalPadding,
            indexCandidateSpacing: original.indexCandidateSpacing,
            candidateSpacing: original.candidateSpacing + 2,
            rowSpacing: original.rowSpacing,
            capsuleCornerRadius: original.capsuleCornerRadius,
            panelCornerRadius: original.panelCornerRadius,
            edgeInsets: original.edgeInsets,
            material: original.material,
            minimumPanelSize: original.minimumPanelSize,
            maximumPanelWidth: original.maximumPanelWidth,
            widthQuantization: original.widthQuantization,
            panelBackground: original.panelBackground,
            panelBorderColor: original.panelBorderColor,
            panelBorderWidth: original.panelBorderWidth,
            shadowEnabled: original.shadowEnabled
        )
        let panel = CandidatePanelController(presentationEnabled: false, theme: original)
        XCTAssertEqual(panel.candidateFontSizeForTesting, 17)
        panel.setTheme(changed)
        XCTAssertEqual(panel.themeIdentifierForTesting, .system)
        XCTAssertEqual(panel.themeMetricsForTesting[0], original.rowHeight + 2)
        XCTAssertEqual(panel.candidateFontSizeForTesting, 19)
    }

    @MainActor
    func testMinimalExternalSkinLoadsWithoutBuildAndLatchesAtCompositionBoundary() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("candidate-skin-minimal-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let package = root.appendingPathComponent("owner-red", isDirectory: true)
        try FileManager.default.createDirectory(at: package, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: package.path)
        let document = Data("""
        {
          "id":"owner-red",
          "displayName":{"zh":"Owner 红","en":"Owner Red"},
          "formatVersion":1,
          "author":"Owner",
          "description":"minimal package",
          "highlightedCandidateColor":{"light":"#FF0000","dark":"#00FF00"},
          "highlightFillColor":"system:clear",
          "highlightBorderColor":"system:clear"
        }
        """.utf8)
        let skinJSON = package.appendingPathComponent("skin.json")
        try document.write(to: skinJSON)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: skinJSON.path)

        let identifier = try XCTUnwrap(CandidateThemeIdentifier(rawValue: "owner-red"))
        let external = CandidateSkinCatalog.theme(for: identifier, externalRoot: root)
        XCTAssertEqual(external.identifier, identifier)
        XCTAssertEqual(external.rowHeight, CandidateTheme.system.rowHeight)
        XCTAssertEqual(external.candidateSpacing, CandidateTheme.system.candidateSpacing)
        XCTAssertEqual(external.highlightFillColor, .clear)

        let panel = CandidatePanelController(presentationEnabled: false, theme: .emergencySystem)
        panel.requestResolvedTheme(external, compositionActive: true)
        XCTAssertEqual(panel.themeIdentifierForTesting, .system)
        XCTAssertEqual(panel.pendingThemeIdentifierForTesting, identifier)
        panel.hide()
        XCTAssertEqual(panel.themeIdentifierForTesting, identifier)
        XCTAssertNil(panel.pendingThemeIdentifierForTesting)
    }

    @MainActor
    func testBadExternalSkinsAlwaysReturnUsableSystemTheme() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("candidate-skin-bad-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)

        func package(_ id: String, data: Data) throws -> URL {
            let directory = root.appendingPathComponent(id, isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
            let file = directory.appendingPathComponent("skin.json")
            try data.write(to: file)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
            return directory
        }

        _ = try package("bad-json", data: Data("{not-json".utf8))
        _ = try package(
            "missing-required",
            data: Data(#"{"id":"missing-required","formatVersion":1}"#.utf8)
        )
        _ = try package(
            "oversized",
            data: Data(repeating: 0x20, count: CandidateSkinPackageLimits.maximumSkinJSONBytes + 1)
        )
        let linked = root.appendingPathComponent("linked", isDirectory: true)
        try FileManager.default.createDirectory(at: linked, withIntermediateDirectories: false)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: linked.path)
        try FileManager.default.createSymbolicLink(
            at: linked.appendingPathComponent("skin.json"),
            withDestinationURL: root.appendingPathComponent("outside.json")
        )

        for rawID in ["bad-json", "missing-required", "oversized", "linked"] {
            let identifier = try XCTUnwrap(CandidateThemeIdentifier(rawValue: rawID))
            let resolved = CandidateSkinCatalog.theme(for: identifier, externalRoot: root)
            XCTAssertEqual(resolved.identifier, .system, rawID)
            let panel = CandidatePanelController(presentationEnabled: false, theme: resolved)
            panel.update(
                candidates: [IMECandidate(text: "正常出字", comment: "")],
                highlighted: 0,
                anchor: NSRect(x: 100, y: 100, width: 1, height: 20),
                secure: false
            )
            XCTAssertEqual(panel.visibleCandidateTextsForTesting, ["正常出字"], rawID)
        }
    }

    @MainActor
    func testExternalBuiltInIDConflictKeepsBuiltInPackage() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("candidate-skin-conflict-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let package = root.appendingPathComponent("system", isDirectory: true)
        try FileManager.default.createDirectory(at: package, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: package.path)
        try Data("""
        {"id":"system","displayName":{"zh":"冲突","en":"Conflict"},"formatVersion":1,
         "author":"External","description":"must lose","rowHeight":99}
        """.utf8).write(to: package.appendingPathComponent("skin.json"))
        let resolved = CandidateSkinCatalog.theme(for: .system, externalRoot: root)
        XCTAssertEqual(resolved.identifier, .system)
        XCTAssertEqual(resolved.rowHeight, 28, "the built-in package wins; the external 99 is ignored")
    }

    @MainActor
    func testCandidatePanelGeometryKeepsSmallChangesShrinksLargeOnesAndResetsAfterAPause() {
        let panel = CandidatePanelController(presentationEnabled: false, theme: .emergencySystem)
        let n = panel.resolvePanelSizeForTesting(fitting: NSSize(width: 310, height: 62))
        let ni = panel.resolvePanelSizeForTesting(fitting: NSSize(width: 505, height: 70))
        let nih = panel.resolvePanelSizeForTesting(fitting: NSSize(width: 420, height: 64))
        let nihao = panel.resolvePanelSizeForTesting(fitting: NSSize(width: 690, height: 92))

        // Grows at once; a slightly narrower page (420 of 520) keeps the size.
        XCTAssertEqual([n.width, ni.width, nih.width, nihao.width], [320, 520, 520, 720])
        XCTAssertEqual([n.height, ni.height, nih.height, nihao.height], [62, 70, 70, 92])

        // The next word typed right away keeps a similar size (no jump)...
        panel.hide()
        panel.expireGeometryForTesting(afterIdleSeconds: 0.5)
        let similar = panel.resolvePanelSizeForTesting(fitting: NSSize(width: 600, height: 90))
        XCTAssertEqual(similar, NSSize(width: 720, height: 92))
        // ...but a much shorter one shrinks instead of inheriting the widest panel.
        panel.hide()
        panel.expireGeometryForTesting(afterIdleSeconds: 0.5)
        let short = panel.resolvePanelSizeForTesting(fitting: NSSize(width: 300, height: 60))
        XCTAssertEqual(short, NSSize(width: 320, height: 60))

        // After a pause the panel starts fresh.
        panel.hide()
        panel.expireGeometryForTesting(afterIdleSeconds: 2)
        let nextComposition = panel.resolvePanelSizeForTesting(
            fitting: NSSize(width: 300, height: 60)
        )
        XCTAssertEqual(nextComposition, NSSize(width: 320, height: 60))
    }

    @MainActor
    func testCandidatePanelRendersTopAndHighlightedZanCandidatesInVisibleSlots() {
        let panel = CandidatePanelController(presentationEnabled: false, theme: .emergencySystem)
        panel.update(
            candidates: ["咱", "攒", "张", "站", "涨"].map {
                IMECandidate(text: $0, comment: "", readingKey: "zan")
            },
            highlighted: 0,
            anchor: NSRect(x: 100, y: 100, width: 1, height: 20),
            secure: false
        )
        XCTAssertEqual(
            panel.visibleCandidateTitlesForTesting,
            ["1 咱", "2 攒", "3 张", "4 站", "5 涨"]
        )
        XCTAssertEqual(panel.highlightedCandidateTitleForTesting, "1 咱")
        XCTAssertFalse(panel.visibleCandidateTitlesForTesting[0].isEmpty)
    }

    func testZanWithFuzzyGroupsKeepsExactCommonCharactersOnFirstPage() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(
            sharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "test.zan-exact-priority"
        ))
        defer {
            _ = session.setFuzzyMask(0)
            session.close()
        }
        XCTAssertTrue(session.setFuzzyMask(231))
        var result: IMEViewModel?
        for character in "zan" {
            result = session.process(
                kind: .text,
                text: String(character),
                security: .nonSecure
            )
        }
        let candidates = try XCTUnwrap(result).candidates.map(\.text)
        XCTAssertEqual(candidates.first, "咱")
        XCTAssertTrue(candidates.prefix(6).contains("攒"), "candidates=\(candidates)")
        XCTAssertFalse(candidates[0].isEmpty)
        let selection = try XCTUnwrap(session.process(
            kind: .text,
            text: "4",
            security: .nonSecure,
            deliveryID: 0
        ))
        XCTAssertEqual(selection.commitText, "攒")
    }

    func testRealRimeLongReadingPrioritizesFirstSyllableSinglesOnlyFromThirdPage() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(
            sharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "test.deep-page-fallback"
        ))
        defer { session.close() }

        var first: IMEViewModel?
        for character in "woxiangchifan" {
            first = session.process(
                kind: .text, text: String(character), security: .nonSecure
            )
        }
        let page0 = try XCTUnwrap(first)
        let page1 = try XCTUnwrap(session.process(kind: .pageDown, security: .nonSecure))
        let page2 = try XCTUnwrap(session.process(kind: .pageDown, security: .nonSecure))
        XCTAssertEqual(page0.pageNumber, 0)
        XCTAssertEqual(page1.pageNumber, 1)
        XCTAssertEqual(page2.pageNumber, 2)
        XCTAssertEqual(page0.candidates.count, 5)
        XCTAssertEqual(page1.candidates.count, 5)
        XCTAssertEqual(page2.candidates.count, 5, "fallback must not change panel geometry")
        XCTAssertTrue(
            page2.candidates.prefix(3).allSatisfy { $0.text.count == 1 },
            "page2=\(page2.candidates.map(\.text))"
        )
        let firstTwo = page0.candidates.map(\.text) + page1.candidates.map(\.text)
        XCTAssertEqual(firstTwo.count, Set(firstTwo).count)
        XCTAssertTrue(Set(firstTwo).isDisjoint(with: page2.candidates.map(\.text)))

        let committed = try XCTUnwrap(session.process(
            kind: .text, text: "1", security: .nonSecure, deliveryID: 0xD3_0001
        ))
        XCTAssertTrue(committed.consumed)
        XCTAssertTrue(
            committed.commitText == page2.candidates[0].text || !committed.markedText.isEmpty,
            "single-character selection must either commit or confirm a real librime segment"
        )
    }

    func testPart05FuzzyKeepsBaseEraseRulesAndRealZToZhMatches() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(
            sharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "test.part05-fuzzy-regression"
        ))
        defer {
            _ = session.setFuzzyMask(0)
            session.close()
        }
        XCTAssertEqual(session.setFuzzyMaskStatus(1), 0)

        func candidates(for query: String) throws -> [String] {
            _ = session.process(kind: .escape, security: .nonSecure)
            var result: IMEViewModel?
            for character in query {
                result = session.process(
                    kind: .text,
                    text: String(character),
                    security: .nonSecure
                )
            }
            return try XCTUnwrap(result, "query=\(query)").candidates.map(\.text)
        }

        XCTAssertTrue(try candidates(for: "zege").contains("这个"))
        XCTAssertTrue(try candidates(for: "zenm").contains("怎么"))
        let degenerate = try ["m", "n", "ng", "hm", "hng"].flatMap(candidates(for:))
        XCTAssertTrue(Set(degenerate).isDisjoint(with: ["呒", "嗯", "哼"]))
    }

    @MainActor
    func testCandidatePanelKeepsStableConstraintsAcrossHighFrequencyUpdates() {
        let panel = CandidatePanelController(presentationEnabled: false)
        let candidates = (0..<5).map {
            IMECandidate(text: "候选\($0)", comment: "houxuan")
        }
        for iteration in 0..<20_000 {
            autoreleasepool {
                let count = iteration % 6
                panel.update(
                    candidates: Array(candidates.prefix(count)),
                    highlighted: max(0, count - 1),
                    pageNumber: iteration % 3,
                    isLastPage: !iteration.isMultiple(of: 2),
                    secondaryCandidates: iteration.isMultiple(of: 3)
                        ? [IMECandidate(text: "本地候选", comment: "纠错 · bendicixuan")]
                        : [],
                    anchor: NSRect(x: 100, y: 100, width: 1, height: 20),
                    secure: iteration.isMultiple(of: 11)
                )
            }
        }
        panel.update(
            candidates: [], highlighted: 0, secondaryCandidates: [],
            anchor: NSRect(x: 100, y: 100, width: 1, height: 20), secure: false
        )
        XCTAssertEqual(panel.allocatedCandidateButtonCount, 5)
        XCTAssertEqual(panel.hiddenCandidateButtonCount, 0)
        XCTAssertEqual(panel.appliedCandidateCount, 0)
        XCTAssertEqual(panel.enabledCandidateButtonCount, 0)
        XCTAssertEqual(panel.visibleRowCountForTesting, 1)
    }

    func testCoreABIVersionAndSessionLifecycle() throws {
        XCTAssertEqual(aim_core_abi_version(), 6)
        XCTAssertEqual(aim_live_session_count(), 0)
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(
            sharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "test.explicit-close"
        ))
        XCTAssertEqual(aim_live_session_count(), 1)
        session.close()
        session.close()
        XCTAssertEqual(aim_live_session_count(), 0)
    }

    func testContextProfileConsumesHostPrecedingScriptWithoutPersistingSecureText() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(
            sharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.apple.dt.Xcode"
        ))
        defer { session.close() }

        XCTAssertTrue(session.setContextProfile(appClassOverride: 0, punctuationPolicy: 0))
        XCTAssertTrue(session.setPrecedingText("敏感"))
        let secure = try XCTUnwrap(session.process(kind: .text, text: ",", security: .secure))
        XCTAssertTrue(secure.secureDegraded)
        XCTAssertFalse(secure.consumed)
        let afterSecure = try XCTUnwrap(session.process(kind: .text, text: ",", security: .nonSecure))
        XCTAssertFalse(afterSecure.consumed, "secure preceding text must be discarded")

        XCTAssertTrue(session.setPrecedingText("中文"))
        XCTAssertEqual(
            session.process(kind: .text, text: ",", security: .nonSecure)?.commitText,
            "，"
        )
        XCTAssertTrue(session.setPrecedingText("ASCII"))
        let ascii = try XCTUnwrap(session.process(kind: .text, text: ".", security: .nonSecure))
        XCTAssertFalse(ascii.consumed)
        XCTAssertEqual(ascii.commitText, "")
    }

    @MainActor
    func testControllerCapturesApprovedThirtyTwoUnitHostContextForCompositionAndPunctuation() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let chinese = try XCTUnwrap(InputControllerPerformanceSession(
            sharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.apple.dt.Xcode"
        ))
        chinese.seedDocumentTextForTesting("中文")
        let comma = try XCTUnwrap(chinese.processText(","))
        XCTAssertTrue(comma.consumed)
        XCTAssertEqual(comma.commitText, "，")
        chinese.close()

        let composition = try XCTUnwrap(InputControllerPerformanceSession(
            sharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.apple.TextEdit"
        ))
        composition.seedDocumentTextForTesting("0123456789abcdefghijkl")
        _ = composition.processText("n")
        _ = composition.processText("i")
        XCTAssertEqual(
            composition.attributedSubstringRangesForTesting,
            [NSRange(location: 0, length: 22)],
            "context must be read once at the composition boundary with the owner-frozen window"
        )
        composition.close()

        let ascii = try XCTUnwrap(InputControllerPerformanceSession(
            sharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.apple.TextEdit"
        ))
        ascii.seedDocumentTextForTesting("v1")
        let period = try XCTUnwrap(ascii.processText("."))
        XCTAssertFalse(period.consumed)
        XCTAssertEqual(period.commitText, "")
        ascii.close()
    }

    @MainActor
    func testAppActivationWaitsForCurrentCompositionBoundary() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(InputControllerPerformanceSession(
            sharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.apple.dt.Xcode"
        ))
        defer { session.close() }

        XCTAssertFalse(try XCTUnwrap(session.processText("n")).markedText.isEmpty)
        session.activateAppForTesting("com.apple.TextEdit")
        XCTAssertEqual(session.activeAppIdentifierForTesting, "com.apple.dt.Xcode")
        _ = session.processText(",")
        XCTAssertEqual(session.activeAppIdentifierForTesting, "com.apple.TextEdit")
    }

    func testDiagnosticsContainNoInputCandidateOrSecretText() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-diagnostics-\(UUID().uuidString)", isDirectory: true)
        let recorder = DiagnosticsRecorder(directory: directory)
        let result = IMEViewModel(
            status: 0, consumed: true, directEnglish: false, secureDegraded: false,
            markedText: "sensitive-composition", markedCursorUTF8: 4,
            commitText: "sensitive-commit", candidates: [IMECandidate(text: "secret-candidate", comment: "")],
            highlightedIndex: 0, pageNumber: 0, isLastPage: true,
            selectionEventID: 42, coreElapsedNanoseconds: 100
        )
        recorder.record(eventKind: "text", result: result, totalElapsedNanoseconds: 200, clientIdentifier: "secret.app")
        recorder.record(eventKind: "text", result: result, totalElapsedNanoseconds: 200, clientIdentifier: "secret.app")
        recorder.flush()
        let restartedRecorder = DiagnosticsRecorder(directory: directory)
        restartedRecorder.record(
            eventKind: "text-retry", result: result,
            totalElapsedNanoseconds: 300, clientIdentifier: "secret.app"
        )
        restartedRecorder.flush()
        let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
        let eventFiles = files.filter { $0.pathExtension == "jsonl" }
        let directoryMode = try FileManager.default.attributesOfItem(atPath: directory.path)[.posixPermissions] as? NSNumber
        XCTAssertEqual(directoryMode?.intValue, 0o700)
        for file in files {
            let mode = try FileManager.default.attributesOfItem(atPath: file.path)[.posixPermissions] as? NSNumber
            let linkCount = try FileManager.default.attributesOfItem(atPath: file.path)[.referenceCount] as? NSNumber
            XCTAssertEqual(mode?.intValue, 0o600)
            XCTAssertEqual(linkCount?.intValue, 1)
        }
        let allContents = try files.map { try String(contentsOf: $0, encoding: .utf8) }.joined()
        let eventContents = try eventFiles.map { try String(contentsOf: $0, encoding: .utf8) }.joined()
        XCTAssertFalse(allContents.contains("sensitive"))
        XCTAssertFalse(allContents.contains("secret-candidate"))
        XCTAssertFalse(allContents.contains("secret.app"))
        XCTAssertEqual(
            eventContents.split(separator: "\n").count, 1,
            "stable selection event ID must deduplicate retries across recorder/process restarts"
        )
        restartedRecorder.clear()
        XCTAssertTrue(restartedRecorder.exportPreview().isEmpty)

        let afterClearRecorder = DiagnosticsRecorder(directory: directory)
        afterClearRecorder.record(
            eventKind: "text-after-clear", result: result,
            totalElapsedNanoseconds: 400, clientIdentifier: "secret.app"
        )
        afterClearRecorder.flush()
        let afterClearFiles = try FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil
        )
        let afterClearContents = try afterClearFiles.filter { $0.pathExtension == "jsonl" }.map {
            try String(contentsOf: $0, encoding: .utf8)
        }.joined()
        XCTAssertEqual(afterClearContents.split(separator: "\n").count, 1)
        afterClearRecorder.clear()
        try? FileManager.default.removeItem(at: directory)
    }

    func testDiagnosticsRejectFinalDirectorySymlinkAndNonDirectoryWithoutMutatingSentinel() throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-diagnostics-root-boundary-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: base) }
        try FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)

        let externalDirectory = base.appendingPathComponent("external", isDirectory: true)
        try FileManager.default.createDirectory(at: externalDirectory, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o750], ofItemAtPath: externalDirectory.path)
        let sentinel = externalDirectory.appendingPathComponent("sentinel")
        let sentinelData = Data("external-diagnostics-sentinel".utf8)
        try sentinelData.write(to: sentinel)
        try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: sentinel.path)
        let externalModeBefore = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: externalDirectory.path)[.posixPermissions] as? NSNumber
        )
        let sentinelModeBefore = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: sentinel.path)[.posixPermissions] as? NSNumber
        )

        let linkedDirectory = base.appendingPathComponent("linked-logs", isDirectory: true)
        try FileManager.default.createSymbolicLink(at: linkedDirectory, withDestinationURL: externalDirectory)
        let linkedRecorder = DiagnosticsRecorder(directory: linkedDirectory)
        linkedRecorder.recordSystem(eventKind: "must-not-follow-directory-link")
        linkedRecorder.flush()
        linkedRecorder.clear()
        XCTAssertTrue(linkedRecorder.exportPreview().isEmpty)
        XCTAssertEqual(try Data(contentsOf: sentinel), sentinelData)
        XCTAssertEqual(
            try XCTUnwrap(
                FileManager.default.attributesOfItem(atPath: externalDirectory.path)[.posixPermissions] as? NSNumber
            ).uint16Value,
            externalModeBefore.uint16Value
        )
        XCTAssertEqual(
            try XCTUnwrap(
                FileManager.default.attributesOfItem(atPath: sentinel.path)[.posixPermissions] as? NSNumber
            ).uint16Value,
            sentinelModeBefore.uint16Value
        )
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: externalDirectory.path), ["sentinel"])

        let fileAtDirectoryPath = base.appendingPathComponent("file-instead-of-logs")
        let fileSentinel = Data("non-directory-sentinel".utf8)
        try fileSentinel.write(to: fileAtDirectoryPath)
        try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: fileAtDirectoryPath.path)
        let fileModeBefore = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: fileAtDirectoryPath.path)[.posixPermissions] as? NSNumber
        )
        let fileRecorder = DiagnosticsRecorder(directory: fileAtDirectoryPath)
        fileRecorder.recordSystem(eventKind: "must-not-open-non-directory")
        fileRecorder.flush()
        fileRecorder.clear()
        XCTAssertTrue(fileRecorder.exportPreview().isEmpty)
        XCTAssertEqual(try Data(contentsOf: fileAtDirectoryPath), fileSentinel)
        XCTAssertEqual(
            try XCTUnwrap(
                FileManager.default.attributesOfItem(atPath: fileAtDirectoryPath.path)[.posixPermissions] as? NSNumber
            ).uint16Value,
            fileModeBefore.uint16Value
        )
    }

    func testDiagnosticsRejectSymlinkedAndHardLinkedControlFilesWithoutMutatingSentinel() throws {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        let eventFileName = "events-\(formatter.string(from: Date())).jsonl"
        let leafNames = [
            ".selection-idempotency.lock",
            ".selection-idempotency.json",
            ".selection-idempotency.pending",
            eventFileName,
        ]
        let result = IMEViewModel(
            status: 0, consumed: true, directEnglish: false, secureDegraded: false,
            markedText: "", markedCursorUTF8: 0, commitText: "", candidates: [],
            highlightedIndex: 0, pageNumber: 0, isLastPage: true,
            selectionEventID: 9_001, coreElapsedNanoseconds: 0
        )

        for linkKind in ["symlink", "hardlink"] {
            for leafName in leafNames {
                let base = FileManager.default.temporaryDirectory.appendingPathComponent(
                    "agent-ime-diagnostics-leaf-boundary-\(linkKind)-\(UUID().uuidString)",
                    isDirectory: true
                )
                defer { try? FileManager.default.removeItem(at: base) }
                let logs = base.appendingPathComponent("logs", isDirectory: true)
                try FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true)
                try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: logs.path)
                let external = base.appendingPathComponent("external-sentinel")
                let sentinelData = Data("diagnostics-\(linkKind)-\(leafName)".utf8)
                try sentinelData.write(to: external)
                try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: external.path)
                let externalModeBefore = try XCTUnwrap(
                    FileManager.default.attributesOfItem(atPath: external.path)[.posixPermissions] as? NSNumber
                )
                let attackedLeaf = logs.appendingPathComponent(leafName)
                if linkKind == "symlink" {
                    try FileManager.default.createSymbolicLink(at: attackedLeaf, withDestinationURL: external)
                } else {
                    try FileManager.default.linkItem(at: external, to: attackedLeaf)
                }
                let externalLinksBefore = try XCTUnwrap(
                    FileManager.default.attributesOfItem(atPath: external.path)[.referenceCount] as? NSNumber
                )

                let recorder = DiagnosticsRecorder(directory: logs)
                recorder.record(
                    eventKind: "selection", result: result,
                    totalElapsedNanoseconds: 0, clientIdentifier: "private.test"
                )
                recorder.flush()
                recorder.clear()

                XCTAssertEqual(try Data(contentsOf: external), sentinelData, "\(linkKind) \(leafName)")
                XCTAssertEqual(
                    try XCTUnwrap(
                        FileManager.default.attributesOfItem(atPath: external.path)[.posixPermissions] as? NSNumber
                    ).uint16Value,
                    externalModeBefore.uint16Value,
                    "\(linkKind) \(leafName)"
                )
                XCTAssertEqual(
                    try XCTUnwrap(
                        FileManager.default.attributesOfItem(atPath: external.path)[.referenceCount] as? NSNumber
                    ).uint64Value,
                    externalLinksBefore.uint64Value,
                    "\(linkKind) \(leafName)"
                )
                XCTAssertNotNil(
                    try? FileManager.default.attributesOfItem(atPath: attackedLeaf.path),
                    "invalid control leaf must be preserved fail-closed"
                )
            }
        }
    }

    func testDiagnosticsRestoreUInt64SelectionIDWithoutPrecisionLoss() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-diagnostics-uint64-\(UUID().uuidString)", isDirectory: true)
        let identifier = UInt64.max - 58
        let result = IMEViewModel(
            status: 0, consumed: true, directEnglish: false, secureDegraded: false,
            markedText: "", markedCursorUTF8: 0, commitText: "", candidates: [],
            highlightedIndex: 0, pageNumber: 0, isLastPage: true,
            selectionEventID: identifier, coreElapsedNanoseconds: 0
        )
        let recorder = DiagnosticsRecorder(directory: directory)
        recorder.record(eventKind: "selection", result: result, totalElapsedNanoseconds: 0, clientIdentifier: "test")
        recorder.flush()

        let restartedRecorder = DiagnosticsRecorder(directory: directory)
        restartedRecorder.record(
            eventKind: "selection-retry", result: result,
            totalElapsedNanoseconds: 0, clientIdentifier: "test"
        )
        restartedRecorder.flush()

        let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "jsonl" }
        let contents = try files.map { try String(contentsOf: $0, encoding: .utf8) }.joined()
        XCTAssertEqual(contents.split(separator: "\n").count, 1)
        XCTAssertTrue(contents.contains(String(identifier)))
        restartedRecorder.clear()
        try? FileManager.default.removeItem(at: directory)
    }

    func testDiagnosticsClearResetsSelectionIDsWhenDirectoryIsMissing() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-diagnostics-missing-\(UUID().uuidString)", isDirectory: true)
        let result = IMEViewModel(
            status: 0, consumed: true, directEnglish: false, secureDegraded: false,
            markedText: "", markedCursorUTF8: 0, commitText: "", candidates: [],
            highlightedIndex: 0, pageNumber: 0, isLastPage: true,
            selectionEventID: 77, coreElapsedNanoseconds: 0
        )
        let recorder = DiagnosticsRecorder(directory: directory)
        recorder.record(eventKind: "selection", result: result, totalElapsedNanoseconds: 0, clientIdentifier: "test")
        recorder.flush()
        try FileManager.default.removeItem(at: directory)

        recorder.clear()
        recorder.record(eventKind: "selection-after-clear", result: result, totalElapsedNanoseconds: 0, clientIdentifier: "test")
        recorder.flush()

        let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "jsonl" }
        let contents = try files.map { try String(contentsOf: $0, encoding: .utf8) }.joined()
        XCTAssertEqual(contents.split(separator: "\n").count, 1)
        recorder.clear()
        try? FileManager.default.removeItem(at: directory)
    }

    func testDiagnosticsBindProcessGenerationAndAllocateContinuousSequence() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-diagnostics-generation-\(UUID().uuidString)", isDirectory: true)
        let recorder = DiagnosticsRecorder(directory: directory)
        let generation = "11111111-2222-3333-4444-555555555555"
        recorder.configureProcessInstance(generation: generation)
        recorder.recordSystem(eventKind: "first")
        recorder.recordTransition(eventKind: "second", clientIdentifier: "test")
        recorder.flush()

        let eventFiles = try FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil
        ).filter { $0.pathExtension == "jsonl" }
        let events = try eventFiles.flatMap { file in
            try Data(contentsOf: file).split(separator: 0x0A).map {
                try JSONDecoder().decode(DiagnosticEnvelope.self, from: Data($0))
            }
        }.sorted { $0.sequence < $1.sequence }
        XCTAssertEqual(events.map(\.processInstanceID), [generation, generation])
        XCTAssertEqual(events.map(\.sequence), [1, 2])
        recorder.clear()
        try? FileManager.default.removeItem(at: directory)
    }

    func testConcurrentDiagnosticsKeepTimestampAndSequenceInFileOrder() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-diagnostics-concurrent-\(UUID().uuidString)", isDirectory: true)
        let recorder = DiagnosticsRecorder(directory: directory)
        let generation = "33333333-3333-4333-8333-333333333333"
        recorder.configureProcessInstance(generation: generation)

        let eventCount = 128
        DispatchQueue.concurrentPerform(iterations: eventCount) { index in
            recorder.recordSystem(eventKind: "concurrent-\(index)")
        }
        recorder.flush()

        let eventFiles = try FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil
        ).filter { $0.pathExtension == "jsonl" }
        let events = try eventFiles.sorted { $0.lastPathComponent < $1.lastPathComponent }.flatMap { file in
            try Data(contentsOf: file).split(separator: 0x0A).map {
                try JSONDecoder().decode(DiagnosticEnvelope.self, from: Data($0))
            }
        }
        XCTAssertEqual(events.count, eventCount)
        XCTAssertEqual(events.map(\.processInstanceID), Array(repeating: generation, count: eventCount))
        XCTAssertEqual(events.map(\.sequence), (1...UInt64(eventCount)).map { $0 })
        XCTAssertEqual(events.map(\.timestamp), events.map(\.timestamp).sorted())
        recorder.clear()
        try? FileManager.default.removeItem(at: directory)
    }

    func testSnapshotReaderRejectsBadCurrentAndFallsBack() throws {
        let directory = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-snapshot-reader-\(UUID().uuidString)", isDirectory: true)
        try makePrivateDirectory(directory)
        let (validData, checksum, validName) = snapshotFixture(version: 1)
        try writePrivateFile(validData, to: directory.appendingPathComponent(validName))
        let badChecksum = String(repeating: "b", count: 64)
        let badName = "snapshot-2-\(badChecksum).aimsnap"
        try writePrivateFile(Data("corrupt".utf8), to: directory.appendingPathComponent(badName))
        try writePrivateFile(
            Data("version=2\nchecksum=\(badChecksum)\nfile=\(badName)\nentry_count=1\n".utf8),
            to: directory.appendingPathComponent("current")
        )
        let snapshot = SnapshotReader(root: directory).loadCurrentOrFallback()
        XCTAssertEqual(snapshot?.version, 1)
        XCTAssertEqual(snapshot?.checksum, checksum)
        try? FileManager.default.removeItem(at: directory)
    }

    func testSnapshotFallbackUsesStrictEmbeddedVersionInsteadOfModificationDate() throws {
        let directory = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-snapshot-order-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        try makePrivateDirectory(directory)
        let older = snapshotFixture(version: 2)
        let newer = snapshotFixture(version: 10)
        let olderURL = directory.appendingPathComponent(older.2)
        let newerURL = directory.appendingPathComponent(newer.2)
        try writePrivateFile(older.0, to: olderURL)
        try writePrivateFile(newer.0, to: newerURL)
        try FileManager.default.setAttributes([.modificationDate: Date()], ofItemAtPath: olderURL.path)
        try FileManager.default.setAttributes(
            [.modificationDate: Date(timeIntervalSince1970: 1)], ofItemAtPath: newerURL.path
        )

        let snapshot = SnapshotReader(root: directory).loadCurrentOrFallback()
        XCTAssertEqual(snapshot?.version, 10)
        XCTAssertEqual(snapshot?.checksum, newer.1)
    }

    func testSnapshotReaderRejectsTraversalDuplicateOversizedAndUnsafeLinks() throws {
        for hardLink in [false, true] {
            let parent = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
                .appendingPathComponent("agent-ime-snapshot-link-\(UUID().uuidString)", isDirectory: true)
            defer { try? FileManager.default.removeItem(at: parent) }
            let directory = parent.appendingPathComponent("Snapshots", isDirectory: true)
            try makePrivateDirectory(directory)
            let sentinel = parent.appendingPathComponent("sentinel")
            try Data("outside-must-not-change".utf8).write(to: sentinel)
            try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: sentinel.path)
            let current = directory.appendingPathComponent("current")
            if hardLink {
                try FileManager.default.linkItem(at: sentinel, to: current)
            } else {
                try FileManager.default.createSymbolicLink(at: current, withDestinationURL: sentinel)
            }
            XCTAssertNil(SnapshotReader(root: directory).loadCurrentOrFallback())
            XCTAssertEqual(try Data(contentsOf: sentinel), Data("outside-must-not-change".utf8))
            XCTAssertEqual(try posixMode(sentinel), 0o640)
            XCTAssertTrue(FileManager.default.fileExists(atPath: current.path))
        }

        let directory = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-snapshot-invalid-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        try makePrivateDirectory(directory)
        let current = directory.appendingPathComponent("current")
        let checksum = String(repeating: "a", count: 64)
        try writePrivateFile(
            Data("version=1\nchecksum=\(checksum)\nfile=../sentinel\nentry_count=1\n".utf8),
            to: current
        )
        XCTAssertNil(SnapshotReader(root: directory).loadCurrentOrFallback())
        try writePrivateFile(
            Data("version=1\nversion=2\nchecksum=\(checksum)\nfile=snapshot-1-\(checksum).aimsnap\nentry_count=1\n".utf8),
            to: current
        )
        XCTAssertNil(SnapshotReader(root: directory).loadCurrentOrFallback())
        try writePrivateFile(Data(repeating: 0x61, count: 4_097), to: current)
        XCTAssertNil(SnapshotReader(root: directory).loadCurrentOrFallback())
        try writePrivateFile(
            Data("version=1\nchecksum=\(checksum)\nfile=snapshot-1-\(checksum).aimsnap\nentry_count=1\n\n".utf8),
            to: current
        )
        XCTAssertNil(SnapshotReader(root: directory).loadCurrentOrFallback())
    }

    func testSnapshotReaderRejectsInvalidEntryShapeWeightAndOversizedBody() throws {
        let directory = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-snapshot-entry-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        try makePrivateDirectory(directory)

        for (index, entry) in [
            "a\t甲\tfixture",
            "a\t甲\tfixture\t1\textra",
            "a\t甲\tfixture\tnot-a-weight",
            "a\t甲\tfixture\t01",
        ].enumerated() {
            let snapshot = snapshotFixture(version: UInt64(index + 1), entry: entry)
            try writePrivateFile(snapshot.0, to: directory.appendingPathComponent(snapshot.2))
            try writePrivateFile(
                Data("version=\(index + 1)\nchecksum=\(snapshot.1)\nfile=\(snapshot.2)\nentry_count=1\n".utf8),
                to: directory.appendingPathComponent("current")
            )
            XCTAssertNil(SnapshotReader(root: directory).loadCurrentOrFallback())
        }

        let oversizedData = Data(repeating: 0x61, count: 16 * 1024 * 1024 + 1)
        let oversizedChecksum = SHA256.hash(data: oversizedData)
            .map { String(format: "%02x", $0) }.joined()
        let oversizedName = "snapshot-99-\(oversizedChecksum).aimsnap"
        try writePrivateFile(oversizedData, to: directory.appendingPathComponent(oversizedName))
        try writePrivateFile(
            Data("version=99\nchecksum=\(oversizedChecksum)\nfile=\(oversizedName)\nentry_count=1\n".utf8),
            to: directory.appendingPathComponent("current")
        )
        XCTAssertNil(SnapshotReader(root: directory).loadCurrentOrFallback())
    }

    func testSecureDirectoryRejectsTemporaryLinksAndPathReplacement() throws {
        for hardLink in [false, true] {
            let parent = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
                .appendingPathComponent("agent-ime-secure-dir-\(UUID().uuidString)", isDirectory: true)
            defer { try? FileManager.default.removeItem(at: parent) }
            let directoryURL = parent.appendingPathComponent("Snapshots", isDirectory: true)
            try makePrivateDirectory(directoryURL)
            let directory = try SecureDirectoryDescriptor.openExisting(
                directoryURL, requiredMode: mode_t(0o700)
            )
            let sentinel = parent.appendingPathComponent("temp-sentinel")
            try Data("temporary-sentinel".utf8).write(to: sentinel)
            try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: sentinel.path)
            let temporaryName = ".fixed-temp"
            let temporary = directoryURL.appendingPathComponent(temporaryName)
            if hardLink {
                try FileManager.default.linkItem(at: sentinel, to: temporary)
            } else {
                try FileManager.default.createSymbolicLink(at: temporary, withDestinationURL: sentinel)
            }
            XCTAssertThrowsError(try directory.atomicWrite(
                Data("safe".utf8), to: "current", maximumBytes: 64,
                temporaryNames: [temporaryName]
            ))
            XCTAssertEqual(try Data(contentsOf: sentinel), Data("temporary-sentinel".utf8))
            XCTAssertEqual(try posixMode(sentinel), 0o640)
            XCTAssertTrue(FileManager.default.fileExists(atPath: temporary.path))
        }

        let parent = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-secure-dir-swap-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: parent) }
        let directoryURL = parent.appendingPathComponent("Snapshots", isDirectory: true)
        try makePrivateDirectory(directoryURL)
        let directory = try SecureDirectoryDescriptor.openExisting(
            directoryURL, requiredMode: mode_t(0o700)
        )
        let moved = parent.appendingPathComponent("moved-Snapshots", isDirectory: true)
        try FileManager.default.moveItem(at: directoryURL, to: moved)
        let external = parent.appendingPathComponent("external", isDirectory: true)
        try makePrivateDirectory(external)
        let sentinel = external.appendingPathComponent("sentinel")
        try Data("external-tree".utf8).write(to: sentinel)
        try FileManager.default.createSymbolicLink(at: directoryURL, withDestinationURL: external)
        XCTAssertThrowsError(try directory.atomicWrite(
            Data("safe".utf8), to: "current", maximumBytes: 64
        ))
        XCTAssertEqual(try Data(contentsOf: sentinel), Data("external-tree".utf8))
        XCTAssertFalse(FileManager.default.fileExists(atPath: external.appendingPathComponent("current").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: moved.appendingPathComponent("current").path))
    }

    func testSnapshotAndRecoveryRejectSymlinkedAncestors() throws {
        let parent = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-ancestor-link-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: parent) }
        try makePrivateDirectory(parent)
        let external = parent.appendingPathComponent("external", isDirectory: true)
        try makePrivateDirectory(external)
        let snapshots = external.appendingPathComponent("Snapshots", isDirectory: true)
        try makePrivateDirectory(snapshots)
        let runtimeRoot = external.appendingPathComponent("RuntimeRoot", isDirectory: true)
        try makePrivateDirectory(runtimeRoot)
        let sentinel = external.appendingPathComponent("sentinel")
        try Data("ancestor-external".utf8).write(to: sentinel)
        let linkedAncestor = parent.appendingPathComponent("linked-ancestor", isDirectory: true)
        try FileManager.default.createSymbolicLink(at: linkedAncestor, withDestinationURL: external)

        XCTAssertNil(SnapshotReader(
            root: linkedAncestor.appendingPathComponent("Snapshots", isDirectory: true)
        ).loadCurrentOrFallback())
        let marker = RuntimeRecoveryMarker(
            root: linkedAncestor.appendingPathComponent("RuntimeRoot", isDirectory: true)
        )
        XCTAssertThrowsError(try marker.start())
        marker.stop()
        XCTAssertEqual(try Data(contentsOf: sentinel), Data("ancestor-external".utf8))
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: runtimeRoot.appendingPathComponent("Runtime/running.marker").path
        ))
    }

    func testSyntheticKeychainSaveReadDelete() throws {
        guard ProcessInfo.processInfo.environment["AIM_ALLOW_REAL_KEYCHAIN_TEST"] == "1" else {
            throw XCTSkip("real Keychain mutation requires explicit operator authorization")
        }
        let account = "test-\(UUID().uuidString)"
        let secret = Data("synthetic-secret-never-log".utf8)
        XCTAssertEqual(KeychainSpike.save(account: account, secret: secret), errSecSuccess)
        let (readStatus, readSecret) = KeychainSpike.read(account: account)
        XCTAssertEqual(readStatus, errSecSuccess)
        XCTAssertEqual(readSecret, secret)
        XCTAssertEqual(KeychainSpike.delete(account: account), errSecSuccess)
        XCTAssertEqual(KeychainSpike.read(account: account).0, errSecItemNotFound)
    }
}

private struct DiagnosticEnvelope: Decodable {
    let timestamp: String
    let processInstanceID: String
    let sequence: UInt64
}
