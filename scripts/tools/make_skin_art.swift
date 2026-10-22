// Draws the sprite sheets of the built-in animated skins with CoreGraphics (no image tools needed).
//   swift scripts/tools/make_skin_art.swift <out dir>
// Writes <out dir>/shuitun/capybara.png: 8 frames side by side, each 88x60 px (shown at 44x30 pt on a 2x display).
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let frameWidth = 88, frameHeight = 60, frames = 8

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255, alpha: alpha
    )
}

func capybaraFrame(_ context: CGContext, phase: CGFloat, originX: CGFloat) {
    context.saveGState()
    context.translateBy(x: originX, y: 0)
    context.setLineCap(.round)
    context.setLineJoin(.round)
    let bob = abs(sin(phase)) * 3.2
    let ground: CGFloat = 6

    func leg(hipX: CGFloat, phaseShift: CGFloat, shade: UInt32) {
        let swing = sin(phase + phaseShift)
        let lift = max(0, cos(phase + phaseShift)) * 3.5
        let hip = CGPoint(x: hipX, y: ground + 17 + bob)
        let foot = CGPoint(x: hipX + swing * 11, y: ground + lift)
        context.setStrokeColor(color(shade))
        context.setLineWidth(6.5)
        context.move(to: hip)
        context.addLine(to: foot)
        context.strokePath()
        context.setFillColor(color(0x5A3A28))
        context.fillEllipse(in: CGRect(x: foot.x - 3.4, y: foot.y - 2, width: 7, height: 4.2))
    }

    // far-side legs first (darker), then the body, then the near-side legs
    leg(hipX: 24, phaseShift: .pi, shade: 0x8A603F)
    leg(hipX: 54, phaseShift: 0, shade: 0x8A603F)

    context.setFillColor(color(0xB8855B))
    context.fillEllipse(in: CGRect(x: 8, y: ground + 12 + bob, width: 58, height: 29))   // body
    context.setFillColor(color(0xD2A878))
    context.fillEllipse(in: CGRect(x: 18, y: ground + 13 + bob, width: 38, height: 14))  // belly
    context.setFillColor(color(0xA9764C))
    context.fillEllipse(in: CGRect(x: 5, y: ground + 19 + bob, width: 10, height: 10))   // rump

    // head: a blunt, boxy muzzle
    let head = CGRect(x: 54, y: ground + 15 + bob, width: 31, height: 25)
    context.setFillColor(color(0xB8855B))
    context.addPath(CGPath(roundedRect: head, cornerWidth: 11, cornerHeight: 11, transform: nil))
    context.fillPath()
    context.setFillColor(color(0xC99B6F))
    context.addPath(CGPath(roundedRect: CGRect(x: 71, y: ground + 15 + bob, width: 14, height: 14), cornerWidth: 6, cornerHeight: 6, transform: nil))
    context.fillPath()
    // ear
    context.setFillColor(color(0x8A603F))
    context.fillEllipse(in: CGRect(x: 58, y: ground + 36 + bob, width: 8, height: 9))
    context.setFillColor(color(0x5A3A28))
    context.fillEllipse(in: CGRect(x: 60, y: ground + 38 + bob, width: 4, height: 5))
    // eye and nose
    context.setFillColor(color(0x2B1B12))
    context.fillEllipse(in: CGRect(x: 69, y: ground + 29 + bob, width: 5, height: 5.5))
    context.setFillColor(color(0xFFFFFF))
    context.fillEllipse(in: CGRect(x: 70.6, y: ground + 31.6 + bob, width: 1.8, height: 1.8))
    context.setFillColor(color(0x5A3A28))
    context.fillEllipse(in: CGRect(x: 80, y: ground + 26 + bob, width: 6, height: 5))
    // cheek blush
    context.setFillColor(color(0xE59A86, 0.55))
    context.fillEllipse(in: CGRect(x: 66, y: ground + 21 + bob, width: 6, height: 4))

    leg(hipX: 20, phaseShift: 0.4, shade: 0xA9764C)
    leg(hipX: 50, phaseShift: .pi + 0.4, shade: 0xA9764C)
    context.restoreGState()
}

func writePNG(_ image: CGImage, to url: URL) throws {
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        throw CocoaError(.fileWriteUnknown)
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else { throw CocoaError(.fileWriteUnknown) }
}

guard CommandLine.arguments.count == 2 else {
    FileHandle.standardError.write(Data("usage: make_skin_art.swift <out dir>\n".utf8))
    exit(2)
}
let outDirectory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true).appendingPathComponent("shuitun", isDirectory: true)
try FileManager.default.createDirectory(at: outDirectory, withIntermediateDirectories: true)
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let context = CGContext(
    data: nil, width: frameWidth * frames, height: frameHeight, bitsPerComponent: 8, bytesPerRow: 0, space: space,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
)!
context.setShouldAntialias(true)
for index in 0..<frames {
    capybaraFrame(context, phase: CGFloat(index) / CGFloat(frames) * 2 * .pi, originX: CGFloat(index * frameWidth))
}
try writePNG(context.makeImage()!, to: outDirectory.appendingPathComponent("capybara.png"))
print("wrote \(outDirectory.path)/capybara.png \(frameWidth * frames)x\(frameHeight)")
