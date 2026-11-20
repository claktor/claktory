// Draws Claktory's app icons (docs/IDENTITY.md, "App icon") in the app's
// colors: a flat white cog with a hard offset shadow in Rivet's blue on a
// graphite tile for Claktory, and the inverse, a graphite cog on a light tile
// with the same blue shadow and a "DEV" label, for Claktory Dev.
//
//   swift scripts/icons.swift
//
// Writes app/macos/Runner/Assets.xcassets/AppIcon.appiconset/app_icon_*.png
// and app/macos/dev/AppIcon.icns, and for iOS the 1024px square of
// app/ios/Runner/Assets.xcassets/AppIcon.appiconset and app/ios/dev's
// icons. Run it from the repo root after changing the design; the outputs
// are checked in.

import AppKit
import CoreText

let root = FileManager.default.currentDirectoryPath
func hex(_ value: Int) -> NSColor {
    NSColor(srgbRed: CGFloat((value >> 16) & 0xFF) / 255, green: CGFloat((value >> 8) & 0xFF) / 255, blue: CGFloat(value & 0xFF) / 255, alpha: 1)
}
// The app's palette (app/lib/identity.dart).
let graphite = hex(0x151517) // dark theme's sidebar
let light = hex(0xF5F5F6) // light theme's sidebar
let white = hex(0xFFFFFF)
let blue = hex(0x2F6FEB) // Rivet's blue, the agent color "blue"

struct Style {
    let tile: NSColor
    let cog: NSColor
    let shadow: NSColor
    let label: String?
}

let claktory = Style(tile: graphite, cog: white, shadow: blue, label: nil)
let dev = Style(tile: light, cog: graphite, shadow: blue, label: "DEV")

// The label uses the app's own font when it's there.
let geist = URL(fileURLWithPath: root + "/app/fonts/Geist-SemiBold.ttf")
if FileManager.default.fileExists(atPath: geist.path) {
    CTFontManagerRegisterFontsForURL(geist as CFURL, .process, nil)
}

/// A cog centered at `center`: `teeth` trapezoid teeth from `body` out to
/// `tip`, with a round hole. Corners are rounded by stroking it as it's filled.
func cog(center: NSPoint, body: CGFloat, tip: CGFloat, hole: CGFloat, teeth: Int) -> NSBezierPath {
    let path = NSBezierPath()
    let step = 360.0 / CGFloat(teeth)
    let baseHalf = step * 0.26
    let tipHalf = step * 0.17
    func point(_ radius: CGFloat, _ degrees: CGFloat) -> NSPoint {
        let radians = (degrees + 90) * .pi / 180
        return NSPoint(x: center.x + radius * cos(radians), y: center.y + radius * sin(radians))
    }
    for tooth in 0..<teeth {
        let angle = CGFloat(tooth) * step
        let start = point(body, angle - baseHalf)
        if tooth == 0 { path.move(to: start) } else { path.line(to: start) }
        path.line(to: point(tip, angle - tipHalf))
        path.line(to: point(tip, angle + tipHalf))
        path.line(to: point(body, angle + baseHalf))
        path.appendArc(withCenter: center, radius: body, startAngle: angle + baseHalf + 90, endAngle: angle + step - baseHalf + 90)
    }
    path.close()
    path.appendOval(in: NSRect(x: center.x - hole, y: center.y - hole, width: hole * 2, height: hole * 2))
    path.windingRule = .evenOdd
    return path
}

