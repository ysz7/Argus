// Generates ground-truth fixtures for visual UI detection: real AppKit
// controls rendered off-screen at 2x in light and dark appearance.
//
//   swift tests/fixtures/vision/generate_controls.swift
//
// Writes controls_{light,dark}.png and controls.json. Rectangles in the JSON
// are in pixels (top-left origin) and describe the drawn control, e.g. only
// the box of a checkbox, not its title.
import AppKit

let size = NSSize(width: 520, height: 360)

struct Truth: Encodable { let role: String; let name: String; let checked: Bool?; let x: Int; let y: Int; let width: Int; let height: Int }

final class Flipped: NSView {
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        dirtyRect.fill()
    }
}

func build() -> (NSView, [(String, String, NSView, (NSView) -> NSRect)]) {
    let root = Flipped(frame: NSRect(origin: .zero, size: size))
    var items: [(String, String, NSView, (NSView) -> NSRect)] = []
    let whole: (NSView) -> NSRect = { $0.frame }
    let cellImage: (NSView) -> NSRect = { view in
        let button = view as! NSButton
        let image = button.cell!.imageRect(forBounds: button.bounds)
        return button.convert(image, to: root)
    }
    func add(_ role: String, _ name: String, _ view: NSView, _ rect: @escaping (NSView) -> NSRect = whole) {
        root.addSubview(view); items.append((role, name, view, rect))
    }

    let title = NSTextField(labelWithString: "Preferences")
    title.font = .boldSystemFont(ofSize: 15); title.frame = NSRect(x: 20, y: 16, width: 200, height: 20)
    root.addSubview(title)

    for (index, name) in ["Save", "Cancel", "Apply"].enumerated() {
        let button = NSButton(title: name, target: nil, action: nil)
        button.bezelStyle = .push; button.frame = NSRect(x: 20 + index * 110, y: 300, width: 100, height: 32)
        if index == 0 { button.keyEquivalent = "\r" }
        add("button", name, button)
    }

    for (index, value) in ["", "hello@example.com"].enumerated() {
        let field = NSTextField(frame: NSRect(x: 20, y: 56 + index * 40, width: 240, height: 24))
        field.stringValue = value; field.placeholderString = index == 0 ? "Search" : nil
        add("text_box", index == 0 ? "Search" : "Email", field)
    }

    for (index, (name, on)) in [("Remember me", true), ("Show hidden files", false)].enumerated() {
        let box = NSButton(checkboxWithTitle: name, target: nil, action: nil)
        box.state = on ? .on : .off; box.frame.origin = NSPoint(x: 20, y: 146 + index * 28)
        add("checkbox", name, box, cellImage)
    }

    for (index, (name, on)) in [("Automatic", true), ("Manual", false)].enumerated() {
        let radio = NSButton(radioButtonWithTitle: name, target: nil, action: nil)
        radio.state = on ? .on : .off; radio.frame.origin = NSPoint(x: 220, y: 146 + index * 28)
        add("radio_button", name, radio, cellImage)
    }

    // Fixed segment widths give each segment a known rectangle.
    let labels = ["General", "Advanced", "Privacy"]
    let tabs = NSSegmentedControl(labels: labels, trackingMode: .selectOne, target: nil, action: nil)
    for index in labels.indices { tabs.setWidth(68, forSegment: index) }
    tabs.selectedSegment = 0; tabs.sizeToFit(); tabs.frame.origin = NSPoint(x: 290, y: 56)
    root.addSubview(tabs)
    for (index, label) in labels.enumerated() {
        let segmentWidth = tabs.frame.width / CGFloat(labels.count)
        let segment = NSRect(x: tabs.frame.minX + CGFloat(index) * segmentWidth, y: tabs.frame.minY,
                             width: segmentWidth, height: tabs.frame.height)
        items.append(("tab", label, tabs, { _ in segment }))
    }

    let popup = NSPopUpButton(frame: NSRect(x: 290, y: 96, width: 150, height: 26), pullsDown: false)
    popup.addItems(withTitles: ["Medium", "Large"])
    add("button", "Medium", popup)

    for (index, symbol) in ["gearshape", "trash", "star"].enumerated() {
        let icon = NSButton(image: NSImage(systemSymbolName: symbol, accessibilityDescription: symbol)!, target: nil, action: nil)
        icon.isBordered = false; icon.frame = NSRect(x: 290 + index * 36, y: 220, width: 24, height: 24)
        add("icon", symbol, icon, cellImage)
    }
    return (root, items)
}

var truth: [Truth] = []
for (suffix, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
    let window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.borderless], backing: .buffered, defer: false)
    window.appearance = NSAppearance(named: appearance)
    let (root, items) = build()
    window.contentView = root
    root.layoutSubtreeIfNeeded()

    let rep = root.bitmapImageRepForCachingDisplay(in: root.bounds)!
    NSAppearance(named: appearance)!.performAsCurrentDrawingAppearance {
        root.cacheDisplay(in: root.bounds, to: rep)
    }
    let scale = CGFloat(rep.pixelsWide) / size.width
    let dir = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
    try! rep.representation(using: .png, properties: [:])!.write(to: dir.appendingPathComponent("controls_\(suffix).png"))

    if suffix == "light" {
        truth = items.map { role, name, view, rect in
            let r = rect(view)
            let checked = (view as? NSButton).flatMap { ["checkbox", "radio_button"].contains(role) ? $0.state == .on : nil }
            return Truth(role: role, name: name, checked: checked, x: Int((r.minX * scale).rounded()), y: Int((r.minY * scale).rounded()),
                         width: Int((r.width * scale).rounded()), height: Int((r.height * scale).rounded()))
        }
        print("scale \(scale), \(rep.pixelsWide)x\(rep.pixelsHigh)")
    }
}
let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
let dir = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
try! encoder.encode(truth).write(to: dir.appendingPathComponent("controls.json"))
print("wrote \(truth.count) ground-truth controls")
