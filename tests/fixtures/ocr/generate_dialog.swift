// Generates tests/fixtures/ocr/dialog.png and dialog.json: a synthetic dialog
// of 480x240 points rendered at 2x with known labels (positions in pixels),
// used as OCR ground truth.
//
//   swift tests/fixtures/ocr/generate_dialog.swift
import AppKit

let scale: CGFloat = 2
let size = NSSize(width: 480, height: 240) // points
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale), pixelsHigh: Int(size.height * scale),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = size

struct Label: Encodable { let text: String; let x: Int; let y: Int; let width: Int; let height: Int }
var labels: [Label] = []

NSGraphicsContext.saveGraphicsState()
let context = NSGraphicsContext(bitmapImageRep: rep)!
NSGraphicsContext.current = NSGraphicsContext(cgContext: context.cgContext, flipped: true)
context.cgContext.translateBy(x: 0, y: size.height)
context.cgContext.scaleBy(x: 1, y: -1)

NSColor(white: 0.95, alpha: 1).setFill()
NSRect(origin: .zero, size: size).fill()

func text(_ string: String, at point: NSPoint, size fontSize: CGFloat, bold: Bool = false) {
    let font = bold ? NSFont.boldSystemFont(ofSize: fontSize) : NSFont.systemFont(ofSize: fontSize)
    let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.black]
    let extent = (string as NSString).size(withAttributes: attributes)
    (string as NSString).draw(at: point, withAttributes: attributes)
    labels.append(Label(text: string, x: Int(point.x * scale), y: Int(point.y * scale),
                        width: Int(ceil(extent.width * scale)), height: Int(ceil(extent.height * scale))))
}

func button(_ title: String, _ rect: NSRect) {
    NSColor.white.setFill()
    NSBezierPath(roundedRect: rect, xRadius: 6, yRadius: 6).fill()
    NSColor(white: 0.7, alpha: 1).setStroke()
    NSBezierPath(roundedRect: rect, xRadius: 6, yRadius: 6).stroke()
    let font = NSFont.systemFont(ofSize: 13)
    let extent = (title as NSString).size(withAttributes: [.font: font])
    text(title, at: NSPoint(x: rect.midX - extent.width / 2, y: rect.midY - extent.height / 2), size: 13)
}

text("Delete file?", at: NSPoint(x: 24, y: 20), size: 17, bold: true)
text("This action cannot be undone.", at: NSPoint(x: 24, y: 52), size: 13)
text("Сохранить изменения", at: NSPoint(x: 24, y: 84), size: 13)
text("Total: 12 345", at: NSPoint(x: 24, y: 116), size: 13)
button("Cancel", NSRect(x: 240, y: 180, width: 100, height: 32))
button("Delete", NSRect(x: 356, y: 180, width: 100, height: 32))

NSGraphicsContext.restoreGraphicsState()

let dir = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
try! rep.representation(using: .png, properties: [:])!.write(to: dir.appendingPathComponent("dialog.png"))
let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
try! encoder.encode(labels).write(to: dir.appendingPathComponent("dialog.json"))
print("wrote dialog.png with \(labels.count) labels")
