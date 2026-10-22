// Renders the project's own menu-bar mark (native/macos/Resources/AgentInputMethodMenu.pdf, a rounded square with a Z) as a
// flat grey PNG with transparent background, for the default skin's right-hand watermark.
//   swift make_watermark.swift <in.pdf> <out.png> [--size 64] [--gray 0.5] [--alpha 1]
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func option(_ name: String, _ fallback: Double) -> Double {
    guard let i = CommandLine.arguments.firstIndex(of: name), i + 1 < CommandLine.arguments.count, let v = Double(CommandLine.arguments[i + 1]) else { return fallback }
    return v
}
guard CommandLine.arguments.count >= 3, let pdf = CGPDFDocument(URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL), let page = pdf.page(at: 1) else {
    FileHandle.standardError.write(Data("usage: make_watermark.swift <in.pdf> <out.png> [--size N] [--gray G]\n".utf8)); exit(2)
}
let size = Int(option("--size", 64)), gray = CGFloat(option("--gray", 0.5)), alpha = CGFloat(option("--alpha", 1))
let box = page.getBoxRect(.mediaBox)
let space = CGColorSpace(name: CGColorSpace.sRGB)!
// 1. the mark as an alpha mask (the PDF is black-on-transparent)
let mask = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
let scale = CGFloat(size) / max(box.width, box.height)
mask.translateBy(x: (CGFloat(size) - box.width * scale) / 2, y: (CGFloat(size) - box.height * scale) / 2)
mask.scaleBy(x: scale, y: scale)
mask.translateBy(x: -box.minX, y: -box.minY)
mask.drawPDFPage(page)
let rendered = mask.makeImage()!
// 2. fill the mark's shape with one grey
let out = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
out.clip(to: CGRect(x: 0, y: 0, width: size, height: size), mask: rendered)
out.setFillColor(CGColor(srgbRed: gray, green: gray, blue: gray, alpha: alpha))
out.fill(CGRect(x: 0, y: 0, width: size, height: size))
let url = URL(fileURLWithPath: CommandLine.arguments[2])
let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(dest, out.makeImage()!, nil)
guard CGImageDestinationFinalize(dest) else { exit(1) }
print("wrote \(url.path) \(size)x\(size)")
