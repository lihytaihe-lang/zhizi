import Foundation

struct ContextCaptureConfiguration: Equatable, Sendable {
    /// Owner-frozen UTF-16 host window. IMK text ranges use UTF-16 offsets,
    /// so the configuration is expressed in the same unit as selectedRange().
    let precedingTextWindowLength: Int

    // Captured model context is memory-only. Preserve the separately consented
    // historical QualityRecorder limit when enlarging the inference window.
    static let qualityRecordingWindowLength = 8

    static let ownerDefault = ContextCaptureConfiguration(precedingTextWindowLength: 32)
}
