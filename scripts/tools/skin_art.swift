// Prepares generated skin illustrations: removes a pure-green background, trims, scales, and cuts sprite sheets into strips.
//   swift skin_art.swift key   <in.png> <out.png> [--max 512] [--pad 4]        green screen -> transparent, trimmed, scaled
//   swift skin_art.swift strip <in.png> <out.png> --grid 4x2 [--frame 176x120]   sprite sheet -> one horizontal strip, frames aligned
//   (add --blobs instead of relying on the grid lines: each frame is found as a connected shape, which survives frames that cross cell borders)
//   swift skin_art.swift strip <in.png> <out.png> --grid 4x2 --feet --height 96  same, aligned on the feet: each row's lowest point is the ground, a frame
//                                                                                that hops keeps its lift, frames are centred horizontally
// The green key keeps pixels whose green does not dominate red and blue; edges are despilled so no green fringe remains.
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

struct Pixels {
    var width: Int
    var height: Int
    var data: [UInt8]  // RGBA, straight alpha

    init(width: Int, height: Int) {
        self.width = width
        self.height = height
        data = [UInt8](repeating: 0, count: width * height * 4)
    }

    init(image: CGImage) {
        self.init(width: image.width, height: image.height)
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        data.withUnsafeMutableBytes { raw in
            let context = CGContext(
                data: raw.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4, space: space,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )!
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        }
        // un-premultiply (inputs are opaque in practice)
        for index in stride(from: 0, to: data.count, by: 4) where data[index + 3] > 0 && data[index + 3] < 255 {
            let alpha = Float(data[index + 3]) / 255
            for channel in 0..<3 { data[index + channel] = UInt8(min(255, Float(data[index + channel]) / alpha)) }
        }
    }

    func makeImage() -> CGImage {
        var copy = data
        for index in stride(from: 0, to: copy.count, by: 4) {  // premultiply for CoreGraphics
            let alpha = Float(copy[index + 3]) / 255
            for channel in 0..<3 { copy[index + channel] = UInt8(Float(copy[index + channel]) * alpha) }
        }
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        let provider = CGDataProvider(data: Data(copy) as CFData)!
        return CGImage(
            width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4, space: space,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue), provider: provider, decode: nil,
            shouldInterpolate: true, intent: .defaultIntent
        )!
    }

    /// Removes a solid background: pure green (default) or pure magenta (`--key magenta`, for subjects that are green themselves).
    mutating func keyGreen(magenta: Bool = false) {
        for index in stride(from: 0, to: data.count, by: 4) {
            let red = Int(data[index]), green = Int(data[index + 1]), blue = Int(data[index + 2])
            let dominance = magenta ? min(red, blue) - green : green - max(red, blue)
            // hot-pink backgrounds from image generators are not pure magenta, so the cut-off is lower for them
            let full = magenta ? 62 : 110
            if dominance <= 28 { continue }
            if dominance >= full {
                data[index] = 0; data[index + 1] = 0; data[index + 2] = 0; data[index + 3] = 0
                continue
            }
            let keep = 1 - Float(dominance - 28) / Float(full - 28)
            data[index + 3] = UInt8(Float(data[index + 3]) * keep)
            if magenta {  // despill: pull red and blue down to the green level
                data[index] = UInt8(min(red, green + 12)); data[index + 2] = UInt8(min(blue, green + 12))
            } else {
                data[index + 1] = UInt8(max(red, blue))  // despill
            }
        }
        // despill the opaque rim: green that still leans over red/blue next to transparent pixels
        for y in 1..<(height - 1) {
            for x in 1..<(width - 1) {
                let index = (y * width + x) * 4
                guard data[index + 3] > 0 else { continue }
                var nearEdge = false
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] where data[((y + dy) * width + x + dx) * 4 + 3] < 40 { nearEdge = true }
                if nearEdge {
                    if magenta {
                        let limit = UInt8(min(255, Int(data[index + 1]) + 12))
                        if data[index] > limit { data[index] = limit }
                        if data[index + 2] > limit { data[index + 2] = limit }
                    } else {
                        let limit = max(data[index], data[index + 2])
                        if data[index + 1] > limit { data[index + 1] = limit }
                    }
                }
            }
        }
    }

    /// The `count` largest connected shapes (neighbouring parts within a few pixels count as one), each as its own image the size of
    /// the sheet with only that shape's pixels; ordered row by row (`rows` bands from the top), left to right.
    func blobs(count: Int, rows: Int) -> [Pixels] {
        let radius = 6
        var mask = [Bool](repeating: false, count: width * height)
        for y in 0..<height { for x in 0..<width where data[(y * width + x) * 4 + 3] > 24 { mask[y * width + x] = true } }
        // dilate horizontally then vertically so detached feet join their body
        func dilate(_ source: [Bool], horizontal: Bool) -> [Bool] {
            var out = source
            for y in 0..<height {
                for x in 0..<width where source[y * width + x] {
                    for d in -radius...radius {
                        let nx = horizontal ? x + d : x, ny = horizontal ? y : y + d
                        if nx >= 0, nx < width, ny >= 0, ny < height { out[ny * width + nx] = true }
                    }
                }
            }
            return out
        }
        let joined = dilate(dilate(mask, horizontal: true), horizontal: false)
        var label = [Int](repeating: -1, count: width * height)
        var sizes: [Int] = []
        for start in 0..<(width * height) where joined[start] && label[start] < 0 {
            let id = sizes.count
            var stack = [start], size = 0
            label[start] = id
            while let current = stack.popLast() {
                size += 1
                let x = current % width, y = current / width
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let nx = x + dx, ny = y + dy
                    if nx >= 0, nx < width, ny >= 0, ny < height, joined[ny * width + nx], label[ny * width + nx] < 0 {
                        label[ny * width + nx] = id
                        stack.append(ny * width + nx)
                    }
                }
            }
            sizes.append(size)
        }
        let largest = sizes.indices.sorted { sizes[$0] > sizes[$1] }.prefix(count)
        var found: [(id: Int, centerX: Int, centerY: Int)] = []
        for id in largest {
            var minX = width, maxX = 0, minY = height, maxY = 0
            for y in 0..<height { for x in 0..<width where label[y * width + x] == id { minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y) } }
            found.append((id, (minX + maxX) / 2, (minY + maxY) / 2))
        }
        found.sort { $0.centerY < $1.centerY }
        let perRow = max(1, count / rows)
        var ordered: [(id: Int, centerX: Int, centerY: Int)] = []
        for row in 0..<rows {
            ordered += found[(row * perRow)..<min(found.count, (row + 1) * perRow)].sorted { $0.centerX < $1.centerX }
        }
        return ordered.map { item in
            var shape = Pixels(width: width, height: height)
            for index in 0..<(width * height) where label[index] == item.id && data[index * 4 + 3] > 0 {
                for channel in 0..<4 { shape.data[index * 4 + channel] = data[index * 4 + channel] }
            }
            return shape
        }
    }

    /// Bounding box of pixels with alpha above the threshold.
    func opaqueBounds(threshold: UInt8 = 24) -> (minX: Int, minY: Int, maxX: Int, maxY: Int)? {
        var minX = width, minY = height, maxX = -1, maxY = -1
        for y in 0..<height {
            for x in 0..<width where data[(y * width + x) * 4 + 3] > threshold {
                minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
            }
        }
        return maxX < 0 ? nil : (minX, minY, maxX, maxY)
    }
}

