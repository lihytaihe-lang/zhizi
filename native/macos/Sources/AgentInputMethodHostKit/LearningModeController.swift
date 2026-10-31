import Foundation

public enum LearningMode: UInt32, Sendable {
    case rime = 0
    case zhixu = 1

    public var title: String { self == .rime ? "Rime" : "知序" }
    var preferenceValue: String { self == .rime ? "rime" : "zhixu" }
}

/// One process-wide selection, applied to every idle input session before it is saved.
@MainActor
final class LearningModeController {
    static let defaultsKey = "learningMode"
    static let wholeDefaultsKey = "zhixuWholeEnabled"
    private(set) var mode = LearningMode.rime
    private(set) var wholeEnabled = false
    private(set) var statusText: String?
    private(set) var isMigrating = false
    var onChange: (() -> Void)?

    private struct Session {
        let id: UUID
        let isComposing: () -> Bool
        let apply: (LearningMode) -> Int32
        let applyZhixuWhole: (Bool) -> Int32
    }

    private let defaults: UserDefaults
    private let libraryExists: () -> Bool
    private let migrate: (@escaping @MainActor (Bool) -> Void) -> Void
    private let diagnose: (String) -> Void
    private var sessions: [Session] = []
    private var restoredPreference = false
    private var wantsZhixuAfterMigration = false
    private var wantsWholeAfterMigration = false
    private var wholeReason = "user"
    private var migrationID: UUID?

    init(
        defaults: UserDefaults,
        libraryExists: @escaping () -> Bool,
        migrate: @escaping (@escaping @MainActor (Bool) -> Void) -> Void,
        diagnose: @escaping (String) -> Void
    ) {
        self.defaults = defaults
        self.libraryExists = libraryExists
        self.migrate = migrate
        self.diagnose = diagnose
    }

    @discardableResult
    func register(
        id: UUID,
        isComposing: @escaping () -> Bool,
        apply: @escaping (LearningMode) -> Int32,
        applyZhixuWhole: @escaping (Bool) -> Int32 = { _ in 0 }
    ) -> Bool {
        sessions.removeAll { $0.id == id }
        // A newly created core starts in Rime with whole generation off.
        if mode != .rime, isComposing() || apply(mode) != 0 {
            diagnose("learning-session-restore-failed")
            statusText = "学习方式恢复失败，请重试"
            onChange?()
            return false
        }
        if wholeEnabled, isComposing() || applyZhixuWhole(true) != 0 {
            _ = apply(.rime)
            diagnose("zhixu-whole-session-restore-failed")
            statusText = "全拼设置恢复失败，请重试"
            onChange?()
            return false
        }
        sessions.append(Session(id: id, isComposing: isComposing, apply: apply, applyZhixuWhole: applyZhixuWhole))
        if !restoredPreference {
            restoredPreference = true
            // Zhixu handles whole input by default (2026-10-31); only an explicit off from the menu stays off.
            if defaults.object(forKey: Self.wholeDefaultsKey) as? Bool != false {
                requestZhixuWhole(true, reason: "restore")
            } else if defaults.string(forKey: Self.defaultsKey) == LearningMode.zhixu.preferenceValue {
                request(.zhixu)
            }
        }
        return true
    }

    func unregister(id: UUID) {
        sessions.removeAll { $0.id == id }
        if sessions.isEmpty {
            // The worker may finish its private copy, but it no longer has a switch to apply.
            wantsZhixuAfterMigration = false
            wantsWholeAfterMigration = false
            restoredPreference = false
            mode = .rime
            wholeEnabled = false
            statusText = nil
            onChange?()
        }
    }

    func request(_ requested: LearningMode) {
        guard !sessions.isEmpty else { return }
        if requested == mode {
            if requested == .rime {
                wantsZhixuAfterMigration = false
                if wantsWholeAfterMigration { diagnoseWhole(false, reason: "learning-rime") }
                wantsWholeAfterMigration = false
            }
            statusText = nil
            saveMode()
            onChange?()
            return
        }
        guard !sessions.contains(where: { $0.isComposing() }) else {
            showCompositionMessage()
            return
        }
        if requested == .rime {
            wantsZhixuAfterMigration = false
            wantsWholeAfterMigration = false
            applyToSessions(.rime, whole: false, reason: "learning-rime")
        } else if libraryExists() {
            let whole = wholeEnabled || wantsWholeAfterMigration
            wantsZhixuAfterMigration = false
            wantsWholeAfterMigration = false
            applyToSessions(.zhixu, whole: whole, reason: wholeReason)
        } else {
            wantsZhixuAfterMigration = true
            startMigration()
        }
    }

