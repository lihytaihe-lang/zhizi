import Darwin
import Foundation

/// Writes the process's memory (numbers only) into the local diagnostics so the weekly review can see
/// how it grows: at launch, every half hour, and before sleep. Footprint is what Activity Monitor shows.
final class MemorySampler {
    static let interval: TimeInterval = 30 * 60
    private var timer: DispatchSourceTimer?

    static func megabytes() -> (footprint: Int, resident: Int)? {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
        let status = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard status == KERN_SUCCESS else { return nil }
        return (Int(info.phys_footprint >> 20), Int(info.resident_size >> 20))
    }

    static func eventKind() -> String? {
        guard let sample = megabytes() else { return nil }
        return "memory footprintMB=\(sample.footprint) residentMB=\(sample.resident)"
    }

    func record() {
        if let kind = Self.eventKind() { DiagnosticsRecorder.shared.recordSystem(eventKind: kind) }
    }

    func start() {
        guard timer == nil else { return }
        record()
        let source = DispatchSource.makeTimerSource(queue: .main)
        source.schedule(deadline: .now() + Self.interval, repeating: Self.interval, leeway: .seconds(60))
        source.setEventHandler { [weak self] in self?.record() }
        source.resume()
        timer = source
    }
}