/// One icon on macOS's 1024 grid: an 824pt flat tile with a soft shadow
/// beneath, and a hard offset shadow under the mark. For iOS (`square`) the
/// tile is square and shadowless, as iOS rounds it itself.
func draw(_ style: Style, square: Bool = false) {
    let tile = NSRect(x: 100, y: 100, width: 824, height: 824)
    let shape = square ? NSBezierPath(rect: tile) : NSBezierPath(roundedRect: tile, xRadius: 185, yRadius: 185)
    NSGraphicsContext.saveGraphicsState()
    let shadow = NSShadow()
    shadow.shadowColor = NSColor(white: 0, alpha: 0.3)
    shadow.shadowOffset = NSSize(width: 0, height: -10)
    shadow.shadowBlurRadius = 24
    if !square { shadow.set() }
    style.tile.setFill()
    shape.fill()
    NSGraphicsContext.restoreGraphicsState()

    let labeled = style.label != nil
    let scale: CGFloat = labeled ? 0.8 : 0.94
    let center = NSPoint(x: 500, y: labeled ? 572 : 524)
    let offset = 34 * scale
    func gear(at point: NSPoint) -> NSBezierPath {
        let path = cog(center: point, body: 222 * scale, tip: 300 * scale, hole: 112 * scale, teeth: 8)
        path.lineJoinStyle = .round
        path.lineWidth = 28 * scale
        return path
    }
    for (color, point) in [(style.shadow, NSPoint(x: center.x + offset, y: center.y - offset)), (style.cog, center)] {
        let path = gear(at: point)
        color.setFill()
        color.setStroke()
        path.fill()
        path.stroke()
    }

    if let label = style.label {
        let font = NSFont(name: "Geist-SemiBold", size: 128) ?? NSFont.systemFont(ofSize: 128, weight: .semibold)
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: graphite, .kern: 20]
        let text = NSAttributedString(string: label, attributes: attributes)
        let size = text.size()
        text.draw(at: NSPoint(x: 512 - size.width / 2 + 10, y: 150))
    }
}

func png(_ style: Style, pixels: Int, square: Bool = false) -> Data {
    // Drawn in sRGB, so the identity's hex colors land exactly.
    let context = CGContext(
        data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: (square ? CGImageAlphaInfo.noneSkipLast : CGImageAlphaInfo.premultipliedLast).rawValue)!
    context.interpolationQuality = .high
    if square {
        // The tile fills the image, opaque, as iOS wants.
        context.scaleBy(x: CGFloat(pixels) / 824, y: CGFloat(pixels) / 824)
        context.translateBy(x: -100, y: -100)
    } else {
        context.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)
    }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
    draw(style, square: square)
    NSGraphicsContext.restoreGraphicsState()
    return NSBitmapImageRep(cgImage: context.makeImage()!).representation(using: .png, properties: [:])!
}

func write(_ data: Data, _ path: String) {
    try! data.write(to: URL(fileURLWithPath: path))
}

let catalog = root + "/app/macos/Runner/Assets.xcassets/AppIcon.appiconset"
for pixels in [16, 32, 64, 128, 256, 512, 1024] {
    write(png(claktory, pixels: pixels), "\(catalog)/app_icon_\(pixels).png")
}

let devDir = root + "/app/macos/dev"
let iconset = NSTemporaryDirectory() + "FactoryDev.iconset"
try? FileManager.default.removeItem(atPath: iconset)
try! FileManager.default.createDirectory(atPath: devDir, withIntermediateDirectories: true)
try! FileManager.default.createDirectory(atPath: iconset, withIntermediateDirectories: true)
for points in [16, 32, 128, 256, 512] {
    write(png(dev, pixels: points), "\(iconset)/icon_\(points)x\(points).png")
    write(png(dev, pixels: points * 2), "\(iconset)/icon_\(points)x\(points)@2x.png")
}
let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset, "-o", devDir + "/AppIcon.icns"]
try! iconutil.run()
iconutil.waitUntilExit()

// iOS: one 1024px square in the catalog, which Xcode scales; Claktory Dev's
// as loose files the dev build points at (scripts/ios.sh), the way the Mac's
// dev build uses AppIcon.icns.
let iosCatalog = root + "/app/ios/Runner/Assets.xcassets/AppIcon.appiconset"
write(png(claktory, pixels: 1024, square: true), "\(iosCatalog)/Icon-App-1024x1024@1x.png")
let iosDev = root + "/app/ios/dev"
try! FileManager.default.createDirectory(atPath: iosDev, withIntermediateDirectories: true)
for (name, pixels) in [("AppIcon60x60@2x", 120), ("AppIcon60x60@3x", 180), ("AppIcon76x76@2x~ipad", 152), ("AppIcon83.5x83.5@2x~ipad", 167)] {
    write(png(dev, pixels: pixels, square: true), "\(iosDev)/\(name).png")
}
print("wrote \(catalog), \(devDir)/AppIcon.icns, \(iosCatalog) and \(iosDev)")