func load(_ path: String) -> CGImage {
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        FileHandle.standardError.write(Data("cannot read \(path)\n".utf8))
        exit(1)
    }
    return image
}

func write(_ image: CGImage, to path: String) {
    let url = URL(fileURLWithPath: path)
    try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else { exit(1) }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else { exit(1) }
}

func scaled(_ image: CGImage, to size: CGSize) -> CGImage {
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let context = CGContext(
        data: nil, width: Int(size.width), height: Int(size.height), bitsPerComponent: 8, bytesPerRow: 0, space: space,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    context.interpolationQuality = .high
    context.draw(image, in: CGRect(origin: .zero, size: size))
    return context.makeImage()!
}

func cropped(_ pixels: Pixels, _ rect: (minX: Int, minY: Int, maxX: Int, maxY: Int), pad: Int) -> Pixels {
    let width = rect.maxX - rect.minX + 1 + pad * 2, height = rect.maxY - rect.minY + 1 + pad * 2
    var out = Pixels(width: width, height: height)
    for y in 0..<height {
        for x in 0..<width {
            let sx = rect.minX - pad + x, sy = rect.minY - pad + y
            guard sx >= 0, sx < pixels.width, sy >= 0, sy < pixels.height else { continue }
            for channel in 0..<4 { out.data[(y * width + x) * 4 + channel] = pixels.data[(sy * pixels.width + sx) * 4 + channel] }
        }
    }
    return out
}

func option(_ name: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: name), index + 1 < CommandLine.arguments.count else { return nil }
    return CommandLine.arguments[index + 1]
}

let arguments = CommandLine.arguments
guard arguments.count >= 4 else {
    FileHandle.standardError.write(Data("usage: skin_art.swift key|strip <in> <out> [options]\n".utf8))
    exit(2)
}
let command = arguments[1], input = arguments[2], output = arguments[3]
var pixels = Pixels(image: load(input))

