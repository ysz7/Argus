// A benchmark application with known contents.
//
//     swift benchmarks/apps/form.swift [--dark] --truth <file.json>
//
// Opens one window (without taking focus) with standard AppKit controls,
// which the accessibility tree describes, and a canvas with drawn controls,
// which it does not (like custom toolkits and games). The canvas contents are
// written to the truth file in global screen points, the way
// `argus benchmark record --extra-truth` expects them.
//
// Prints `ready <pid>` when the window is on screen, then reads commands from
// stdin, one per line, and answers `ok <command>` after each:
//
//     check   toggle the "Remember me" checkbox
//     row     append a row to the table
//     canvas  toggle the drawn checkbox and disable the drawn "Erase" button
//     quit
//
// All text is synthetic: nothing private is ever shown.

import AppKit

let arguments = CommandLine.arguments
let dark = arguments.contains("--dark")
guard let truthIndex = arguments.firstIndex(of: "--truth"), truthIndex + 1 < arguments.count else {
    FileHandle.standardError.write("usage: form.swift [--dark] --truth <file.json>\n".data(using: .utf8)!)
    exit(2)
}
let truthPath = arguments[truthIndex + 1]

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
if dark { app.appearance = NSAppearance(named: .darkAqua) } else { app.appearance = NSAppearance(named: .aqua) }

// MARK: - Drawn controls

struct Drawn {
    var role: String
    var name: String
    var rect: NSRect  // in canvas coordinates (flipped: top-left origin)
    var enabled = true
    var checked: Bool? = nil
}

final class Canvas: NSView {
    var items: [Drawn] = [
        Drawn(role: "button", name: "Draw", rect: NSRect(x: 12, y: 12, width: 62, height: 26)),
        Drawn(role: "button", name: "Erase", rect: NSRect(x: 80, y: 12, width: 62, height: 26)),
        Drawn(role: "button", name: "Fill", rect: NSRect(x: 148, y: 12, width: 62, height: 26)),
        Drawn(role: "button", name: "Export", rect: NSRect(x: 216, y: 12, width: 62, height: 26)),
        Drawn(role: "checkbox", name: "Snap to grid", rect: NSRect(x: 12, y: 52, width: 16, height: 16), checked: true),
        Drawn(role: "text", name: "Snap to grid", rect: NSRect(x: 34, y: 51, width: 86, height: 18)),
        Drawn(role: "text", name: "Layers: 3", rect: NSRect(x: 12, y: 84, width: 70, height: 18)),
    ]

    override var isFlipped: Bool { true }
    // Invisible to accessibility, like a custom toolkit.
    override func isAccessibilityElement() -> Bool { false }
    override func accessibilityChildren() -> [Any]? { [] }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.blended(withFraction: 0.06, of: .labelColor)!.setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 8, yRadius: 8).fill()
        let font = NSFont.systemFont(ofSize: 13)
        for item in items {
            let textColor = item.enabled ? NSColor.labelColor : NSColor.tertiaryLabelColor
            switch item.role {
            case "button":
                let path = NSBezierPath(roundedRect: item.rect, xRadius: 6, yRadius: 6)
                NSColor.controlBackgroundColor.setFill(); path.fill()
                NSColor.separatorColor.setStroke(); path.lineWidth = 1; path.stroke()
                let text = NSAttributedString(string: item.name, attributes: [.font: font, .foregroundColor: textColor])
                let size = text.size()
                text.draw(at: NSPoint(x: item.rect.midX - size.width / 2, y: item.rect.midY - size.height / 2))
            case "checkbox":
                let path = NSBezierPath(roundedRect: item.rect, xRadius: 3, yRadius: 3)
                if item.checked == true {
                    NSColor.controlAccentColor.setFill(); path.fill()
                    let mark = NSBezierPath()
                    mark.move(to: NSPoint(x: item.rect.minX + 4, y: item.rect.midY))
                    mark.line(to: NSPoint(x: item.rect.midX - 1, y: item.rect.maxY - 4))
                    mark.line(to: NSPoint(x: item.rect.maxX - 3, y: item.rect.minY + 4))
                    NSColor.white.setStroke(); mark.lineWidth = 2; mark.stroke()
                } else {
                    NSColor.controlBackgroundColor.setFill(); path.fill()
                    NSColor.secondaryLabelColor.setStroke(); path.lineWidth = 1; path.stroke()
                }
            default:
                NSAttributedString(string: item.name, attributes: [.font: font, .foregroundColor: textColor])
                    .draw(at: item.rect.origin)
            }
        }
    }
}

