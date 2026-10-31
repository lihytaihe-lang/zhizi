import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class LearningModeControllerTests: XCTestCase {
    @MainActor
    func testDefaultAndUnknownPreferenceUseRimeWithoutMigration() {
        for saved in [nil, "unsupported"] as [String?] {
            let fixture = LearningModeFixture(saved: saved)
            defer { fixture.cleanUp() }
            XCTAssertEqual(fixture.controller.mode, .rime)
            XCTAssertEqual(fixture.migrationCalls, 0)
            XCTAssertTrue(fixture.session.applied.isEmpty)
            XCTAssertNil(fixture.controller.statusText)
        }
        XCTAssertEqual(LearningMode.rime.title, "Rime")
        XCTAssertEqual(LearningMode.zhixu.title, "知序")
    }

    @MainActor
    func testFirstSwitchWaitsForMigrationBeforeChangingModeOrPreference() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.migrationCalls, 1)
        XCTAssertTrue(fixture.controller.isMigrating)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertTrue(fixture.session.applied.isEmpty)
        XCTAssertNil(fixture.defaults.string(forKey: LearningModeController.defaultsKey))
        XCTAssertEqual(fixture.controller.statusText, "正在迁移，仍使用 Rime")

        fixture.finishMigration(success: true)
        XCTAssertFalse(fixture.controller.isMigrating)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual(fixture.session.applied, [.zhixu])
        XCTAssertEqual(fixture.defaults.string(forKey: LearningModeController.defaultsKey), "zhixu")
        XCTAssertNil(fixture.controller.statusText)
    }

    @MainActor
    func testSavedModeRestoresInANewControllerWithoutAnotherMigration() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        fixture.finishMigration(success: true)
        let restarted = fixture.makeController()
        let nextSession = LearningModeSession()
        XCTAssertTrue(restarted.register(
            id: nextSession.id,
            isComposing: { nextSession.isComposing },
            apply: { nextSession.apply($0) }
        ))
        XCTAssertEqual(restarted.mode, .zhixu)
        XCTAssertEqual(nextSession.applied, [.zhixu])
        XCTAssertEqual(fixture.migrationCalls, 1)
    }

    @MainActor
    func testSavedZhixuWithMissingLibraryMigratesBeforeRestoring() {
        let fixture = LearningModeFixture(saved: "zhixu")
        defer { fixture.cleanUp() }
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.migrationCalls, 1)
        XCTAssertTrue(fixture.session.applied.isEmpty)
        fixture.finishMigration(success: true)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual(fixture.session.applied, [.zhixu])
    }

    @MainActor
    func testMigrationFailureKeepsRimeAndRecordsOneDiagnostic() {
        let fixture = LearningModeFixture(saved: "zhixu")
        defer { fixture.cleanUp() }
        fixture.finishMigration(success: false)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.defaults.string(forKey: LearningModeController.defaultsKey), "rime")
        XCTAssertEqual(fixture.controller.statusText, "切换失败，仍使用 Rime")
        XCTAssertTrue(fixture.session.applied.isEmpty)
        XCTAssertEqual(fixture.diagnostics, ["learning-migration-failed"])
    }

    @MainActor
    func testSwitchBackIsImmediateAndNextSwitchKeepsTheLearningLibrary() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        fixture.finishMigration(success: true)
        fixture.controller.request(.rime)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.defaults.string(forKey: LearningModeController.defaultsKey), "rime")
        XCTAssertTrue(fixture.libraryPresent)
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual(fixture.session.applied, [.zhixu, .rime, .zhixu])
        XCTAssertEqual(fixture.migrationCalls, 1)
    }

    @MainActor
    func testExistingLibrarySkipsMigration() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual(fixture.migrationCalls, 0)
        XCTAssertEqual(fixture.session.applied, [.zhixu])
    }

    @MainActor
    func testCompositionRejectsSwitchBeforeMigrationAndRequiresExplicitRetry() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.session.isComposing = true
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.controller.statusText, "正在组词，请完成后再切换")
        XCTAssertEqual(fixture.migrationCalls, 0)
        fixture.session.isComposing = false
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.migrationCalls, 0)
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.migrationCalls, 1)
    }

    @MainActor
    func testCompositionThatStartsDuringMigrationDefersToAnExplicitRetry() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        fixture.session.isComposing = true
        fixture.finishMigration(success: true)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.controller.statusText, "正在组词，请完成后再切换")
        XCTAssertTrue(fixture.session.applied.isEmpty)
        fixture.session.isComposing = false
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual(fixture.migrationCalls, 1)
    }

    @MainActor
    func testSwitchBackIsRejectedWhileComposing() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        fixture.session.isComposing = true
        fixture.controller.request(.rime)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual(fixture.session.applied, [.zhixu])
        XCTAssertEqual(fixture.defaults.string(forKey: LearningModeController.defaultsKey), "zhixu")
        XCTAssertEqual(fixture.controller.statusText, "正在组词，请完成后再切换")
    }

    @MainActor
    func testCancelDuringMigrationDoesNotApplyAStaleCompletion() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        fixture.controller.request(.rime)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertNil(fixture.controller.statusText)
        fixture.finishMigration(success: true)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertTrue(fixture.session.applied.isEmpty)
        XCTAssertEqual(fixture.defaults.string(forKey: LearningModeController.defaultsKey), "rime")
        XCTAssertTrue(fixture.libraryPresent)
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual(fixture.migrationCalls, 1)
    }

    @MainActor
    func testNewRequestWhileCancelledMigrationIsStillRunningReusesThatTask() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        fixture.controller.request(.rime)
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.migrationCalls, 1)
        fixture.finishMigration(success: true)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual(fixture.session.applied, [.zhixu])
    }

    @MainActor
    func testClosingAllSessionsInvalidatesThePendingSwitch() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        fixture.controller.unregister(id: fixture.session.id)
        fixture.finishMigration(success: true)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertNil(fixture.defaults.string(forKey: LearningModeController.defaultsKey))
        XCTAssertTrue(fixture.session.applied.isEmpty)
        XCTAssertNil(fixture.controller.statusText)
    }

    @MainActor
    func testAllRegisteredSessionsSwitchTogetherAndNewSessionRestoresCurrentMode() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        let second = LearningModeSession()
        XCTAssertTrue(fixture.register(second))
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.session.mode, .zhixu)
        XCTAssertEqual(second.mode, .zhixu)
        let third = LearningModeSession()
        XCTAssertTrue(fixture.register(third))
        XCTAssertEqual(third.applied, [.zhixu])
        fixture.controller.request(.rime)
        XCTAssertEqual([fixture.session.mode, second.mode, third.mode], [.rime, .rime, .rime])
    }

    @MainActor
    func testCoreRejectionRollsBackEarlierSessionsWithoutPersisting() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        let second = LearningModeSession()
        second.rejectedMode = .zhixu
        XCTAssertTrue(fixture.register(second))
        fixture.controller.request(.zhixu)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.session.applied, [.zhixu, .rime])
        XCTAssertEqual(fixture.session.mode, .rime)
        XCTAssertEqual(second.mode, .rime)
        XCTAssertEqual(fixture.defaults.string(forKey: LearningModeController.defaultsKey), "rime")
        XCTAssertEqual(fixture.controller.statusText, "切换失败，仍使用 Rime")
        XCTAssertEqual(fixture.diagnostics, ["learning-switch-failed"])
    }

    @MainActor
    func testRejectedNewSessionIsNotRegisteredAsSuccessfullyRestored() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        fixture.controller.request(.zhixu)
        let second = LearningModeSession()
        second.rejectedMode = .zhixu
        XCTAssertFalse(fixture.register(second))
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual(fixture.session.mode, .zhixu)
        XCTAssertEqual(fixture.diagnostics, ["learning-session-restore-failed"])
        second.isComposing = true
        fixture.controller.request(.rime)
        XCTAssertEqual(fixture.controller.mode, .rime, "the rejected session was not registered")
    }

    @MainActor
    func testRejectedReplacementRemovesThePreviousSessionCallback() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        let second = LearningModeSession()
        XCTAssertTrue(fixture.register(second))
        fixture.controller.request(.zhixu)
        XCTAssertFalse(fixture.controller.register(
            id: second.id, isComposing: { false }, apply: { _ in -1 }
        ))
        second.isComposing = true
        fixture.controller.request(.rime)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.session.mode, .rime)
        XCTAssertEqual(second.applied, [.zhixu], "a replaced core must never be called again")
    }

    @MainActor
    func testChangesNotifyTheMenuForPendingSuccessAndFailure() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        var updates = 0
        fixture.controller.onChange = { updates += 1 }
        fixture.controller.request(.zhixu)
        XCTAssertEqual(updates, 1)
        fixture.finishMigration(success: false)
        XCTAssertEqual(updates, 2)
        fixture.controller.request(.zhixu)
        fixture.finishMigration(success: true)
        XCTAssertEqual(updates, 4)
    }

    @MainActor
    func testWholeDefaultsOnAndUnknownPreferenceDoesNotDisableIt() {
        for savedWhole in [nil, "unsupported"] as [Any?] {
            let fixture = LearningModeFixture(savedWhole: savedWhole)
            defer { fixture.cleanUp() }
            XCTAssertFalse(fixture.controller.wholeEnabled)
            XCTAssertTrue(fixture.session.wholeApplied.isEmpty)
            XCTAssertEqual(fixture.migrationCalls, 1)
        }
    }

    @MainActor
    func testWholeEnablesOnlyAfterMigrationAndLearningSwitchSucceed() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.requestZhixuWhole(true)
        fixture.controller.requestZhixuWhole(true)
        XCTAssertEqual(fixture.migrationCalls, 1)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertTrue(fixture.session.wholeApplied.isEmpty)
        XCTAssertFalse(fixture.defaults.bool(forKey: LearningModeController.wholeDefaultsKey))
        fixture.finishMigration(success: true)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertTrue(fixture.controller.wholeEnabled)
        XCTAssertTrue(fixture.session.wholeEnabled)
        XCTAssertEqual(fixture.session.events, ["learning-zhixu", "whole-on"])
        XCTAssertTrue(fixture.defaults.bool(forKey: LearningModeController.wholeDefaultsKey))
        XCTAssertEqual(fixture.diagnostics, ["zhixu-whole on reason=user"])
    }

    @MainActor
    func testUnsetWholePreferenceDefaultsToOnAfterMigration() {
        let fixture = LearningModeFixture(savedWhole: nil)
        defer { fixture.cleanUp() }
        XCTAssertEqual(fixture.migrationCalls, 1)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        fixture.finishMigration(success: true)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertTrue(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.diagnostics, ["zhixu-whole on reason=restore"])
    }

    @MainActor
    func testExplicitWholeOffStaysOff() {
        let fixture = LearningModeFixture(savedWhole: false)
        defer { fixture.cleanUp() }
        XCTAssertEqual(fixture.migrationCalls, 0)
        XCTAssertFalse(fixture.controller.wholeEnabled)
    }

    @MainActor
    func testWholeMigrationFailureKeepsRimeOffAndShowsFailure() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.requestZhixuWhole(true)
        fixture.finishMigration(success: false)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertFalse(fixture.session.wholeEnabled)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.session.mode, .rime)
        XCTAssertEqual(fixture.controller.statusText, "切换失败，仍使用 Rime")
        XCTAssertFalse(fixture.defaults.bool(forKey: LearningModeController.wholeDefaultsKey))
        XCTAssertEqual(fixture.diagnostics, ["learning-migration-failed", "zhixu-whole off reason=migration-failed"])
    }

    @MainActor
    func testSavedWholeRestoresAndMigratesEvenWhenLearningPreferenceWasRime() {
        let fixture = LearningModeFixture(saved: "rime", savedWhole: true)
        defer { fixture.cleanUp() }
        XCTAssertEqual(fixture.migrationCalls, 1)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        fixture.finishMigration(success: true)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertTrue(fixture.controller.wholeEnabled)
        let restarted = fixture.makeController()
        let nextSession = LearningModeSession()
        XCTAssertTrue(restarted.register(
            id: nextSession.id, isComposing: { nextSession.isComposing },
            apply: { nextSession.apply($0) }, applyZhixuWhole: { nextSession.applyWhole($0) }
        ))
        XCTAssertEqual(restarted.mode, .zhixu)
        XCTAssertTrue(restarted.wholeEnabled)
        XCTAssertTrue(nextSession.wholeEnabled)
        XCTAssertEqual(fixture.migrationCalls, 1)
    }

    @MainActor
    func testWholeCanBeDisabledWithoutChangingZhixuLearningAndSurvivesRestart() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        fixture.controller.requestZhixuWhole(true)
        fixture.controller.requestZhixuWhole(false)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.session.applied, [.zhixu])
        XCTAssertEqual(fixture.session.wholeApplied, [true, false])
        let restarted = fixture.makeController()
        let nextSession = LearningModeSession()
        XCTAssertTrue(restarted.register(
            id: nextSession.id, isComposing: { false }, apply: { nextSession.apply($0) },
            applyZhixuWhole: { nextSession.applyWhole($0) }
        ))
        XCTAssertEqual(restarted.mode, .zhixu)
        XCTAssertFalse(restarted.wholeEnabled)
        XCTAssertFalse(nextSession.wholeEnabled)
    }

    @MainActor
    func testSwitchingLearningBackToRimeDisablesWholeFirst() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        fixture.controller.requestZhixuWhole(true)
        fixture.controller.request(.rime)
        XCTAssertEqual(fixture.session.events, ["learning-zhixu", "whole-on", "whole-off", "learning-rime"])
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertFalse(fixture.session.wholeEnabled)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertFalse(fixture.defaults.bool(forKey: LearningModeController.wholeDefaultsKey))
        XCTAssertEqual(fixture.diagnostics, ["zhixu-whole on reason=user", "zhixu-whole off reason=learning-rime"])
    }

    @MainActor
    func testWholeSwitchRejectsCompositionAndRequiresExplicitRetry() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        fixture.session.isComposing = true
        fixture.controller.requestZhixuWhole(true)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.controller.statusText, "正在组词，请完成后再切换")
        fixture.session.isComposing = false
        XCTAssertFalse(fixture.controller.wholeEnabled)
        fixture.controller.requestZhixuWhole(true)
        fixture.session.isComposing = true
        fixture.controller.requestZhixuWhole(false)
        fixture.controller.request(.rime)
        XCTAssertTrue(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        fixture.session.isComposing = false
        fixture.controller.requestZhixuWhole(false)
        XCTAssertFalse(fixture.controller.wholeEnabled)
    }

    @MainActor
    func testCompositionDuringWholeMigrationDoesNotScheduleAnAutomaticSwitch() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.requestZhixuWhole(true)
        fixture.session.isComposing = true
        fixture.finishMigration(success: true)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual(fixture.controller.statusText, "正在组词，请完成后再切换")
        fixture.session.isComposing = false
        XCTAssertFalse(fixture.controller.wholeEnabled)
        fixture.controller.requestZhixuWhole(true)
        XCTAssertTrue(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.migrationCalls, 1)
    }

    @MainActor
    func testCancellingWholeMigrationDoesNotSwitchLearningOnCompletion() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.requestZhixuWhole(true)
        fixture.controller.requestZhixuWhole(false)
        fixture.finishMigration(success: true)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertTrue(fixture.session.applied.isEmpty)
        XCTAssertNil(fixture.controller.statusText)
    }

    @MainActor
    func testCancellingWholeKeepsAnExplicitLearningRequestDuringMigration() {
        let fixture = LearningModeFixture()
        defer { fixture.cleanUp() }
        fixture.controller.requestZhixuWhole(true)
        fixture.controller.request(.zhixu)
        fixture.controller.requestZhixuWhole(false)
        fixture.finishMigration(success: true)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
    }

    @MainActor
    func testRimeSelectionCancelsPendingWholeAndClosingSessionsCancelsItToo() {
        for closeSession in [false, true] {
            let fixture = LearningModeFixture()
            defer { fixture.cleanUp() }
            fixture.controller.requestZhixuWhole(true)
            if closeSession { fixture.controller.unregister(id: fixture.session.id) }
            else { fixture.controller.request(.rime) }
            fixture.finishMigration(success: true)
            XCTAssertFalse(fixture.controller.wholeEnabled)
            XCTAssertEqual(fixture.controller.mode, .rime)
            XCTAssertTrue(fixture.session.applied.isEmpty)
        }
    }

    @MainActor
    func testWholeSyncsAllSessionsAndNewSessionsRestoreLearningBeforeWhole() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        let second = LearningModeSession()
        XCTAssertTrue(fixture.register(second))
        fixture.controller.requestZhixuWhole(true)
        XCTAssertTrue(second.wholeEnabled)
        let third = LearningModeSession()
        XCTAssertTrue(fixture.register(third))
        XCTAssertEqual(third.events, ["learning-zhixu", "whole-on"])
        fixture.controller.request(.rime)
        XCTAssertEqual([fixture.session.wholeEnabled, second.wholeEnabled, third.wholeEnabled], [false, false, false])
        XCTAssertEqual([fixture.session.mode, second.mode, third.mode], [.rime, .rime, .rime])
    }

    @MainActor
    func testWholeCoreFailureRollsBackAllLearningAndWholeChanges() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        let second = LearningModeSession()
        second.rejectedWhole = true
        XCTAssertTrue(fixture.register(second))
        fixture.controller.requestZhixuWhole(true)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertEqual([fixture.session.mode, second.mode], [.rime, .rime])
        XCTAssertEqual([fixture.session.wholeEnabled, second.wholeEnabled], [false, false])
        XCTAssertFalse(fixture.defaults.bool(forKey: LearningModeController.wholeDefaultsKey))
        XCTAssertEqual(fixture.controller.statusText, "切换失败，仍使用 Rime")
    }

    @MainActor
    func testRimeCoreFailureRestoresWholeAndZhixuLearning() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        let second = LearningModeSession()
        XCTAssertTrue(fixture.register(second))
        fixture.controller.requestZhixuWhole(true)
        second.rejectedMode = .rime
        fixture.controller.request(.rime)
        XCTAssertTrue(fixture.controller.wholeEnabled)
        XCTAssertEqual(fixture.controller.mode, .zhixu)
        XCTAssertEqual([fixture.session.mode, second.mode], [.zhixu, .zhixu])
        XCTAssertEqual([fixture.session.wholeEnabled, second.wholeEnabled], [true, true])
        XCTAssertTrue(fixture.defaults.bool(forKey: LearningModeController.wholeDefaultsKey))
    }

    @MainActor
    func testFailedLearningRollbackDoesNotReenableWholeOnRimeSessions() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        let second = LearningModeSession()
        XCTAssertTrue(fixture.register(second))
        fixture.controller.requestZhixuWhole(true)
        fixture.session.rejectedMode = .zhixu
        second.rejectedMode = .rime
        fixture.controller.request(.rime)
        XCTAssertEqual(fixture.controller.statusText, "切换失败，请重启输入法")
        XCTAssertEqual(fixture.session.mode, .rime)
        XCTAssertEqual(second.mode, .zhixu)
        XCTAssertEqual(fixture.session.wholeApplied, [true, false])
        XCTAssertEqual(second.wholeApplied, [true, false])
        XCTAssertFalse(fixture.session.wholeEnabled)
        XCTAssertFalse(second.wholeEnabled)
        XCTAssertTrue(fixture.diagnostics.contains("learning-switch-rollback-failed"))
    }

    @MainActor
    func testFailedWholeRollbackDoesNotSwitchLearningBackToRime() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        let second = LearningModeSession()
        fixture.session.rejectedWhole = false
        second.rejectedWhole = true
        XCTAssertTrue(fixture.register(second))
        fixture.controller.requestZhixuWhole(true)
        XCTAssertEqual(fixture.controller.statusText, "切换失败，请重启输入法")
        XCTAssertTrue(fixture.session.wholeEnabled)
        XCTAssertFalse(second.wholeEnabled)
        XCTAssertEqual(fixture.session.mode, .zhixu)
        XCTAssertEqual(second.mode, .zhixu)
        XCTAssertEqual(fixture.session.applied, [.zhixu])
        XCTAssertEqual(second.applied, [.zhixu])
        XCTAssertTrue(fixture.diagnostics.contains("zhixu-whole-switch-rollback-failed"))
    }


    @MainActor
    func testNewSessionThatRejectsWholeIsNotRegistered() {
        let fixture = LearningModeFixture(libraryPresent: true)
        defer { fixture.cleanUp() }
        fixture.controller.requestZhixuWhole(true)
        let second = LearningModeSession()
        second.rejectedWhole = true
        XCTAssertFalse(fixture.register(second))
        second.isComposing = true
        fixture.controller.request(.rime)
        XCTAssertEqual(fixture.controller.mode, .rime)
        XCTAssertFalse(fixture.controller.wholeEnabled)
        XCTAssertEqual(second.wholeApplied, [true])
    }

}

