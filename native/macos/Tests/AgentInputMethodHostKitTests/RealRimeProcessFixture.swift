import Foundation

/// librime setup is process-global: the first shared/user directory pair stays
/// active for every later session. Every XCTest in this target therefore uses
/// this one real, pinned fixture. Do not delete it from a test-class teardown;
/// later tests can still create sessions against the process-global runtime.
enum RealRimeProcessFixture {
    static let directories = Result<ControllerTestDirectories, Error> {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(
            "agent-ime-rime-process-\(UUID().uuidString)",
            isDirectory: true
        )
        let shared = root.appendingPathComponent("SharedRime", isDirectory: true)
        let user = root.appendingPathComponent("UserRime", isDirectory: true)
        try FileManager.default.createDirectory(at: shared, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)

        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let projectRime = repositoryRoot.appendingPathComponent("native/resources/Rime", isDirectory: true)
        for filename in [
            "agent_common_fix.dict.yaml",
            "agent_daily.dict.yaml",
            "agent_english.dict.yaml",
            "agent_idioms.dict.yaml",
            "agent_thuocl_it.dict.yaml",
            "agent_thuocl_caijing.dict.yaml",
            "agent_modern.dict.yaml",
            "agent_general.dict.yaml",
            "agent_newwords.dict.yaml",
            "agent_spoken.dict.yaml",
            "agent_ai_names.dict.yaml",
            "agent_pack_poetry.dict.yaml",
            "agent_pack_cad.dict.yaml",
            "agent_pack_industry.dict.yaml",
            "agent_project_terms.dict.yaml",
            "agent_tech_terms.dict.yaml",
            "agent_ai_terms.dict.yaml",
            "agent_pack_terms.dict.yaml",
            "agent_exact_readings.tsv",
            "agent_pinyin.dict.yaml",
            "agent_pinyin.schema.yaml",
            "default.yaml",
        ] {
            try FileManager.default.copyItem(
                at: projectRime.appendingPathComponent(filename),
                to: shared.appendingPathComponent(filename)
            )
        }

        let pinnedPinyin = repositoryRoot.appendingPathComponent(
            ".build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml"
        )
        try FileManager.default.copyItem(
            at: pinnedPinyin,
            to: shared.appendingPathComponent("pinyin_simp.dict.yaml")
        )
        return ControllerTestDirectories(root: root, shared: shared, user: user)
    }
}

struct ControllerTestDirectories {
    let root: URL
    let shared: URL
    let user: URL
}