switch command {
case "key":
    pixels.keyGreen(magenta: option("--key") == "magenta")
    guard let bounds = pixels.opaqueBounds() else { print("nothing left after keying"); exit(1) }
    pixels = cropped(pixels, bounds, pad: Int(option("--pad") ?? "4")!)
    var image = pixels.makeImage()
    if let maximum = option("--max").flatMap({ CGFloat(Int($0)!) }), max(CGFloat(image.width), CGFloat(image.height)) > maximum {
        let ratio = maximum / max(CGFloat(image.width), CGFloat(image.height))
        image = scaled(image, to: CGSize(width: (CGFloat(image.width) * ratio).rounded(), height: (CGFloat(image.height) * ratio).rounded()))
    }
    write(image, to: output)
    print("wrote \(output) \(image.width)x\(image.height)")
case "strip":
    guard let grid = option("--grid")?.split(separator: "x").compactMap({ Int($0) }), grid.count == 2 else { exit(2) }
    let columns = grid[0], rows = grid[1]
    let cellWidth = pixels.width / columns, cellHeight = pixels.height / rows
    pixels.keyGreen(magenta: option("--key") == "magenta")
    // crop every cell, then use one union box (anchored on the feet: bottom edge) so the frames stay aligned
    var cells: [Pixels] = []
    var boxes: [(Int, Int, Int, Int)] = []
    if arguments.contains("--blobs") {
        for blob in pixels.blobs(count: rows * columns, rows: rows) {
            cells.append(blob)
            if let box = blob.opaqueBounds() { boxes.append(box) }
        }
    } else {
        for row in 0..<rows {
            for column in 0..<columns {
                let cell = cropped(pixels, (column * cellWidth, row * cellHeight, (column + 1) * cellWidth - 1, (row + 1) * cellHeight - 1), pad: 0)
                cells.append(cell)
                if let box = cell.opaqueBounds() { boxes.append(box) }
            }
        }
    }
    if arguments.contains("--feet") {
        let height = Int(option("--height") ?? "96")!
        let pad = 2
        // ground of each row = the lowest foot among its frames; lift = how far a frame's feet are above that ground
        var infos: [(cell: Pixels, box: (minX: Int, minY: Int, maxX: Int, maxY: Int), lift: Int)] = []
        for row in 0..<rows {
            let rowCells = Array(cells[(row * columns)..<((row + 1) * columns)])
            let rowBoxes = rowCells.compactMap { $0.opaqueBounds() }
            let ground = rowBoxes.map(\.maxY).max()!
            for cell in rowCells {
                let box = cell.opaqueBounds()!
                infos.append((cell, box, ground - box.maxY))
            }
        }
        let maxHeight = infos.map { $0.box.maxY - $0.box.minY + 1 + $0.lift }.max()!
        let maxWidth = infos.map { $0.box.maxX - $0.box.minX + 1 }.max()!
        let scale = CGFloat(height - pad * 2) / CGFloat(maxHeight)
        let frameWidth = Int((CGFloat(maxWidth) * scale).rounded(.up)) + pad * 2
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        let sheet = CGContext(
            data: nil, width: frameWidth * infos.count, height: height, bitsPerComponent: 8, bytesPerRow: 0, space: space,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        sheet.interpolationQuality = .high
        for (index, info) in infos.enumerated() {
            let part = cropped(info.cell, info.box, pad: 0).makeImage()
            let w = CGFloat(info.box.maxX - info.box.minX + 1) * scale, h = CGFloat(info.box.maxY - info.box.minY + 1) * scale
            // CoreGraphics is y-up: the ground sits `pad` above the bottom, a lifted frame is drawn higher
            sheet.draw(part, in: CGRect(
                x: CGFloat(index * frameWidth) + (CGFloat(frameWidth) - w) / 2, y: CGFloat(pad) + CGFloat(info.lift) * scale, width: w, height: h
            ))
        }
        write(sheet.makeImage()!, to: output)
        print("wrote \(output) \(frameWidth * infos.count)x\(height) frames=\(infos.count) frame=\(frameWidth)x\(height)")
        exit(0)
    }
    let unionMinX = boxes.map(\.0).min()!, unionMinY = boxes.map(\.1).min()!
    let unionMaxX = boxes.map(\.2).max()!, unionMaxY = boxes.map(\.3).max()!
    let frameSize = option("--frame")?.split(separator: "x").compactMap { Int($0) }
    let frameWidth = frameSize?.first ?? (unionMaxX - unionMinX + 1), frameHeight = frameSize?.last ?? (unionMaxY - unionMinY + 1)
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let sheet = CGContext(
        data: nil, width: frameWidth * cells.count, height: frameHeight, bitsPerComponent: 8, bytesPerRow: 0, space: space,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    sheet.interpolationQuality = .high
    for (index, cell) in cells.enumerated() {
        let part = cropped(cell, (unionMinX, unionMinY, unionMaxX, unionMaxY), pad: 0).makeImage()
        sheet.draw(part, in: CGRect(x: index * frameWidth, y: 0, width: frameWidth, height: frameHeight))
    }
    write(sheet.makeImage()!, to: output)
    print("wrote \(output) \(frameWidth * cells.count)x\(frameHeight) frames=\(cells.count) union=\(unionMaxX - unionMinX + 1)x\(unionMaxY - unionMinY + 1)")
default:
    exit(2)
}