@MainActor
private final class LearningModeFixture {
    let suiteName = "test.learning-mode.\(UUID().uuidString)"
    let defaults: UserDefaults
    let session = LearningModeSession()
    var libraryPresent: Bool
    var migrationCalls = 0
    var diagnostics: [String] = []
    var completions: [@MainActor (Bool) -> Void] = []
    lazy var controller = makeController()

    init(saved: String? = nil, libraryPresent: Bool = false, savedWhole: Any? = false) {
        defaults = UserDefaults(suiteName: suiteName)!
        self.libraryPresent = libraryPresent
        if let saved { defaults.set(saved, forKey: LearningModeController.defaultsKey) }
        if let savedWhole { defaults.set(savedWhole, forKey: LearningModeController.wholeDefaultsKey) }
        _ = register(session)
    }

    func makeController() -> LearningModeController {
        LearningModeController(
            defaults: defaults,
            libraryExists: { [unowned self] in libraryPresent },
            migrate: { [unowned self] completion in
                migrationCalls += 1
                completions.append(completion)
            },
            diagnose: { [unowned self] in diagnostics.append($0) }
        )
    }

    func register(_ session: LearningModeSession) -> Bool {
        controller.register(
            id: session.id,
            isComposing: { session.isComposing },
            apply: { session.apply($0) },
            applyZhixuWhole: { session.applyWhole($0) }
        )
    }

    func finishMigration(success: Bool) {
        if success { libraryPresent = true }
        completions.removeFirst()(success)
    }

    func cleanUp() { defaults.removePersistentDomain(forName: suiteName) }
}

@MainActor
private final class LearningModeSession {
    let id = UUID()
    var isComposing = false
    var mode = LearningMode.rime
    var rejectedMode: LearningMode?
    var applied: [LearningMode] = []
    var wholeEnabled = false
    var rejectedWhole: Bool?
    var wholeApplied: [Bool] = []
    var events: [String] = []

    func apply(_ requested: LearningMode) -> Int32 {
        applied.append(requested)
        events.append("learning-\(requested.preferenceValue)")
        if requested == rejectedMode { return -1 }
        if isComposing { return -10 }
        mode = requested
        return 0
    }

    func applyWhole(_ enabled: Bool) -> Int32 {
        wholeApplied.append(enabled)
        events.append(enabled ? "whole-on" : "whole-off")
        if enabled == rejectedWhole { return -1 }
        if isComposing { return -10 }
        if enabled && mode != .zhixu { return -1 }
        wholeEnabled = enabled
        return 0
    }
}
