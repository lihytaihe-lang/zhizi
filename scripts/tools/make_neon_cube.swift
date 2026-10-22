// Renders a neon tesseract (4D hypercube) turning in a seamless loop as a horizontal PNG strip, with CoreGraphics.
//   swift make_neon_cube.swift <out.png> [--frames 24] [--size 120]
// Two 4D rotations (X-W and Y-Z planes) are projected to 2D with perspective; every edge is a glowing stroke whose colour moves
// from magenta through cyan to lime along the cube, with a faint wide halo under a bright thin core.
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func option(_ name: String, _ fallback: Int) -> Int {
    guard let index = CommandLine.arguments.firstIndex(of: name), index + 1 < CommandLine.arguments.count,
          let value = Int(CommandLine.arguments[index + 1]) else { return fallback }
    return value
}

guard CommandLine.arguments.count >= 2 else {
    FileHandle.standardError.write(Data("usage: make_neon_cube.swift <out.png> [--frames N] [--size N]\n".utf8))
    exit(2)
}
let frames = option("--frames", 24), size = option("--size", 120)
let palette: [(CGFloat, CGFloat, CGFloat)] = [(1.0, 0.18, 0.78), (0.15, 0.85, 1.0), (0.62, 1.0, 0.25), (1.0, 0.62, 0.15)]

// 16 vertices (±1)^4 and the 32 edges that join vertices differing in one coordinate
var vertices: [[Double]] = []
for index in 0..<16 { vertices.append((0..<4).map { index & (1 << $0) == 0 ? -1.0 : 1.0 }) }
var edges: [(Int, Int)] = []
for a in 0..<16 { for bit in 0..<4 { let b = a ^ (1 << bit); if a < b { edges.append((a, b)) } } }

func rotate(_ point: [Double], _ i: Int, _ j: Int, _ angle: Double) -> [Double] {
    var out = point
    out[i] = point[i] * cos(angle) - point[j] * sin(angle)
    out[j] = point[i] * sin(angle) + point[j] * cos(angle)
    return out
}

func project(_ vertex: [Double], phase: Double) -> (x: Double, y: Double, depth: Double) {
    var p = vertex
    p = rotate(p, 0, 3, phase)            // X-W: the cube turns inside out
    p = rotate(p, 1, 2, phase)            // Y-Z: both turn a quarter turn per loop, so the figure repeats exactly
    p = rotate(p, 0, 2, 0.35)              // fixed tilt so it never lines up flat
    let w = 4.5 / (4.5 - p[3])             // 4D -> 3D
    let q = (p[0] * w, p[1] * w, p[2] * w)
    let z = 6.0 / (6.0 - q.2)              // 3D -> 2D
    return (q.0 * z, q.1 * z, q.2)
}

func mix(_ t: CGFloat) -> CGColor {
    let scaled = t * CGFloat(palette.count)
    let index = Int(scaled) % palette.count, next = (index + 1) % palette.count
    let f = scaled - CGFloat(Int(scaled))
    let a = palette[index], b = palette[next]
    return CGColor(srgbRed: a.0 + (b.0 - a.0) * f, green: a.1 + (b.1 - a.1) * f, blue: a.2 + (b.2 - a.2) * f, alpha: 1)
}

let space = CGColorSpace(name: CGColorSpace.sRGB)!
let sheet = CGContext(
    data: nil, width: size * frames, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
)!
sheet.setLineCap(.round)
for frame in 0..<frames {
    let phase = Double(frame) / Double(frames) * (.pi / 2)   // the figure repeats every quarter turn of X-W (symmetry)
    let projected = vertices.map { project($0, phase: phase) }
    let scale = Double(size) * 0.19, centre = Double(size) / 2
    sheet.saveGState()
    sheet.translateBy(x: CGFloat(frame * size), y: 0)
    for (edgeIndex, edge) in edges.enumerated() {
        let a = projected[edge.0], b = projected[edge.1]
        let colour = mix(CGFloat(edgeIndex) / CGFloat(edges.count))
        let nearness = CGFloat(min(1, max(0.35, 0.65 + (a.depth + b.depth) / 8)))
        let start = CGPoint(x: centre + a.x * scale, y: centre + a.y * scale)
        let end = CGPoint(x: centre + b.x * scale, y: centre + b.y * scale)
        for (width, alpha) in [(CGFloat(size) * 0.06, 0.12), (CGFloat(size) * 0.032, 0.28)] {   // halo
            sheet.setStrokeColor(colour.copy(alpha: CGFloat(alpha) * nearness)!)
            sheet.setLineWidth(width)
            sheet.move(to: start); sheet.addLine(to: end); sheet.strokePath()
        }
        sheet.setStrokeColor(colour.copy(alpha: 0.95 * nearness)!)
        sheet.setLineWidth(max(1.2, CGFloat(size) * 0.011))
        sheet.move(to: start); sheet.addLine(to: end); sheet.strokePath()
    }
    sheet.restoreGState()
}
let url = URL(fileURLWithPath: CommandLine.arguments[1])
try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(destination, sheet.makeImage()!, nil)
guard CGImageDestinationFinalize(destination) else { exit(1) }
print("wrote \(url.path) \(size * frames)x\(size) frames=\(frames)")