// MARK: - Window

let window = NSWindow(
    contentRect: NSRect(x: 0, y: 0, width: 640, height: 440),
    styleMask: [.titled, .closable, .miniaturizable],
    backing: .buffered, defer: false)
window.title = "Argus Benchmark Form"
window.isReleasedWhenClosed = false
let content = NSView(frame: window.contentRect(forFrameRect: window.frame))
window.contentView = content

/// Places a view with a top-left `y` in the content view.
func place(_ view: NSView, _ x: CGFloat, _ y: CGFloat, _ width: CGFloat, _ height: CGFloat) {
    view.frame = NSRect(x: x, y: content.bounds.height - y - height, width: width, height: height)
    content.addSubview(view)
}

func label(_ text: String) -> NSTextField { NSTextField(labelWithString: text) }

let nameLabel = label("Name:")
place(nameLabel, 20, 22, 60, 17)
let nameField = NSTextField(string: "Ada Lovelace")
place(nameField, 90, 20, 200, 22)
let emailLabel = label("Email:")
place(emailLabel, 20, 54, 60, 17)
let emailField = NSTextField(string: "ada@example.com")
place(emailField, 90, 52, 200, 22)

let subscribe = NSButton(checkboxWithTitle: "Subscribe", target: nil, action: nil)
subscribe.state = .on
place(subscribe, 320, 20, 140, 18)
let remember = NSButton(checkboxWithTitle: "Remember me", target: nil, action: nil)
place(remember, 320, 44, 140, 18)
let locked = NSButton(checkboxWithTitle: "Locked option", target: nil, action: nil)
locked.isEnabled = false
place(locked, 320, 68, 140, 18)

final class RadioTarget: NSObject { @objc func pick(_ sender: Any?) {} }
let radioTarget = RadioTarget()
let sizes = ["Small", "Medium", "Large"].enumerated().map { index, title -> NSButton in
    let button = NSButton(radioButtonWithTitle: title, target: radioTarget, action: #selector(RadioTarget.pick(_:)))
    button.state = index == 1 ? .on : .off
    place(button, 480, 20 + CGFloat(index) * 24, 120, 18)
    return button
}

let periodLabel = label("Period:")
place(periodLabel, 20, 100, 60, 17)
let period = NSPopUpButton(frame: .zero, pullsDown: false)
period.addItems(withTitles: ["Monthly", "Yearly"])
place(period, 88, 96, 140, 25)
let view = NSSegmentedControl(labels: ["Day", "Week", "Month"], trackingMode: .selectOne, target: nil, action: nil)
view.selectedSegment = 1
place(view, 250, 96, 200, 24)

let volume = NSSlider(value: 30, minValue: 0, maxValue: 100, target: nil, action: nil)
place(volume, 480, 98, 140, 24)

// A table of synthetic files.
final class Files: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    var rows = [("notes.txt", "4 KB"), ("photo.png", "2 MB"), ("song.mp3", "5 MB")]
    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
    func tableView(_ tableView: NSTableView, viewFor column: NSTableColumn?, row: Int) -> NSView? {
        let text = column?.identifier.rawValue == "name" ? rows[row].0 : rows[row].1
        let cell = NSTextField(labelWithString: text)
        return cell
    }
}
let files = Files()
let table = NSTableView()
for (id, title, width) in [("name", "Name", 180.0), ("size", "Size", 90.0)] {
    let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(id))
    column.title = title
    column.width = width
    table.addTableColumn(column)
}
table.dataSource = files
table.delegate = files
table.rowHeight = 22
let scroll = NSScrollView()
scroll.documentView = table
scroll.hasVerticalScroller = true
scroll.borderType = .bezelBorder
place(scroll, 20, 140, 290, 150)

