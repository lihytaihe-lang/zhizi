import Foundation

struct ProjectRuntimeConfiguration: Codable, Equatable, Sendable {
    let activeProfileId: String?
    let activeProjectDomain: String?
    let activeProfessionalPacks: [String]
    let agentContext: String
    let secondaryLayerEnabled: Bool
    let contextModelRankingEnabled: Bool
    let predictionEnabled: Bool
    let candidateTheme: CandidateThemeIdentifier

    static let stableGlobal = ProjectRuntimeConfiguration(
        activeProfileId: nil,
        activeProjectDomain: nil,
        activeProfessionalPacks: [],
        agentContext: "",
        secondaryLayerEnabled: true,
        predictionEnabled: false,
        candidateTheme: .system
    )

    init(
        activeProfileId: String?,
        activeProjectDomain: String?,
        activeProfessionalPacks: [String],
        agentContext: String,
        secondaryLayerEnabled: Bool,
        predictionEnabled: Bool,
        candidateTheme: CandidateThemeIdentifier,
        contextModelRankingEnabled: Bool = false
    ) {
        self.contextModelRankingEnabled = contextModelRankingEnabled
        self.activeProfileId = activeProfileId
        self.activeProjectDomain = activeProjectDomain
        self.activeProfessionalPacks = activeProfessionalPacks
        self.agentContext = agentContext
        self.secondaryLayerEnabled = secondaryLayerEnabled
        self.predictionEnabled = predictionEnabled
        self.candidateTheme = candidateTheme
    }

    private enum CodingKeys: String, CodingKey {
        case activeProfileId, activeProjectDomain, activeProfessionalPacks, agentContext
        case secondaryLayerEnabled, predictionEnabled, candidateTheme, contextModelRankingEnabled
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        activeProfileId = try values.decodeIfPresent(String.self, forKey: .activeProfileId)
        activeProjectDomain = try values.decodeIfPresent(String.self, forKey: .activeProjectDomain)
        activeProfessionalPacks = try values.decode([String].self, forKey: .activeProfessionalPacks)
        agentContext = try values.decode(String.self, forKey: .agentContext)
        secondaryLayerEnabled = try values.decode(Bool.self, forKey: .secondaryLayerEnabled)
        contextModelRankingEnabled = try values.decodeIfPresent(Bool.self, forKey: .contextModelRankingEnabled) ?? false
        predictionEnabled = try values.decode(Bool.self, forKey: .predictionEnabled)
        let candidateThemeId = try values.decodeIfPresent(String.self, forKey: .candidateTheme)
        candidateTheme = candidateThemeId.flatMap(CandidateThemeIdentifier.init(rawValue:)) ?? .system
    }
}

struct ProjectRuntimeConfigurationReader: Sendable {
    let root: URL

    func load() -> ProjectRuntimeConfiguration {
        let directory = root.appendingPathComponent("Shared/Profiles", isDirectory: true)
        guard let secureDirectory = try? LocalSecureDirectory.openExisting(directory),
              let data = try? secureDirectory.readRegular("runtime.json", maximumBytes: 64 * 1024).0,
              let decoded = try? JSONDecoder().decode(ProjectRuntimeConfiguration.self, from: data)
        else { return .stableGlobal }
        guard isAvailable(decoded.candidateTheme) else {
            return ProjectRuntimeConfiguration(
                activeProfileId: decoded.activeProfileId,
                activeProjectDomain: decoded.activeProjectDomain,
                activeProfessionalPacks: decoded.activeProfessionalPacks,
                agentContext: decoded.agentContext,
                secondaryLayerEnabled: decoded.secondaryLayerEnabled,
                predictionEnabled: decoded.predictionEnabled,
                candidateTheme: .system,
                contextModelRankingEnabled: decoded.contextModelRankingEnabled
            )
        }
        return decoded
    }

    private func isAvailable(_ identifier: CandidateThemeIdentifier) -> Bool {
        if CandidateSkinCatalog.builtInIdentifiers.contains(identifier) { return true }
        let skins = ProcessInfo.processInfo.environment["AIM_SKINS_DIR"]
            .map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? root.appendingPathComponent("Skins", isDirectory: true)
        guard let secureRoot = try? LocalSecureDirectory.openExisting(skins),
              let package = try? secureRoot.openChild(identifier.rawValue),
              let entries = try? package.entryNames(
                maximumCount: CandidateSkinPackageLimits.maximumFilesPerPackage + 1
              ),
              entries == ["skin.json"],
              let data = try? package.readRegular(
                "skin.json",
                maximumBytes: CandidateSkinPackageLimits.maximumSkinJSONBytes
              ).0,
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              object["id"] as? String == identifier.rawValue,
              object["formatVersion"] as? Int == 1,
              let displayName = object["displayName"] as? [String: Any],
              !(displayName["zh"] as? String ?? "").isEmpty,
              !(displayName["en"] as? String ?? "").isEmpty,
              !(object["author"] as? String ?? "").isEmpty,
              object["description"] is String else {
            return false
        }
        return true
    }
}
