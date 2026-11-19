import Foundation

enum ModelContextSource: Equatable {
    case host
    case recentCommit
    case unavailable
}

struct ModelContextSelection: Equatable {
    let text: String?
    let source: ModelContextSource

    static func resolve(
        hostText: String?,
        recentCommitText: String,
        rankingEnabled: Bool,
        generationEnabled: Bool
    ) -> Self {
        if let hostText {
            return ModelContextSelection(text: hostText, source: .host)
        }
        if rankingEnabled || generationEnabled {
            return ModelContextSelection(text: recentCommitText, source: .recentCommit)
        }
        return ModelContextSelection(text: nil, source: .unavailable)
    }
}
