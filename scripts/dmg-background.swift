// Draws the background of the .dmg's window (scripts/make-dmg.sh): the light
// sidebar's gray, an arrow from where the app sits to where Applications sits,
// and "Drag Claktory to Applications" under them, in the app's colors and font
// (docs/IDENTITY.md).
//
//   swift scripts/dmg-background.swift
//
// Writes scripts/dmg/background.png (660 x 400) and background@2x.png, which
// are checked in. make-dmg.sh puts the app's icon at (165, 190) and
// Applications at (495, 190), as this draws them; change both together.

import AppKit
import CoreText

let root = FileManager.default.currentDirectoryPath
func hex(_ value: Int) -> NSColor {
    NSColor(srgbRed: CGFloat((value >> 16) & 0xFF) / 255, green: CGFloat((value >> 8) & 0xFF) / 255, blue: CGFloat(value & 0xFF) / 255, alpha: 1)
}
// The light theme (app/lib/identity.dart).
let surface = hex(0xF5F5F6) // sidebar
let muted = hex(0x52525B) // textMuted
let faint = hex(0x8B8B94) // textFaint

for name in ["Geist-Medium.ttf", "Geist-Regular.ttf"] {
    let url = URL(fileURLWithPath: root + "/app/fonts/" + name)
    if FileManager.default.fileExists(atPath: url.path) {
        CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
    }
}

let width = 660.0, height = 400.0
// The icons' centers, from the top left as Finder places them.
let iconY = 190.0, appX = 165.0, applicationsX = 495.0

func draw(scale: CGFloat, to path: String) {
    let pixelsWide = Int(width * scale), pixelsHigh = Int(height * scale)
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: pixelsWide, pixelsHigh: pixelsHigh, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    rep.size = NSSize(width: width, height: height)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

    surface.setFill()
    NSRect(x: 0, y: 0, width: width, height: height).fill()

    // Drawing is from the bottom left.
    let y = height - iconY
    let start = appX + 92, end = applicationsX - 92
    let arrow = NSBezierPath()
    arrow.lineWidth = 3
    arrow.lineCapStyle = .round
    arrow.lineJoinStyle = .round
    arrow.move(to: NSPoint(x: start, y: y))
    arrow.line(to: NSPoint(x: end, y: y))
    arrow.move(to: NSPoint(x: end - 14, y: y + 14))
    arrow.line(to: NSPoint(x: end, y: y))
    arrow.line(to: NSPoint(x: end - 14, y: y - 14))
    faint.setStroke()
    arrow.stroke()

    let font = NSFont(name: "Geist-Medium", size: 15) ?? NSFont.systemFont(ofSize: 15, weight: .medium)
    let paragraph = NSMutableParagraphStyle()
    paragraph.alignment = .center
    let text = NSAttributedString(
        string: "Drag Claktory to Applications",
        attributes: [.font: font, .foregroundColor: muted, .paragraphStyle: paragraph])
    let textHeight = text.size().height
    text.draw(in: NSRect(x: 0, y: 64 - textHeight / 2, width: width, height: textHeight))

    NSGraphicsContext.restoreGraphicsState()
    let png = rep.representation(using: .png, properties: [:])!
    try! png.write(to: URL(fileURLWithPath: path))
}

try! FileManager.default.createDirectory(atPath: root + "/scripts/dmg", withIntermediateDirectories: true)
draw(scale: 1, to: root + "/scripts/dmg/background.png")
draw(scale: 2, to: root + "/scripts/dmg/background@2x.png")
print("wrote scripts/dmg/background.png and background@2x.png")
