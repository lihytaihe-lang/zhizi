@_spi(PerformanceHarness) import AgentInputMethodHostKit
import Foundation

struct Case: Decodable { let context: String; let candidates: [String] }
struct Row: Encodable { let scores: [Double]; let order: [Int]; let elapsedNanoseconds: UInt64 }
struct Report: Encodable {
    let loadMilliseconds: Double
    let rows: [Row]
}

if CommandLine.arguments.count == 5, CommandLine.arguments[1] == "--zhiju-distributions" {
    let cases = try JSONDecoder().decode([ZhijuDistributionCase].self,
        from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[3])))
    let rows = try ZhijuDistributionProbe.run(directory: URL(fileURLWithPath: CommandLine.arguments[2]), cases: cases,
        output: URL(fileURLWithPath: CommandLine.arguments[4]))
    print("synthetic-distribution-rows=\(rows)")
    exit(0)
}

if [6, 8].contains(CommandLine.arguments.count), ["--zhiju-delivery", "--zhiju-fifth-delivery", "--context-delivery", "--context-delivery-trace"].contains(CommandLine.arguments[1]) {
    guard ["on", "off"].contains(CommandLine.arguments[5]) else { fatalError("expected on or off") }
    if CommandLine.arguments.count == 8, CommandLine.arguments[6] != "--learning-fixture" {
        fatalError("expected --learning-fixture <copied-learning.json>")
    }
    let cases = try JSONDecoder().decode([ZhijuDeliveryCase].self,
        from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[3])))
    let report = try ZhijuDeliveryProbe.run(sharedDirectory: URL(fileURLWithPath: CommandLine.arguments[2]),
        cases: cases, enabled: CommandLine.arguments[5] == "on",
        requireFifthOffer: CommandLine.arguments[1] == "--zhiju-fifth-delivery",
        requireZhijuApplication: !["--context-delivery", "--context-delivery-trace"].contains(CommandLine.arguments[1]),
        learningFixture: CommandLine.arguments.count == 8 ? URL(fileURLWithPath: CommandLine.arguments[7]) : nil,
        recordZhijuTrace: CommandLine.arguments[1] == "--context-delivery-trace")
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(report).write(to: URL(fileURLWithPath: CommandLine.arguments[4]), options: .atomic)
    exit(report.passed ? 0 : 1)
}

// An optional final argument measures that exact input; the default preserves the original probe.
if (4...6).contains(CommandLine.arguments.count), CommandLine.arguments[1] == "--zhiju" {
    let report = try ZhijuSearchProbe.run(sharedDirectory: URL(fileURLWithPath: CommandLine.arguments[2]),
        input: CommandLine.arguments.count >= 5 ? CommandLine.arguments[4] : "wmxcqcfb",
        context: CommandLine.arguments.count == 6 ? CommandLine.arguments[5] : "")
    try JSONEncoder().encode(report).write(to: URL(fileURLWithPath: CommandLine.arguments[3]), options: .atomic)
    if report.status == "skipped_model_missing" { print("知句模型缺失，跳过搜索耗时测量。") }
    exit(0)
}

// `--paced <ContextModel-dir> <report.json>` replays keys with typing pauses.
if CommandLine.arguments.count == 4, CommandLine.arguments[1] == "--paced" {
    // Inter-key gaps seen while typing: within a word, between words, a short think,
    // and pauses long enough for the Neural Engine to clock down.
    let cycle = [0.08, 0.15, 0.3, 1.0, 3.0, 10.0]
    let gaps = (0..<60).map { cycle[$0 % cycle.count] }
    let samples = try ContextRankingPacedProbe.run(directory: URL(fileURLWithPath: CommandLine.arguments[2]),
        gaps: gaps, context: "我们现在先把这个问题", candidates: ["在", "再", "载", "仔", "崽"])
    try JSONEncoder().encode(samples).write(to: URL(fileURLWithPath: CommandLine.arguments[3]), options: .atomic)
    exit(0)
}

guard CommandLine.arguments.count == 4 else {
    fatalError("usage: AgentIMEContextModelHarness <ContextModel-dir> <cases.json> <report.json> | --paced <ContextModel-dir> <report.json> | --zhiju <Resources/Rime-dir> <report.json> [input] | --zhiju-delivery <Resources/Rime-dir> <cases.json> <report.json> <on|off>")
}
let cases = try JSONDecoder().decode([Case].self, from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[2])))
let started = DispatchTime.now().uptimeNanoseconds
let scorer = try ContextRankingScorer(directory: URL(fileURLWithPath: CommandLine.arguments[1]))
// Async production loader performs this same warm-up before publishing a ready
// scorer. Publish load + first inference separately from warm rerank latency.
for _ in 0..<3 { _ = try scorer.score(context: "", candidates: ["在", "再"]) }
let loadMilliseconds = Double(DispatchTime.now().uptimeNanoseconds &- started) / 1_000_000
let rows = try cases.map { item in
    let begin = DispatchTime.now().uptimeNanoseconds
    let scores = try scorer.score(context: item.context, candidates: item.candidates)
    let order = ContextRankingScorer.blendedOrder(scores: scores)
    return Row(scores: scores, order: order, elapsedNanoseconds: DispatchTime.now().uptimeNanoseconds &- begin)
}
let report = Report(loadMilliseconds: loadMilliseconds, rows: rows)
try JSONEncoder().encode(report).write(to: URL(fileURLWithPath: CommandLine.arguments[3]), options: .atomic)
