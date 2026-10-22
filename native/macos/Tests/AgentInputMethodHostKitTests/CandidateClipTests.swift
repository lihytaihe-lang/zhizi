import AppKit
import XCTest
@testable import AgentInputMethodHostKit

/// Action sequences: a sprite sheet with one row per action (wave, walk, sit...) played in order, round and round.
@MainActor
final class CandidateClipTests: XCTestCase {
    /// A sheet of `columns` x `rows` coloured cells, as PNG data.
    private func sheet(columns: Int, rows: Int, cell: Int = 8, cellHeight: Int = 6) -> Data {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: columns * cell, pixelsHigh: rows * cellHeight, bitsPerSample: 8,
                                   samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                   bytesPerRow: 0, bitsPerPixel: 0)!
        for row in 0..<rows {
            for column in 0..<columns {
                let color = NSColor(calibratedHue: CGFloat(row * columns + column) / CGFloat(rows * columns), saturation: 1, brightness: 1, alpha: 1)
                for y in 0..<cellHeight { for x in 0..<cell { rep.setColor(color, atX: column * cell + x, y: row * cellHeight + y) } }
            }
        }
        return rep.representation(using: .png, properties: [:])!
    }

    private func parse(_ json: String, assets: [String: Data]) throws -> [CandidateDecoration] {
        let value = try JSONSerialization.jsonObject(with: Data(json.utf8))
        return try CandidateDecorationParser.parse(value, assets: assets) { _, _ in NSColor.white }
    }

    private let clips = """
    [{"row":0,"frames":4,"fps":5,"seconds":2},
     {"row":1,"frames":6,"fps":8,"seconds":3,"move":{"from":0.1,"to":0.7}},
     {"row":2,"frames":3,"fps":6,"seconds":1.5,"flip":true,"move":{"from":0.7,"to":0.1}}]
    """

    private func spriteJSON(_ clips: String, frames: Int = 6, extra: String = "") -> String {
        #"[{"type":"sprite","sheet":"a.png","frames":\#(frames),"size":{"width":32,"height":24},"clips":\#(clips)\#(extra)}]"#
    }

    func testAnActionSequenceParses() throws {
        let decorations = try parse(spriteJSON(clips), assets: ["a.png": sheet(columns: 6, rows: 3)])
        guard case let .sprite(sprite) = try XCTUnwrap(decorations.first) else { return XCTFail("a sprite") }
        XCTAssertEqual(sprite.frames, 6)
        XCTAssertEqual(sprite.rows, 3)
        XCTAssertEqual(sprite.clips.map(\.frames), [4, 6, 3])
        XCTAssertEqual(sprite.clips.map(\.row), [0, 1, 2], "rows default to the clip's place in the list")
        XCTAssertEqual(sprite.clips[1].move?.to ?? 0, 0.7, accuracy: 0.0001)
        XCTAssertTrue(sprite.clips[2].flip)
        XCTAssertFalse(sprite.clips[0].flip)
    }

    func testBadSequencesAreRejected() throws {
        let png = sheet(columns: 6, rows: 3)
        XCTAssertThrowsError(try parse(spriteJSON(#"[{"frames":7,"seconds":1}]"#), assets: ["a.png": png]), "a clip cannot have more frames than the sheet has columns")
        XCTAssertThrowsError(try parse(spriteJSON(#"[{"row":5,"frames":2,"seconds":1}]"#), assets: ["a.png": png]), "the sheet is not tall enough for row 5")
        XCTAssertThrowsError(try parse(spriteJSON(#"[{"frames":2}]"#), assets: ["a.png": png]), "seconds is required")
        XCTAssertThrowsError(try parse(spriteJSON(#"[{"frames":2,"seconds":1,"colour":"red"}]"#), assets: ["a.png": png]), "unknown clip keys")
        XCTAssertThrowsError(try parse(spriteJSON("[]"), assets: ["a.png": png]), "an empty sequence")
        XCTAssertThrowsError(try parse(spriteJSON(clips, extra: #","travel":{"from":0,"to":1,"duration":5}"#), assets: ["a.png": png]),
                             "travel and clips do not mix: the walking is written in each clip's move")
        XCTAssertThrowsError(try parse(spriteJSON(#"[{"frames":2,"fps":30,"seconds":30},{"frames":2,"fps":30,"seconds":30},{"frames":2,"fps":30,"seconds":30}]"#),
                                       assets: ["a.png": png]), "too many frame steps in one round")
        // a sheet whose cells are not the shape `size` asks for
        XCTAssertThrowsError(try parse(spriteJSON(#"[{"frames":2,"seconds":1}]"#), assets: ["a.png": sheet(columns: 6, rows: 3, cell: 8, cellHeight: 7)]))
        // the clips use a row the sheet does not have
        XCTAssertThrowsError(try parse(spriteJSON(#"[{"row":2,"frames":2,"seconds":1}]"#), assets: ["a.png": sheet(columns: 6, rows: 2)]))
        XCTAssertNoThrow(try parse(spriteJSON(#"[{"frames":2,"seconds":1}]"#), assets: ["a.png": png]), "fewer rows used than the sheet holds is fine")
    }

    func testTheSequenceRunsFramesWalksAndTurnsWhileShownAndStopsWithThePanel() throws {
        let decorations = try parse(spriteJSON(clips), assets: ["a.png": sheet(columns: 6, rows: 3)])
        let host = CandidateDecorationHostView(frame: NSRect(x: 0, y: 0, width: 300, height: 60))
        host.cardInsets = NSEdgeInsets()
        host.configure(decorations)
        XCTAssertTrue(host.animationKeysForTesting.isEmpty, "nothing runs before the panel is shown")
        host.setRunning(true)
        if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        } else {
            let keys = Set(host.animationKeysForTesting)
            XCTAssertTrue(keys.isSuperset(of: ["clipFrames", "clipMove", "clipFlip"]), "running: \(keys)")
        }
        host.setRunning(false)
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
    }
}
