import AppKit
import AgentIMECoreBridge
import Carbon.HIToolbox

public struct MappedIMEEvent {
    public let kind: IMEKeyKind
    public let text: String
    public let modifiers: UInt32
    public let deliveryID: UInt64
    public let isAutorepeat: Bool
}

public final class EventMapper {
    private var shiftPressedAlone = false

    public init() {}

    public func map(_ event: NSEvent) -> MappedIMEEvent? {
        guard event.type == .keyDown || event.type == .flagsChanged else {
            shiftPressedAlone = false
            return nil
        }
        let deliveryID = EventDeliveryIDFactory.identifier(for: event)
        let isAutorepeat = event.cgEvent?.getIntegerValueField(.keyboardEventAutorepeat) != 0
        if event.type == .flagsChanged {
            if event.keyCode == 56 || event.keyCode == 60 {
                let shiftIsDown = event.modifierFlags.contains(.shift)
                if shiftIsDown {
                    shiftPressedAlone = event.modifierFlags.intersection([.command, .option, .control]).isEmpty
                    return nil
                }
                if shiftPressedAlone {
                    shiftPressedAlone = false
                    return MappedIMEEvent(
                        kind: .shiftTap, text: "", modifiers: 0,
                        deliveryID: deliveryID, isAutorepeat: false
                    )
                }
            }
            shiftPressedAlone = false
            if event.keyCode == 57 {
                return MappedIMEEvent(
                    kind: .capsLock, text: "", modifiers: modifiers(event),
                    deliveryID: deliveryID, isAutorepeat: false
                )
            }
            return nil
        }

        shiftPressedAlone = false
        let mappedModifiers = modifiers(event)
        // Preserve the printable character produced by Shift. Using only
        // charactersIgnoringModifiers turns Shift+1 into candidate 1 and
        // makes paired quotes/colon/parentheses unreachable while composing.
        // The keypad + is always `+` (expands the panel): owner 2026-10-11, keypad + did
        // nothing in the Claude/ChatGPT desktop apps while Shift+= worked.
        let printableText = event.keyCode == 69
            ? "+" : event.characters ?? event.charactersIgnoringModifiers ?? ""
        let actionModifiers = event.modifierFlags.intersection([.shift, .control, .option, .command])
        if event.keyCode == 51 && actionModifiers == [.control, .shift] {
            return MappedIMEEvent(
                kind: .blacklistCandidate, text: "", modifiers: mappedModifiers,
                deliveryID: deliveryID, isAutorepeat: isAutorepeat
            )
        }
        if event.keyCode == 51 && actionModifiers == [.control] {
            return MappedIMEEvent(
                kind: .deleteCandidate, text: "", modifiers: mappedModifiers,
                deliveryID: deliveryID, isAutorepeat: isAutorepeat
            )
        }
        let kind: IMEKeyKind
        switch event.keyCode {
        case 51: kind = .backspace
        case 123: kind = .left
        case 124: kind = .right
        case 125: kind = .down
        case 126: kind = .up
        case 49: kind = .space
        case 36, 76: kind = .enter
        case 53: kind = .escape
        case 116: kind = .pageUp
        case 121: kind = .pageDown
        case 27 where printableText == "-": kind = .pageUp
        case 24 where printableText == "=": kind = .pageDown
        default:
            kind = printableText.isEmpty ? .unsupported : .text
        }
        return MappedIMEEvent(
            kind: kind,
            text: kind == .text ? printableText : "",
            modifiers: mappedModifiers,
            deliveryID: deliveryID,
            isAutorepeat: isAutorepeat
        )
    }

    public func securityState(hasClient: Bool) -> IMESecurityState {
        guard hasClient else { return .unknown }
        return IsSecureEventInputEnabled() ? .secure : .nonSecure
    }

    private func modifiers(_ event: NSEvent) -> UInt32 {
        var value: UInt32 = 0
        if event.modifierFlags.contains(.shift) { value |= UInt32(AIM_MOD_SHIFT) }
        if event.modifierFlags.contains(.control) { value |= UInt32(AIM_MOD_CONTROL) }
        if event.modifierFlags.contains(.option) { value |= UInt32(AIM_MOD_OPTION) }
        if event.modifierFlags.contains(.command) { value |= UInt32(AIM_MOD_COMMAND) }
        if event.modifierFlags.contains(.capsLock) { value |= UInt32(AIM_MOD_CAPS_LOCK) }
        return value
    }
}
