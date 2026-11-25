import AppKit
import UserNotifications

@MainActor
final class ReleaseUpdateMonitor {
    static let shared = ReleaseUpdateMonitor()

    private(set) var available: StableRelease?
    private var worker: Task<Void, Never>?
    private var notificationsInFlight: Set<String> = []

    func start() {
        #if arch(arm64)
        guard worker == nil, let current = ReleaseIdentity.running() else { return }
        worker = Task { [weak self] in
            let os = ProcessInfo.processInfo.operatingSystemVersion
            let operatingSystem = [os.majorVersion, os.minorVersion, os.patchVersion].map(UInt32.init)
            let client = ReleaseUpdateClient()
            var nextCheck = Duration.seconds(20)
            while !Task.isCancelled {
                do { try await Task.sleep(for: nextCheck) } catch { return }
                let result = await client.check(current: current, platform: "macos-arm64", operatingSystem: operatingSystem)
                guard !Task.isCancelled, let self else { return }
                nextCheck = .seconds(6 * 60 * 60)
                switch result {
                case .available(let release):
                    setAvailable(release)
                    await notifyIfAllowed(release)
                case .current, .incompatible:
                    setAvailable(nil)
                case .unavailable:
                    nextCheck = .seconds(30 * 60)
                }
            }
        }
        #endif
    }

    func stop() {
        worker?.cancel()
        worker = nil
    }

    private func setAvailable(_ release: StableRelease?) {
        guard available != release else { return }
        available = release
        InputModeStatusItemCoordinator.shared.refreshActiveMenu()
    }

    private func notifyIfAllowed(_ release: StableRelease) async {
        let center = UNUserNotificationCenter.current()
        var authorization = await center.notificationSettings().authorizationStatus
        if authorization == .notDetermined {
            // Give input-method-only users a quiet notification the first
            // time an update exists; macOS owns the subsequent preference.
            _ = try? await center.requestAuthorization(options: [.alert, .provisional])
            authorization = await center.notificationSettings().authorizationStatus
        }
        guard !Task.isCancelled, authorization == .authorized || authorization == .provisional else { return }
        await notifyIfNew(release, defaults: .standard) { request in
            try await center.add(request)
        }
    }

    func notifyIfNew(_ release: StableRelease, defaults: UserDefaults,
                     schedule: (UNNotificationRequest) async throws -> Void) async {
        let key = "lastNotifiedStableBuild"
        let identity = "\(release.version)-\(release.build)"
        guard defaults.string(forKey: key) != identity,
              notificationsInFlight.insert(identity).inserted else { return }
        defer { notificationsInFlight.remove(identity) }

        let content = UNMutableNotificationContent()
        content.title = "知字有新版本"
        content.body = "知字 \(release.version) 已发布。可在知字输入法菜单查看更新说明。"
        do {
            try await schedule(UNNotificationRequest(identifier: "zhizi-release-\(identity)",
                                                     content: content, trigger: nil))
            defaults.set(identity, forKey: key)
        } catch {
            // The menu remains available if macOS declines delivery.
        }
    }
}