    func requestZhixuWhole(_ enabled: Bool) {
        requestZhixuWhole(enabled, reason: "user")
    }

    private func requestZhixuWhole(_ enabled: Bool, reason: String) {
        guard !sessions.isEmpty else { return }
        if enabled == wholeEnabled {
            if !enabled {
                wantsWholeAfterMigration = false
                diagnoseWhole(false, reason: reason)
            }
            statusText = wantsZhixuAfterMigration ? "正在迁移，仍使用 Rime" : nil
            saveMode()
            onChange?()
            return
        }
        guard !sessions.contains(where: { $0.isComposing() }) else {
            showCompositionMessage()
            return
        }
        wholeReason = reason
        if enabled, mode == .rime, !libraryExists() {
            wantsWholeAfterMigration = true
            startMigration()
        } else {
            wantsWholeAfterMigration = false
            applyToSessions(enabled ? .zhixu : mode, whole: enabled, reason: reason)
        }
    }

    private func startMigration() {
        statusText = "正在迁移，仍使用 Rime"
        if isMigrating {
            onChange?()
            return
        }
        let id = UUID()
        migrationID = id
        isMigrating = true
        onChange?()
        migrate { [weak self] success in self?.migrationFinished(id: id, success: success) }
    }

    private func migrationFinished(id: UUID, success: Bool) {
        guard migrationID == id else { return }
        migrationID = nil
        isMigrating = false
        guard wantsZhixuAfterMigration || wantsWholeAfterMigration, !sessions.isEmpty else {
            onChange?()
            return
        }
        let whole = wantsWholeAfterMigration
        wantsZhixuAfterMigration = false
        wantsWholeAfterMigration = false
        guard success, libraryExists() else {
            diagnose("learning-migration-failed")
            if whole { diagnoseWhole(false, reason: "migration-failed") }
            statusText = "切换失败，仍使用 Rime"
            saveMode()
            onChange?()
            return
        }
        guard !sessions.contains(where: { $0.isComposing() }) else {
            if whole { diagnoseWhole(false, reason: "composing") }
            saveMode()
            showCompositionMessage()
            return
        }
        applyToSessions(.zhixu, whole: whole, reason: wholeReason)
    }

    private func applyToSessions(_ requested: LearningMode, whole: Bool, reason: String) {
        let previousMode = mode
        let previousWhole = wholeEnabled
        var rollback: [() -> Int32] = []

        func failed(_ status: Int32, wholeOperation: Bool) {
            var rolledBack = true
            for undo in rollback.reversed() {
                if undo() != 0 {
                    // Later undos may depend on this step (whole generation requires Zhixu).
                    rolledBack = false
                    break
                }
            }
            let operation = wholeOperation ? "zhixu-whole-switch" : "learning-switch"
            diagnose(rolledBack ? "\(operation)-failed" : "\(operation)-rollback-failed")
            if whole != previousWhole {
                diagnoseWhole(previousWhole, reason: rolledBack ? "switch-failed" : "rollback-failed")
            }
            if !rolledBack {
                statusText = "切换失败，请重启输入法"
            } else if status == -10 {
                statusText = "正在组词，请完成后再切换"
            } else {
                statusText = "切换失败，仍使用 \(mode.title)"
            }
            saveMode()
            onChange?()
        }

        // Disable before returning to Rime; enable only after every session learns with Zhixu.
        if previousWhole && !whole {
            for session in sessions {
                let status = session.applyZhixuWhole(false)
                guard status == 0 else { failed(status, wholeOperation: true); return }
                rollback.append { session.applyZhixuWhole(previousWhole) }
            }
        }
        if requested != previousMode {
            for session in sessions {
                let status = session.apply(requested)
                guard status == 0 else { failed(status, wholeOperation: false); return }
                rollback.append { session.apply(previousMode) }
            }
        }
        if !previousWhole && whole {
            for session in sessions {
                let status = session.applyZhixuWhole(true)
                guard status == 0 else { failed(status, wholeOperation: true); return }
                rollback.append { session.applyZhixuWhole(previousWhole) }
            }
        }
        mode = requested
        wholeEnabled = whole
        if previousWhole != whole { diagnoseWhole(whole, reason: reason) }
        statusText = nil
        saveMode()
        onChange?()
    }

    private func saveMode() {
        defaults.set(mode.preferenceValue, forKey: Self.defaultsKey)
        defaults.set(wholeEnabled, forKey: Self.wholeDefaultsKey)
    }

    private func diagnoseWhole(_ enabled: Bool, reason: String) {
        diagnose("zhixu-whole \(enabled ? "on" : "off") reason=\(reason)")
    }

    private func showCompositionMessage() {
        statusText = "正在组词，请完成后再切换"
        onChange?()
    }
}
