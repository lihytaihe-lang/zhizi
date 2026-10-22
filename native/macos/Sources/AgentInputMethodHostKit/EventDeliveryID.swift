import AppKit
import CryptoKit
import Darwin
import Foundation

/// Builds a stable, privacy-preserving identity for one physical event.
///
/// The identity is scoped to the current macOS boot session. No input text,
/// candidate text, application identity, or wall-clock time is included.
public enum EventDeliveryIDFactory {
    private static let bootSessionIdentity: String? = readBootSessionIdentity()

    public static func identifier(for event: NSEvent, candidateIndex: Int? = nil) -> UInt64 {
        guard let bootSessionIdentity = effectiveBootSessionIdentity,
              let cgEvent = event.cgEvent, cgEvent.timestamp != 0 else {
            return 0
        }
        var canonical = Data("agent-ime-delivery-id-v1\0".utf8)
        append(UInt32(bootSessionIdentity.utf8.count), to: &canonical)
        canonical.append(contentsOf: bootSessionIdentity.utf8)
        append(cgEvent.timestamp, to: &canonical)
        append(UInt64(event.type.rawValue), to: &canonical)
        // NSEvent's keyCode/isARepeat and mouse eventNumber/buttonNumber/
        // clickCount accessors raise Objective-C exceptions when used with the
        // wrong event family. CGEvent fields are total and return zero when a
        // field is not applicable, so mouse candidate clicks and keyboard or
        // accessibility activation share one crash-free canonicalization path.
        append(field(.keyboardEventKeycode, from: cgEvent), to: &canonical)
        append(event.modifierFlags.intersection(.deviceIndependentFlagsMask).rawValue, to: &canonical)
        append(field(.keyboardEventAutorepeat, from: cgEvent), to: &canonical)
        append(field(.eventSourceStateID, from: cgEvent), to: &canonical)
        append(field(.eventSourceUserData, from: cgEvent), to: &canonical)
        append(field(.keyboardEventKeyboardType, from: cgEvent), to: &canonical)
        if let candidateIndex {
            append(UInt8(1), to: &canonical)
            append(field(.mouseEventNumber, from: cgEvent), to: &canonical)
            append(field(.mouseEventButtonNumber, from: cgEvent), to: &canonical)
            append(field(.mouseEventClickState, from: cgEvent), to: &canonical)
            append(UInt64(candidateIndex), to: &canonical)
        } else {
            append(UInt8(0), to: &canonical)
        }
        let digest = SHA256.hash(data: canonical)
        var identifier: UInt64 = 0
        for byte in digest.prefix(8) { identifier = (identifier << 8) | UInt64(byte) }
        return identifier == 0 ? UInt64.max : identifier
    }

    private static func field(_ field: CGEventField, from event: CGEvent) -> UInt64 {
        UInt64(bitPattern: event.getIntegerValueField(field))
    }

    private static func append<T: FixedWidthInteger>(_ value: T, to data: inout Data) {
        var bigEndian = value.bigEndian
        withUnsafeBytes(of: &bigEndian) { data.append(contentsOf: $0) }
    }

    private static func readBootSessionIdentity() -> String? {
        var length = 0
        if sysctlbyname("kern.bootsessionuuid", nil, &length, nil, 0) == 0, length > 1 {
            var bytes = [CChar](repeating: 0, count: length)
            let status = bytes.withUnsafeMutableBufferPointer {
                sysctlbyname("kern.bootsessionuuid", $0.baseAddress, &length, nil, 0)
            }
            if status == 0 {
                let payload = bytes.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }
                let value = String(decoding: payload, as: UTF8.self)
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                if !value.isEmpty { return "uuid:\(value.lowercased())" }
            }
        }

        // Managed test sandboxes can deny kern.bootsessionuuid even though the
        // system boot record remains readable. BOOT_TIME includes microseconds,
        // is shared by every process in the boot session, and changes on reboot.
        var query = utmpx()
        query.ut_type = Int16(BOOT_TIME)
        setutxent()
        defer { endutxent() }
        guard let record = getutxid(&query) else { return nil }
        let seconds = Int64(record.pointee.ut_tv.tv_sec)
        let microseconds = Int64(record.pointee.ut_tv.tv_usec)
        guard seconds > 0, microseconds >= 0 else { return nil }
        return "boot-time:\(seconds):\(microseconds)"
    }

    private static var effectiveBootSessionIdentity: String? {
        // Managed development sandboxes can deny both system identity paths.
        // Only the dedicated UserDB test executable may supply a deterministic
        // DEVELOPMENT_ONLY value; the shipped input-method process cannot.
        if URL(fileURLWithPath: CommandLine.arguments[0]).lastPathComponent == "AgentIMEUserDBHarness",
           let fixture = ProcessInfo.processInfo.environment["AIM_USERDB_TEST_BOOT_SESSION_UUID"],
           !fixture.isEmpty {
            return "fixture:\(fixture.lowercased())"
        }
        return bootSessionIdentity
    }
}
