import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class ProjectRuntimeConfigurationTests: XCTestCase {
    func testSecureRuntimeConfigurationLoadsAndMalformedFileFallsBackToGlobal() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("project-runtime-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let directory = root.appendingPathComponent("Shared/Profiles", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
        let expected = ProjectRuntimeConfiguration(
            activeProfileId: "aim",
            activeProjectDomain: "agent-input-method",
            activeProfessionalPacks: ["input-method", "rust"],
            agentContext: "只修改当前项目",
            secondaryLayerEnabled: false,
            predictionEnabled: true,
            candidateTheme: .xuanmo
        )
        let path = directory.appendingPathComponent("runtime.json")
        try JSONEncoder().encode(expected).write(to: path)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path.path)
        let reader = ProjectRuntimeConfigurationReader(root: root)
        XCTAssertEqual(reader.load(), expected)

        let legacy = Data("""
        {"activeProfileId":null,"activeProjectDomain":null,"activeProfessionalPacks":[],
         "agentContext":"","secondaryLayerEnabled":true,"predictionEnabled":false}
        """.utf8)
        try legacy.write(to: path)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path.path)
        XCTAssertEqual(reader.load().candidateTheme, .system)

        let unknownTheme = Data("""
        {"activeProfileId":"kept","activeProjectDomain":"kept-domain",
         "activeProfessionalPacks":["kept-pack"],"agentContext":"kept-context",
         "secondaryLayerEnabled":false,"predictionEnabled":true,
         "candidateTheme":"not-installed"}
        """.utf8)
        try unknownTheme.write(to: path)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path.path)
        let unknownThemeResult = reader.load()
        XCTAssertEqual(unknownThemeResult.candidateTheme, .system)
        XCTAssertEqual(unknownThemeResult.activeProfileId, "kept")
        XCTAssertEqual(unknownThemeResult.activeProjectDomain, "kept-domain")
        XCTAssertEqual(unknownThemeResult.activeProfessionalPacks, ["kept-pack"])
        XCTAssertEqual(unknownThemeResult.agentContext, "kept-context")
        XCTAssertFalse(unknownThemeResult.secondaryLayerEnabled)
        XCTAssertTrue(unknownThemeResult.predictionEnabled)

        try Data("not-json".utf8).write(to: path)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path.path)
        XCTAssertEqual(reader.load(), .stableGlobal)
    }

    func testRuntimeConfigurationSymlinkIsRejectedWithoutReadingTarget() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("project-runtime-link-\(UUID().uuidString)", isDirectory: true)
        let target = root.deletingLastPathComponent().appendingPathComponent("project-runtime-secret-\(UUID().uuidString)")
        defer {
            try? FileManager.default.removeItem(at: root)
            try? FileManager.default.removeItem(at: target)
        }
        let directory = root.appendingPathComponent("Shared/Profiles", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
        try Data("{\"activeProfileId\":\"secret\"}".utf8).write(to: target)
        try FileManager.default.createSymbolicLink(
            at: directory.appendingPathComponent("runtime.json"),
            withDestinationURL: target
        )
        XCTAssertEqual(ProjectRuntimeConfigurationReader(root: root).load(), .stableGlobal)
    }

    func testRuntimeConfigurationAcceptsInstalledExternalSkinID() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("project-runtime-external-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let profiles = root.appendingPathComponent("Shared/Profiles", isDirectory: true)
        let skin = root.appendingPathComponent("Skins/owner-green", isDirectory: true)
        try FileManager.default.createDirectory(at: profiles, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: skin, withIntermediateDirectories: true)
        for directory in [profiles, root.appendingPathComponent("Skins"), skin] {
            try FileManager.default.setAttributes(
                [.posixPermissions: 0o700],
                ofItemAtPath: directory.path
            )
        }
        let skinJSON = skin.appendingPathComponent("skin.json")
        try Data("""
        {"id":"owner-green","displayName":{"zh":"Owner 绿","en":"Owner Green"},
         "formatVersion":1,"author":"Owner","description":"external"}
        """.utf8).write(to: skinJSON)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: skinJSON.path)
        let expected = ProjectRuntimeConfiguration(
            activeProfileId: nil,
            activeProjectDomain: nil,
            activeProfessionalPacks: [],
            agentContext: "",
            secondaryLayerEnabled: true,
            predictionEnabled: false,
            candidateTheme: CandidateThemeIdentifier(rawValue: "owner-green")!
        )
        let runtimeJSON = profiles.appendingPathComponent("runtime.json")
        try JSONEncoder().encode(expected).write(to: runtimeJSON)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: runtimeJSON.path
        )
        XCTAssertEqual(ProjectRuntimeConfigurationReader(root: root).load(), expected)
    }
}
