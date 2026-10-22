import Foundation

public let candidateQualityDeduplicationRule =
    "unicode-nfkc-trimmed-text+lowercase-ascii-reading-v1"

private struct CandidateQualityKey: Hashable {
    let text: String
    let reading: String
}

public func normalizedCandidateTextForQuality(_ value: String) -> String {
    value.precomposedStringWithCompatibilityMapping
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

public func normalizedReadingKeyForQuality(_ value: String) -> String {
    let folded = value.precomposedStringWithCompatibilityMapping.lowercased()
    return String(folded.unicodeScalars.filter { scalar in
        (scalar.value >= 0x61 && scalar.value <= 0x7a) || scalar.value == 0x27
    })
}

/// Applies the joint quality gate's text+reading de-duplication before top-k.
/// Order is stable, and identical text with a genuinely different reading is
/// deliberately retained as a separate candidate.
public func qualityTopCandidateTexts(
    _ candidates: [IMECandidate],
    limit: Int = 5
) -> [String] {
    guard limit > 0 else { return [] }
    var seen = Set<CandidateQualityKey>()
    var result: [String] = []
    result.reserveCapacity(min(limit, candidates.count))
    for candidate in candidates {
        let text = normalizedCandidateTextForQuality(candidate.text)
        let reading = normalizedReadingKeyForQuality(candidate.readingKey)
        guard !text.isEmpty, !reading.isEmpty else { continue }
        guard seen.insert(CandidateQualityKey(text: text, reading: reading)).inserted else {
            continue
        }
        result.append(text)
        if result.count == limit { break }
    }
    return result
}
