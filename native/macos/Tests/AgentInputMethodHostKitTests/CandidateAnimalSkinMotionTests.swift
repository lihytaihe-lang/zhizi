import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class CandidateAnimalSkinMotionTests: XCTestCase {
    private let names = ["shuitun", "youzi", "juzimao", "baitu"]
    private var repository: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    }

    private func document(_ name: String) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: repository.appendingPathComponent(
            "native/resources/Skins/Builtin/\(name)/skin.json"))) as? [String: Any])
    }

    private func theme(_ name: String) throws -> CandidateTheme {
        let directory = repository.appendingPathComponent("native/resources/Skins/Builtin/\(name)")
        var assets: [String: Data] = [:]
        for file in try FileManager.default.contentsOfDirectory(atPath: directory.path) where file.hasSuffix(".png") {
            assets[file] = try Data(contentsOf: directory.appendingPathComponent(file))
        }
        return try CandidateSkinPackageLoader.decodeTheme(Data(contentsOf: directory.appendingPathComponent("skin.json")),
            expected: XCTUnwrap(CandidateThemeIdentifier(rawValue: name)), fallback: .system, assets: assets)
    }

    func testAnimalsHaveVisibleBreathingAndDifferentActionBeats() throws {
        for name in names {
            let items = try XCTUnwrap(document(name)["decorations"] as? [[String: Any]])
            let sprites = items.filter { $0["type"] as? String == "sprite" }
            let motions = sprites.flatMap { $0["motion"] as? [[String: Any]] ?? [] }
            let breathing = motions.filter {
                let period = $0["period"] as? Double ?? 0
                if !(3...4.5).contains(period) { return false }
                if $0["kind"] as? String == "breathe" { return (0.02...0.04).contains($0["scale"] as? Double ?? 0) }
                if $0["kind"] as? String == "float" { return (1.5...3).contains($0["amplitude"] as? Double ?? 0) }
                return false
            }
            XCTAssertFalse(breathing.isEmpty, "\(name) must move visibly in its first three seconds")
            XCTAssertGreaterThanOrEqual(Set(motions.compactMap { $0["period"] as? Double }).count, 2, name)
            XCTAssertLessThanOrEqual(items.count, 8)
            if name != "baitu" {
                let eyes = try XCTUnwrap(sprites.first { $0["clips"] != nil })
                let clips = try XCTUnwrap(eyes["clips"] as? [[String: Any]])
                XCTAssertLessThan(clips.first?["seconds"] as? Double ?? 10, 3, "first expression is visible promptly")
            }
        }
    }

    func testTailAndRabbitEarsHaveDistinctReadableLocalActions() throws {
        let cat = try XCTUnwrap(document("juzimao")["decorations"] as? [[String: Any]])
        let tail = try XCTUnwrap(cat.first { $0["sheet"] as? String == "cat-tail.png" })
        let tailMotions = try XCTUnwrap(tail["motion"] as? [[String: Any]])
        XCTAssertTrue(tailMotions.contains { $0["kind"] as? String == "sway" && ($0["degrees"] as? Double ?? 0) >= 12 })
        let rabbit = try XCTUnwrap(document("baitu")["decorations"] as? [[String: Any]])
        let ears = rabbit.filter { ["left-ear.png", "right-ear.png"].contains($0["sheet"] as? String ?? "") }
        let twitches = ears.flatMap { $0["motion"] as? [[String: Any]] ?? [] }.filter { $0["kind"] as? String == "twitch" }
        XCTAssertEqual(twitches.count, 2)
        XCTAssertEqual(Set(twitches.compactMap { $0["delay"] as? Double }).count, 2, "ears must take turns")
    }

    func testEveryAnimalMotionExtremeClearsTextAndFitsTheWindow() throws {
        for name in names {
            let loaded = try theme(name)
            for decoration in loaded.decorations {
                guard case let .sprite(sprite) = decoration else { continue }
                let bitmap = NSBitmapImageRep(cgImage: sprite.sheet)
                let cellWidth = bitmap.pixelsWide / sprite.frames, cellHeight = bitmap.pixelsHigh / sprite.rows
                var ink = CGRect.null
                for y in 0..<bitmap.pixelsHigh {
                    for x in 0..<bitmap.pixelsWide where (bitmap.colorAt(x: x, y: y)?.alphaComponent ?? 0) > 0 {
                        ink = ink.union(CGRect(x: CGFloat(x % cellWidth) * sprite.size.width / CGFloat(cellWidth),
                            y: CGFloat(cellHeight - y % cellHeight - 1) * sprite.size.height / CGFloat(cellHeight),
                            width: sprite.size.width / CGFloat(cellWidth), height: sprite.size.height / CGFloat(cellHeight)))
                    }
                }
                if sprite.clips.first?.flip == true { ink.origin.x = sprite.size.width - ink.maxX }
                let pivot = CGPoint(x: sprite.pivot.x * sprite.size.width, y: sprite.pivot.y * sprite.size.height)
                var poses = [ink]
                for motion in sprite.motions.reversed() {
                    switch motion {
                    case let .float(amplitude, _):
                        poses = poses.flatMap { [$0.offsetBy(dx: 0, dy: -amplitude), $0.offsetBy(dx: 0, dy: amplitude)] }
                    case let .breathe(scale, _):
                        let transform = CGAffineTransform(translationX: pivot.x, y: pivot.y)
                            .scaledBy(x: 1 + scale, y: 1 + scale).translatedBy(x: -pivot.x, y: -pivot.y)
                        poses = poses.flatMap { [$0, $0.applying(transform)] }
                    case let .sway(degrees, _), let .twitch(degrees, _, _, _):
                        poses = poses.flatMap { rect in [-degrees, 0, degrees].map { angle in
                            rect.applying(CGAffineTransform(translationX: pivot.x, y: pivot.y)
                                .rotated(by: angle * .pi / 180).translatedBy(x: -pivot.x, y: -pivot.y))
                        } }
                    case let .fall(height, drift, _):
                        poses = poses.flatMap { [$0, $0.offsetBy(dx: -drift, dy: height)] }
                    default: break
                    }
                }
                for width in [loaded.minimumPanelSize.width, 460, 760] {
                    let height = loaded.minimumPanelSize.height
                    let text = CGRect(x: loaded.edgeInsets.left, y: loaded.edgeInsets.bottom,
                        width: width - loaded.edgeInsets.left - loaded.edgeInsets.right,
                        height: height - loaded.edgeInsets.top - loaded.edgeInsets.bottom)
                    let window = CGRect(x: -loaded.overflowInsets.left, y: -loaded.overflowInsets.bottom,
                        width: width + loaded.overflowInsets.left + loaded.overflowInsets.right,
                        height: height + loaded.overflowInsets.top + loaded.overflowInsets.bottom)
                    var xs = [sprite.anchor.x * (width - sprite.size.width) + sprite.offset.x]
                    for clip in sprite.clips {
                        if let move = clip.move { xs += [move.from, move.to].map { $0 * max(0, width - sprite.size.width) + sprite.offset.x } }
                    }
                    for pose in poses {
                        for x in xs {
                            let rect = pose.offsetBy(dx: x, dy: sprite.anchor.y * (height - sprite.size.height) + sprite.offset.y)
                            XCTAssertFalse(rect.intersects(text), "\(name) \(sprite.size): animated art must clear candidates")
                            XCTAssertTrue(window.contains(rect), "\(name) \(sprite.size) at \(width): \(rect) outside \(window)")
                        }
                    }
                }
            }
        }
    }

    func testAnimalsKeepFiveAndTwoCandidatesInBothAppearancesAndReducedMotionStopsAllLayers() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            for name in names {
                let loaded = try theme(name)
                for texts in [["我对比", "我对你", "我对", "我队", "我"], ["我对比", "我对你"]] {
                    let panel = CandidatePanelController(presentationEnabled: false, layoutEnabled: true, theme: loaded)
                    panel.update(candidates: texts.map { IMECandidate(text: $0, comment: "", readingKey: "wo'dui'bi") },
                                 highlighted: 0, preedit: "wo dui bi", anchor: .zero, secure: false)
                    XCTAssertNotNil(panel.renderedStill())
                    XCTAssertEqual(panel.visibleCandidateTextsForTesting, texts)
                    XCTAssertLessThanOrEqual(panel.firstRowTrailingEdgeForTesting, panel.cardWidthForTesting - loaded.edgeInsets.right)
                }
                let host = CandidateDecorationHostView(frame: CGRect(x: 0, y: 0, width: 460, height: 220))
                host.configure(loaded.decorations)
                host.reduceMotionOverrideForTesting = false
                host.setRunning(true)
                XCTAssertFalse(host.animationKeysForTesting.isEmpty)
                host.reduceMotionOverrideForTesting = true
                XCTAssertTrue(host.animationKeysForTesting.isEmpty)
                XCTAssertEqual(host.emitterCountForTesting, 0)
                host.reduceMotionOverrideForTesting = false
                XCTAssertFalse(host.animationKeysForTesting.isEmpty)
                host.setRunning(false)
                XCTAssertTrue(host.animationKeysForTesting.isEmpty)
            }
        }
    }

    func testMovingEarsAndTailRemainConnectedAtBurstPeaks() throws {
        for name in ["baitu", "juzimao"] {
            let loaded = try theme(name)
            let parts = loaded.decorations.filter {
                guard case let .sprite(sprite) = $0 else { return false }
                return name == "juzimao" || sprite.size.width == 108
            }
            for time in [0.0, 1.438, 1.55, 2.249, 2.375, 3.8, 5.6, 7.8, 43.049] {
                let size = CGSize(width: 460 + loaded.overflowInsets.right,
                                  height: loaded.minimumPanelSize.height + loaded.overflowInsets.top)
                let host = CandidateDecorationHostView(frame: CGRect(origin: .zero, size: size))
                host.cardInsets = loaded.overflowInsets
                host.configure(parts)
                host.layoutSubtreeIfNeeded()
                host.reduceMotionOverrideForTesting = false
                host.setRunning(true)
                let layer = try XCTUnwrap(host.layer)
                CATransaction.begin()
                CATransaction.setDisableActions(true)
                try sample(layer, at: time)
                CATransaction.commit()
                let width = Int(size.width * 2), height = Int(size.height * 2)
                let context = try XCTUnwrap(CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
                    bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
                context.scaleBy(x: 2, y: 2)
                layer.render(in: context)
                let pixels = try XCTUnwrap(context.data).assumingMemoryBound(to: UInt8.self)
                var remaining = (0..<(width * height)).map { pixels[$0 * 4 + 3] >= 64 }
                var largeComponents = 0
                for seed in remaining.indices where remaining[seed] {
                    remaining[seed] = false
                    var stack = [seed], count = 0
                    while let point = stack.popLast() {
                        count += 1
                        for next in [point - width, point + width, point % width > 0 ? point - 1 : -1,
                                     point % width < width - 1 ? point + 1 : -1] where next >= 0 && next < remaining.count && remaining[next] {
                            remaining[next] = false
                            stack.append(next)
                        }
                    }
                    if count >= 64 { largeComponents += 1 }
                }
                XCTAssertEqual(largeComponents, 1, "\(name) at \(time)s: roots must have opaque overlap")
                host.setRunning(false)
            }
        }
    }

    func testRabbitCrownRemainsTuckedBehindTheRimAtMaximumLift() throws {
        let loaded = try theme("baitu")
        let crown = try XCTUnwrap(loaded.decorations.compactMap { item -> CandidateSpriteDecoration? in
            if case let .sprite(sprite) = item, sprite.pivot == CGPoint(x: 0.5, y: 0.1) { return sprite }; return nil
        }.first)
        let bitmap = NSBitmapImageRep(cgImage: crown.sheet)
        var bottom = crown.size.height
        for y in 0..<bitmap.pixelsHigh {
            for x in 0..<bitmap.pixelsWide where (bitmap.colorAt(x: x, y: y)?.alphaComponent ?? 0) > 0 {
                bottom = min(bottom, CGFloat(bitmap.pixelsHigh - y - 1) * crown.size.height / CGFloat(bitmap.pixelsHigh))
            }
        }
        let lift = try XCTUnwrap(crown.motions.compactMap { motion -> CGFloat? in
            if case let .float(amplitude, _) = motion { return amplitude }; return nil
        }.first)
        XCTAssertLessThanOrEqual(loaded.minimumPanelSize.height - crown.size.height + crown.offset.y + bottom + lift,
                                 loaded.minimumPanelSize.height, "the rabbit must not detach from the strip while breathing")
    }

    /// Offline fallback: sample the actual CAAnimation values into model layers, then render those layers.
    /// The static card comes from the panel renderer; no presentation-layer or real-window claim is made.
    func testWriteOfflineAnimalSequencesWhenRequested() throws {
        guard let output = ProcessInfo.processInfo.environment["AIM_ANIMAL_SEQUENCE_DIR"] else {
            throw XCTSkip("set AIM_ANIMAL_SEQUENCE_DIR to write offline animation frames")
        }
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            NSApp.appearance = NSAppearance(named: appearance)
            for name in names {
                let loaded = try theme(name)
                var base = loaded
                base.decorations = loaded.decorations.filter { if case .gradient = $0 { return true }; return false }
                let panel = CandidatePanelController(presentationEnabled: false, layoutEnabled: true, theme: base)
                panel.update(candidates: ["我对比", "我对你", "我对", "我队", "我"].map {
                    IMECandidate(text: $0, comment: "", readingKey: "wo'dui'bi")
                }, highlighted: 0, preedit: "wo dui bi", anchor: .zero, secure: false)
                let card = try XCTUnwrap(panel.renderedStill())
                let background = try XCTUnwrap(card.cgImage(forProposedRect: nil, context: nil, hints: nil))
                let directory = URL(fileURLWithPath: output).appendingPathComponent("\(name)-\(suffix)")
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                var samples = (0...16).map { (time: Double($0) * 0.5, label: String(format: "%.1f 秒", Double($0) * 0.5)) }
                for decoration in loaded.decorations {
                    guard case let .sprite(sprite) = decoration else { continue }
                    for motion in sprite.motions {
                        if case let .twitch(_, _, delay, duration) = motion {
                            samples.append((delay + duration * 0.22, "短促动作峰值"))
                        }
                    }
                    if sprite.rows > 1, sprite.clips.count > 1 {
                        samples.append((sprite.clips[0].seconds + sprite.clips[1].seconds / 2, "表情帧"))
                    }
                }
                var first: Data?
                for (frame, moment) in samples.enumerated() {
                    let host = CandidateDecorationHostView(frame: CGRect(origin: .zero, size: card.size))
                    host.cardInsets = loaded.overflowInsets
                    host.configure(loaded.decorations.filter { if case .gradient = $0 { return false }; return true })
                    host.layoutSubtreeIfNeeded()
                    host.reduceMotionOverrideForTesting = false
                    host.setRunning(true)
                    let layer = try XCTUnwrap(host.layer)
                    CATransaction.begin()
                    CATransaction.setDisableActions(true)
                    try sample(layer, at: moment.time)
                    CATransaction.commit()
                    let context = try XCTUnwrap(CGContext(data: nil, width: Int(card.size.width * 2), height: Int(card.size.height * 2),
                        bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
                    context.scaleBy(x: 2, y: 2)
                    context.draw(background, in: CGRect(origin: .zero, size: card.size))
                    layer.render(in: context)
                    let bitmap = NSBitmapImageRep(cgImage: try XCTUnwrap(context.makeImage()))
                    let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                    if frame == 0 { first = data }
                    if frame == 3 { XCTAssertNotEqual(data, first, "\(name) must visibly change within three seconds") }
                    try data.write(to: directory.appendingPathComponent(String(format: "%02d.png", frame)))
                    host.setRunning(false)
                }
                let manifest: [[String: Any]] = samples.enumerated().map { index, moment in
                    ["file": String(format: "%02d.png", index), "seconds": moment.time, "label": moment.label]
                }
                try JSONSerialization.data(withJSONObject: ["source": "离线渲染，非真窗口", "samples": manifest],
                    options: [.prettyPrinted, .sortedKeys]).write(to: directory.appendingPathComponent("sampling.json"))
            }
        }
    }

    private func sample(_ layer: CALayer, at time: Double) throws {
        let animations = (layer.animationKeys() ?? []).compactMap { layer.animation(forKey: $0) }
        layer.removeAllAnimations()
        for animation in animations { try sample(animation, on: layer, at: time) }
        for child in layer.sublayers ?? [] { try sample(child, at: time) }
    }

    private func sample(_ animation: CAAnimation, on layer: CALayer, at time: Double, duration inherited: Double = 0) throws {
        let duration = animation.duration > 0 ? animation.duration : inherited
        if let group = animation as? CAAnimationGroup {
            for child in group.animations ?? [] { try sample(child, on: layer, at: time.truncatingRemainder(dividingBy: duration), duration: duration) }
            return
        }
        let property = try XCTUnwrap(animation as? CAPropertyAnimation)
        let key = try XCTUnwrap(property.keyPath)
        let period = duration * (animation.autoreverses ? 2 : 1)
        let elapsed = time.truncatingRemainder(dividingBy: period)
        var fraction = elapsed / duration
        if fraction > 1 { fraction = 2 - fraction }
        let value: Any
        if let basic = animation as? CABasicAnimation {
            let from = try XCTUnwrap(basic.fromValue as? NSNumber).doubleValue
            let to = try XCTUnwrap(basic.toValue as? NSNumber).doubleValue
            value = from + (to - from) * eased(fraction, by: basic.timingFunction)
        } else {
            let frames = try XCTUnwrap(animation as? CAKeyframeAnimation)
            let values = try XCTUnwrap(frames.values)
            let times = try XCTUnwrap(frames.keyTimes).map(\.doubleValue)
            let index = max(0, (times.lastIndex { $0 <= fraction } ?? 0))
            if frames.calculationMode == .discrete || index == values.count - 1 {
                value = values[index]
            } else {
                let progress = (fraction - times[index]) / (times[index + 1] - times[index])
                let t = eased(progress, by: frames.timingFunctions?[index])
                if key == "transform" {
                    let from = try XCTUnwrap(values[index] as? NSValue).caTransform3DValue
                    let to = try XCTUnwrap(values[index + 1] as? NSValue).caTransform3DValue
                    value = NSValue(caTransform3D: CATransform3DMakeTranslation(from.m41 + (to.m41 - from.m41) * t,
                        from.m42 + (to.m42 - from.m42) * t, 0))
                } else {
                    let from = try XCTUnwrap(values[index] as? NSNumber).doubleValue
                    let to = try XCTUnwrap(values[index + 1] as? NSNumber).doubleValue
                    value = from + (to - from) * t
                }
            }
        }
        if property.isAdditive {
            layer.setValue(try XCTUnwrap(layer.value(forKeyPath: key) as? NSNumber).doubleValue
                           + XCTUnwrap(value as? Double), forKeyPath: key)
        } else {
            layer.setValue(value, forKeyPath: key)
        }
    }

    private func eased(_ fraction: Double, by function: CAMediaTimingFunction?) -> Double {
        guard let function else { return fraction }
        var first = [Float](repeating: 0, count: 2), second = first
        function.getControlPoint(at: 1, values: &first)
        function.getControlPoint(at: 2, values: &second)
        func bezier(_ t: Double, _ a: Float, _ b: Float) -> Double {
            3 * (1 - t) * (1 - t) * t * Double(a) + 3 * (1 - t) * t * t * Double(b) + t * t * t
        }
        var low = 0.0, high = 1.0
        for _ in 0..<24 {
            let mid = (low + high) / 2
            if bezier(mid, first[0], second[0]) < fraction { low = mid } else { high = mid }
        }
        return bezier((low + high) / 2, first[1], second[1])
    }
}
