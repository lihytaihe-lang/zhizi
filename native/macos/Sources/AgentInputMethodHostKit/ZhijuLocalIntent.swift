import Foundation

/// Context evidence for the bounded, dictionary-proved early sentence trial.
/// Multiple plausible actions, topic-only mentions and open-ended cues abstain.
enum ZhijuLocalIntent: Sendable {
    case leavingWork
    case deferDecision(formalRequest: Bool)
    case processDocuments
    case returnToCar

    init?(context: String) {
        guard context.utf8.contains(where: { $0 >= 0x80 }) else { return nil }
        let paragraph = context.split(omittingEmptySubsequences: false, whereSeparator: \.isNewline).last ?? ""
        let clauses = paragraph.suffix(42).split(whereSeparator: { "。！？；，、：,.!?;:".contains($0) })
        let recent = clauses.last ?? ""
        let reportsSpeech = recent.count <= 7 && Self.any(
            ["说", "答道", "回答", "问道", "表示"], in: String(recent))
        let relevant = reportsSpeech ? clauses.dropLast().suffix(2) : clauses.suffix(2)
        let text = relevant.joined(separator: " ")
        if text.isEmpty { return nil }

        let shiftEnded = Self.any(["到了下班时间", "下班时间到了", "已经收工", "刚交完班", "准备关门",
                                   "下班铃响"], in: text)
        let handover = Self.any(["交接已经做完", "交接已经结束", "交接完成", "交完班", "已经接岗"], in: text)
        let departure = Self.any(["最后一班地铁", "末班地铁", "末班车", "赶车", "离开", "下班"], in: text)
        let staysAtWork = Self.any(["不能下班", "先别下班", "推迟下班", "下班前", "还要值班", "不得离开",
                                    "留在岗位", "也得留下", "加班"], in: text)
        let decisionTopic = Self.any(["意见", "议案", "提案", "条款", "方案", "议程"], in: text)
        let decisionPending = Self.any(["还需要讨论", "还没统一", "仍未统一", "尚未统一", "还没定", "暂缓表决",
                                        "把问题记下来", "表决还差", "还没有结论", "还没谈拢"], in: text)
        let documents = Self.any(["手续", "登记", "材料", "证件"], in: text)
        let atWindow = Self.any(["窗口", "柜台", "办事大厅"], in: text)
        let ready = Self.any(["带齐", "备齐", "都准备好", "齐了", "刚开放", "开始受理"], in: text)
        let missingDocuments = Self.any(["没带齐", "未带齐", "没备齐", "未备齐", "还缺", "缺少材料",
                                         "回去重拿", "材料不全", "还不能办理"], in: text)
        let counterClosed = Self.any(["关门", "打烊", "停止受理", "暂不受理", "已经收工"], in: text)

        let endsHandover = ["交接已经结束后", "交完班以后", "值班交接完毕"].contains {
            text.hasSuffix($0)
        }
        let continuedWork = Self.any(["还要开会", "要继续", "继续工作", "尚未结束", "并非", "不是"], in: text)
        let leaving = (shiftEnded || handover && (departure || endsHandover))
            && !staysAtWork && !continuedWork
        let retentionRefused = Self.any(["不能先保留", "不应先保留", "不得保留", "不再保留", "禁止保留",
                                         "不同意保留"], in: text)
        let deferDecision = decisionTopic && decisionPending && !retentionRefused
        let processing = documents && atWindow && ready && !missingDocuments && !counterClosed
        let parkedCar = Self.any(["车钥匙", "地下车位", "车停在", "停车场"], in: text)
        let fetching = Self.any(["找不到", "丢过", "找钥匙", "要取车", "回去取车"], in: text)
        let returning = parkedCar && fetching && !Self.any(["不用去", "不要去", "不去", "已经找到了"], in: text)
        guard [leaving, deferDecision, processing, returning].filter({ $0 }).count == 1 else { return nil }
        if leaving { self = .leavingWork }
        else if deferDecision {
            let unresolvedMeeting = Self.any(["会上", "会议", "评审会"], in: text)
                && Self.any(["意见还没统一", "意见仍未统一", "意见尚未统一"], in: text)
            let formal = (unresolvedMeeting || Self.any(["主持人", "委员会", "代表提出", "要求", "表决"], in: text))
                && !Self.any(["没有提出要求", "没有提要求", "不提出要求", "不是代表", "不作要求"], in: text)
            self = .deferDecision(formalRequest: formal)
        }
        else { self = returning ? .returnToCar : .processDocuments }
    }

    private static func any(_ terms: [String], in context: String) -> Bool {
        terms.contains { context.contains($0) }
    }

    func uniqueCandidate(in candidates: [String]) -> String? {
        let matching: [String]
        switch self {
        case .leavingWork:
            matching = candidates.filter {
                $0.hasSuffix("下班了") && !Self.any(["要求", "希望", "不", "没"], in: String($0.dropLast(3)))
            }
        case .deferDecision(let formalRequest):
            // With ambiguous initials, a stated disagreement needs the
            // request reading. With an explicit joined subject, keep that reading.
            let retain = candidates.filter { $0.hasSuffix("先保留") }
            let requests = retain.filter { $0.hasSuffix("要求先保留") }
            matching = formalRequest && !requests.isEmpty ? requests : retain
        case .processDocuments:
            matching = candidates.filter { $0.hasSuffix("先办理") }
        case .returnToCar:
            matching = candidates.filter { $0.hasSuffix("去车库") }
        }
        return matching.count == 1 ? matching.first : nil
    }

    /// Forecast one literal keystroke. The core still proves the whole spelling
    /// and verifies that the forecast was the next physical input.
    func nextLetter(after last: UInt8) -> UInt8? {
        switch (last, self) {
        case (99, .returnToCar): return 107 // c → k in che ku
        case (_, .returnToCar): return nil
        case (98, _), (110, _), (111, _): return 108 // b/n/o → l
        case (108, .leavingWork): return 101 // l → e for le
        case (108, _): return 105 // l → i for li or liu
        case (105, .deferDecision): return 117 // li → u for liu
        default: return nil
        }
    }
}
