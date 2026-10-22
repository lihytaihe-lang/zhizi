import AppKit

struct InputModeStatusPresentation: Equatable, Sendable {
    let symbol: String
    let modeName: String
    let menuHeaderTitle: String
    let accessibilityLabel: String
    let toolTip: String

    static func make(directEnglish: Bool) -> Self {
        let symbol = directEnglish ? "A" : "中"
        let modeName = directEnglish ? "直接英文" : "简体中文"
        return Self(
            symbol: symbol,
            modeName: modeName,
            menuHeaderTitle: "知字 · \(modeName)",
            accessibilityLabel: "知字，\(modeName)模式",
            toolTip: "知字 · \(modeName)"
        )
    }
}

@MainActor
protocol InputModeStatusItemRendering: AnyObject {
    func render(presentation: InputModeStatusPresentation, menu: NSMenu)
    func remove()
}

/// The real, process-owned AppKit status surface.  It is deliberately kept
/// separate from the pure presentation mapping so the 中/A contract can be
/// tested without installing or mutating a system input source.
@MainActor
private final class SystemInputModeStatusItemRenderer: InputModeStatusItemRendering {
    private var statusItem: NSStatusItem?

    func render(presentation: InputModeStatusPresentation, menu: NSMenu) {
        let item: NSStatusItem
        if let statusItem {
            item = statusItem
        } else {
            item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
            statusItem = item
        }
        item.length = NSStatusItem.variableLength
        item.menu = menu
        item.button?.title = presentation.symbol
        item.button?.toolTip = presentation.toolTip
        item.button?.setAccessibilityLabel(presentation.accessibilityLabel)
    }

    func remove() {
        guard let statusItem else { return }
        statusItem.menu = nil
        NSStatusBar.system.removeStatusItem(statusItem)
        self.statusItem = nil
    }
}

/// One shared owner prevents a status item per input session while retaining
/// per-controller mode state.  Activating a controller switches the visible
/// presentation and menu provider; closing the last controller removes the
/// process-owned NSStatusItem.
@MainActor
final class InputModeStatusItemCoordinator {
    static let shared = InputModeStatusItemCoordinator(
        renderer: SystemInputModeStatusItemRenderer()
    )

    private final class Registration {
        weak var owner: AnyObject?
        var presentation: InputModeStatusPresentation
        var menuProvider: @MainActor () -> NSMenu?

        init(
            owner: AnyObject,
            presentation: InputModeStatusPresentation,
            menuProvider: @escaping @MainActor () -> NSMenu?
        ) {
            self.owner = owner
            self.presentation = presentation
            self.menuProvider = menuProvider
        }
    }

    private let renderer: any InputModeStatusItemRendering
    private var registrations: [ObjectIdentifier: Registration] = [:]
    private var registrationOrder: [ObjectIdentifier] = []
    private var activeIdentifier: ObjectIdentifier?

    init(renderer: any InputModeStatusItemRendering) {
        self.renderer = renderer
    }

    func register(
        owner: AnyObject,
        directEnglish: Bool,
        menuProvider: @escaping @MainActor () -> NSMenu?
    ) {
        pruneDeadRegistrations()
        let identifier = ObjectIdentifier(owner)
        let presentation = InputModeStatusPresentation.make(directEnglish: directEnglish)
        if let registration = registrations[identifier] {
            registration.presentation = presentation
            registration.menuProvider = menuProvider
        } else {
            registrations[identifier] = Registration(
                owner: owner,
                presentation: presentation,
                menuProvider: menuProvider
            )
            registrationOrder.append(identifier)
        }
        activeIdentifier = identifier
        renderActiveRegistration()
    }

    func activate(owner: AnyObject, directEnglish: Bool) {
        pruneDeadRegistrations()
        let identifier = ObjectIdentifier(owner)
        guard let registration = registrations[identifier] else { return }
        registration.presentation = .make(directEnglish: directEnglish)
        activeIdentifier = identifier
        renderActiveRegistration()
    }

    func update(owner: AnyObject, directEnglish: Bool) {
        pruneDeadRegistrations()
        let identifier = ObjectIdentifier(owner)
        guard let registration = registrations[identifier] else { return }
        registration.presentation = .make(directEnglish: directEnglish)
        if activeIdentifier == identifier {
            renderActiveRegistration()
        }
    }

    func refreshMenu(owner: AnyObject) {
        pruneDeadRegistrations()
        guard activeIdentifier == ObjectIdentifier(owner) else { return }
        renderActiveRegistration()
    }

    func unregister(owner: AnyObject) {
        let identifier = ObjectIdentifier(owner)
        registrations.removeValue(forKey: identifier)
        registrationOrder.removeAll { $0 == identifier }
        if activeIdentifier == identifier {
            activeIdentifier = registrationOrder.last
        }
        renderActiveRegistration()
    }

    func shutdown() {
        registrations.removeAll()
        registrationOrder.removeAll()
        activeIdentifier = nil
        renderer.remove()
    }

    private func pruneDeadRegistrations() {
        let dead = registrations.compactMap { identifier, registration in
            registration.owner == nil ? identifier : nil
        }
        guard !dead.isEmpty else { return }
        let deadSet = Set(dead)
        for identifier in dead {
            registrations.removeValue(forKey: identifier)
        }
        registrationOrder.removeAll { deadSet.contains($0) }
        if let activeIdentifier, deadSet.contains(activeIdentifier) {
            self.activeIdentifier = registrationOrder.last
        }
    }

    private func renderActiveRegistration() {
        pruneDeadRegistrations()
        guard let activeIdentifier,
              let registration = registrations[activeIdentifier],
              registration.owner != nil,
              let menu = registration.menuProvider() else {
            renderer.remove()
            return
        }
        renderer.render(presentation: registration.presentation, menu: menu)
    }
}