let canvas = Canvas()
place(canvas, 330, 140, 290, 150)
canvas.frame.size.width = 290

let delete = NSButton(title: "Delete", target: nil, action: nil)
delete.isEnabled = false
place(delete, 20, 390, 90, 32)
let cancel = NSButton(title: "Cancel", target: nil, action: nil)
place(cancel, 420, 390, 90, 32)
let save = NSButton(title: "Save", target: nil, action: nil)
save.keyEquivalent = "\r"
place(save, 520, 390, 100, 32)
let status = label("3 files, 7 MB")
place(status, 20, 310, 200, 17)

window.setFrameTopLeftPoint(NSPoint(x: 120, y: (NSScreen.screens.first?.frame.height ?? 900) - 120))
window.orderFrontRegardless()

// MARK: - Truth for the drawn controls

@MainActor func writeTruth() {
    canvas.display()
    let primary = NSScreen.screens.first!.frame.height
    // Canvas rect in global top-left points.
    let inWindow = canvas.convert(canvas.bounds, to: nil)
    let onScreen = window.convertToScreen(inWindow)
    let originX = onScreen.minX
    let originY = primary - onScreen.maxY
    let group: [String: Any] = [
        "id": "canvas",
        "role": "group",
        "bounds": [
            "x": Double(originX), "y": Double(originY),
            "width": Double(onScreen.width), "height": Double(onScreen.height),
        ],
        "state": ["visible": true],
        "significant": false,
        "note": "drawn without accessibility",
    ]
    let elements: [[String: Any]] = [group] + canvas.items.map { item in
        var state: [String: Any] = ["visible": true]
        if item.role != "text" { state["enabled"] = item.enabled }
        if let checked = item.checked { state["checked"] = checked ? "checked" : "unchecked" }
        return [
            "id": "canvas/\(item.role):\(item.name)",
            "role": item.role,
            "name": item.name,
            "bounds": [
                "x": Double(originX + item.rect.minX), "y": Double(originY + item.rect.minY),
                "width": Double(item.rect.width), "height": Double(item.rect.height),
            ],
            "state": state,
            "parent": "canvas",
            "significant": true,
            "note": "drawn without accessibility",
        ]
    }
    let data = try! JSONSerialization.data(withJSONObject: elements, options: [.prettyPrinted, .sortedKeys])
    try! data.write(to: URL(fileURLWithPath: truthPath))
}

// MARK: - Commands

DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
    writeTruth()
    print("ready \(ProcessInfo.processInfo.processIdentifier)")
    fflush(stdout)
    DispatchQueue.global().async {
        while let line = readLine() {
            let command = line.trimmingCharacters(in: .whitespaces)
            DispatchQueue.main.sync {
                switch command {
                case "check":
                    remember.state = remember.state == .on ? .off : .on
                case "row":
                    files.rows.append(("draft.md", "1 KB"))
                    table.reloadData()
                    status.stringValue = "4 files, 7 MB"
                case "canvas":
                    if let index = canvas.items.firstIndex(where: { $0.role == "checkbox" }) {
                        canvas.items[index].checked = !(canvas.items[index].checked ?? false)
                    }
                    if let index = canvas.items.firstIndex(where: { $0.name == "Erase" }) {
                        canvas.items[index].enabled = false
                    }
                    canvas.needsDisplay = true
                case "quit":
                    exit(0)
                default:
                    break
                }
                window.displayIfNeeded()
                writeTruth()
            }
            print("ok \(command)")
            fflush(stdout)
        }
        exit(0)
    }
}
app.run()
