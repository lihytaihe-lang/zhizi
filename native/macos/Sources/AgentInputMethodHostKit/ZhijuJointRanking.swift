import Foundation

enum ZhijuJointMode: String {
    case legacy, shadow, offer
    case guardedLead = "guarded-lead"

    static func current() -> Self {
        let setting = ProcessInfo.processInfo.environment["AIM_ZHIJU_JOINT_MODE"]
            ?? UserDefaults.standard.string(forKey: "zhijuJointMode")
        return setting.flatMap(Self.init(rawValue:)) ?? .legacy
    }
}

struct ZhijuJointInputs: Decodable {
    struct Candidate: Codable, Sendable {
        let text: String
        let reading: String
        let shownReading: String?
        let consumed: Int
        let readingProved: Bool
        let source: String
        let editCost: Double?
        let protected: Bool
        let slot: Int?

        enum CodingKeys: String, CodingKey {
            case text, reading, consumed, source, protected, slot
            case editCost = "edit_cost"
            case shownReading = "shown_reading"
            case readingProved = "reading_proved"
        }

        var length: Int { text.unicodeScalars.count }
    }

    let context: String
    let raw: String
    let native: [Candidate]
    let generated: [Candidate]

    struct Row: Codable, Sendable {
        let candidate: Candidate
        let generatedIndex: Int?
    }

    /// Use the fixed eight-row model once. Distinct readings still have
    /// distinct proofs, but the text-only scorer evaluates identical text once.
    func batch() -> [Row] {
        var seen = Set<String>()
        var rows = native.filter { $0.readingProved && $0.consumed == raw.utf8.count && $0.length <= 39 }
            .filter { seen.insert($0.text).inserted }.map { Row(candidate: $0, generatedIndex: nil) }
        // Preserve the selection and pronunciation of any text the native page already shows.
        seen.formUnion(native.map(\.text))
        for (index, candidate) in generated.enumerated() where rows.count < 8 {
            if candidate.readingProved && candidate.consumed == raw.utf8.count && candidate.length <= 39 && seen.insert(candidate.text).inserted {
                rows.append(Row(candidate: candidate, generatedIndex: index))
            }
        }
        return rows
    }

    func matches(_ page: [IMECandidate]) -> Bool {
        native.count == page.count && zip(native, page).allSatisfy { proof, candidate in
            proof.text == candidate.text && (proof.shownReading ?? proof.reading) == candidate.readingKey
        }
    }
}

struct ZhijuJointDecision: Codable, Sendable {
    enum Action: String, Codable, Sendable {
        case keep, offer, lead
    }

    let action: Action
    let generatedIndex: Int?
    let reason: String

    static func evaluate(_ inputs: ZhijuJointInputs, _ rows: [ZhijuJointInputs.Row], _ scores: [Double],
                         mode: ZhijuJointMode) -> Self {
        func keep(_ reason: String) -> Self { Self(action: .keep, generatedIndex: nil, reason: reason) }
        guard scores.count == rows.count, scores.allSatisfy(\.isFinite) else { return keep("invalid-scores") }
        let scored = Array(zip(rows, scores))
        let native = scored.filter { $0.0.generatedIndex == nil && $0.0.candidate.editCost == 0 }
        guard !native.isEmpty else { return keep("no-complete-native-reference") }
        let comparisons = scored.compactMap { row, score -> (index: Int, length: Int, score: Double, margin: Double)? in
            guard let index = row.generatedIndex,
                  let reference = native.filter({ $0.0.candidate.length == row.candidate.length }).map(\.1).max() else { return nil }
            return (index, row.candidate.length, score, score - reference)
        }
        guard let best = comparisons.max(by: { $0.margin < $1.margin }) else { return keep("no-same-length-reference") }
        let first = inputs.native.first
        let contextHasHan = inputs.context.unicodeScalars.contains { (0x3400...0x9fff).contains($0.value) }
        let competing = comparisons.contains { $0.index != best.index && $0.length == best.length && $0.score > best.score - 2 }
        let protectsLastSlot = inputs.native.contains { $0.slot == 4 && $0.protected }
        if mode != .offer, best.margin > 4, !competing, contextHasHan,
           let first, !first.protected, !protectsLastSlot, first.length == best.length,
           first.readingProved, first.consumed == inputs.raw.utf8.count, first.editCost == 0 {
            return Self(action: .lead, generatedIndex: best.index, reason: "clear-same-length-lead")
        }
        guard !protectsLastSlot else { return keep("protected-last-slot") }
        guard best.margin > 2 else { return keep("insufficient-margin") }
        return Self(action: .offer, generatedIndex: best.index, reason: "same-length-offer")
    }
}

struct ZhijuJointReport: Codable, Sendable {
    let mode: String
    let native: [ZhijuJointInputs.Candidate]
    let generated: [ZhijuJointInputs.Candidate]
    let batch: [ZhijuJointInputs.Row]
    let scores: [Double]
    let decision: ZhijuJointDecision
    let rankMilliseconds: Double
    let applicationStatus: Int32?
}
