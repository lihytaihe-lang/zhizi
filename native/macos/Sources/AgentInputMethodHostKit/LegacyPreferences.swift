import Foundation

/// 2026-10-13: the input method's identity moved from `com.agentinputlayer.inputmethod.agent` to
/// `com.hongye.facai.inputmethod.zhizi`. Its settings (fuzzy pinyin, typo correction, per-app direct English, punctuation
/// policy, status-item position) live in the defaults domain named after the bundle, so the first launch under the new
/// identity copies what the old domain held. Nothing already set under the new identity is overwritten, and it runs once.
enum LegacyPreferences {
    static let legacyDomain = "com.agentinputlayer.inputmethod.agent"
    static let doneKey = "legacyPreferencesMigrated"

    /// Returns how many settings were copied.
    @discardableResult
    static func migrate(
        into defaults: UserDefaults = .standard,
        from legacy: [String: Any]? = UserDefaults.standard.persistentDomain(forName: legacyDomain)
    ) -> Int {
        guard !defaults.bool(forKey: doneKey) else { return 0 }
        defer { defaults.set(true, forKey: doneKey) }
        guard let legacy else { return 0 }
        var copied = 0
        for (key, value) in legacy where defaults.object(forKey: key) == nil {
            defaults.set(value, forKey: key)
            copied += 1
        }
        return copied
    }
}
