import XCTest
@testable import AgentInputMethodHostKit

final class LegacyPreferencesTests: XCTestCase {
    private func freshDefaults(_ name: String) -> UserDefaults {
        let defaults = UserDefaults(suiteName: name)!
        defaults.removePersistentDomain(forName: name)
        return defaults
    }

    func testFirstLaunchCopiesTheOldSettingsOnceAndNeverOverwrites() {
        let defaults = freshDefaults("test.legacy-preferences.copy")
        defaults.set(3, forKey: "fuzzyMask")  // already set under the new identity
        let legacy: [String: Any] = [
            "fuzzyMask": 0, "typoCorrectionEnabled": true, "directEnglishMode.com.google.Chrome": true,
        ]
        XCTAssertEqual(LegacyPreferences.migrate(into: defaults, from: legacy), 2)
        XCTAssertEqual(defaults.integer(forKey: "fuzzyMask"), 3)
        XCTAssertTrue(defaults.bool(forKey: "typoCorrectionEnabled"))
        XCTAssertTrue(defaults.bool(forKey: "directEnglishMode.com.google.Chrome"))
        // A second launch changes nothing, even if the old domain still has more.
        XCTAssertEqual(LegacyPreferences.migrate(into: defaults, from: ["extra": 1]), 0)
        XCTAssertNil(defaults.object(forKey: "extra"))
    }

    func testNothingToCopyStillMarksTheMigrationDone() {
        let defaults = freshDefaults("test.legacy-preferences.empty")
        XCTAssertEqual(LegacyPreferences.migrate(into: defaults, from: nil), 0)
        XCTAssertTrue(defaults.bool(forKey: LegacyPreferences.doneKey))
    }
}
