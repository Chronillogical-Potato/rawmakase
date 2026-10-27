// Native regression for #616. Requires an unlocked desktop and Accessibility
// permission for the terminal running it. Moves the pointer and opens only the
// isolated probe; does not open RAWmakase's catalog or change global defaults.
//
// cargo build --locked --example titlebar_probe
// swift scripts/test-macos-titlebar.swift target/debug/examples/titlebar_probe
// RAWMAKASE_TITLEBAR_LEGACY=1 repeats the pre-fix behavior (expected to fail).
// Pass Minimize, None or Fill as a second argument to check other preferences.
// RAWMAKASE_TITLEBAR_MODAL=1 with Maximize or None checks native modal gestures.
// Fill is checked for enlargement only: unlike Zoom, it need not toggle restore.
import AppKit

var probeProcess: Process?
func fail(_ message: String) -> Never {
    if let process = probeProcess, process.isRunning {
        process.terminate()
        process.waitUntilExit()
    }
    fputs("FAIL: \(message)\n", stderr)
    exit(1)
}
guard CommandLine.arguments.count >= 2 else { fail("supply the probe executable") }
guard AXIsProcessTrusted() else { fail("Accessibility permission is required") }
let action = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "Maximize"
let process = Process()
probeProcess = process
process.executableURL = URL(fileURLWithPath: CommandLine.arguments[1])
// NSArgumentDomain affects this process alone, including AppKit's own action.
process.arguments = ["-AppleActionOnDoubleClick", action]
process.standardOutput = FileHandle.nullDevice
process.standardError = FileHandle.nullDevice
do { try process.run() } catch { fail("cannot launch probe: \(error)") }
defer { if process.isRunning { process.terminate(); process.waitUntilExit() } }
let app = AXUIElementCreateApplication(process.processIdentifier)
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value
}
var window: AXUIElement?
for _ in 0..<100 {
    if let windows = attribute(app, kAXWindowsAttribute) as? [AXUIElement], let first = windows.first {
        window = first
        break
    }
    Thread.sleep(forTimeInterval: 0.1)
}
guard let window else { process.terminate(); fail("probe did not open a window") }
func bounds() -> CGRect {
    guard let p = attribute(window, kAXPositionAttribute), let s = attribute(window, kAXSizeAttribute) else {
        fail("cannot read probe bounds")
    }
    var point = CGPoint.zero
    var size = CGSize.zero
    AXValueGetValue(p as! AXValue, .cgPoint, &point)
    AXValueGetValue(s as! AXValue, .cgSize, &size)
    return CGRect(origin: point, size: size)
}
func event(_ type: CGEventType, _ point: CGPoint, _ count: Int64) {
    let e = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)!
    e.setIntegerValueField(.mouseEventClickState, value: count)
    e.post(tap: .cghidEventTap)
}
func doubleClick(depth: Double, hold: Double = 0.08) {
    let b = bounds()
    let p = CGPoint(x: b.midX, y: b.minY + depth)
    for count: Int64 in [1, 2] {
        event(.leftMouseDown, p, count)
        Thread.sleep(forTimeInterval: count == 2 ? hold : 0.08)
        event(.leftMouseUp, p, count)
        Thread.sleep(forTimeInterval: 0.08)
    }
    Thread.sleep(forTimeInterval: 1.0)
}
Thread.sleep(forTimeInterval: 1)
let original = bounds()
var errors: [String] = []
var settable = DarwinBoolean(false)
let canMove = AXUIElementIsAttributeSettable(window, kAXPositionAttribute as CFString, &settable)
var position = CGPoint(x: original.minX + 50, y: original.minY + 20)
let moved = AXUIElementSetAttributeValue(window, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &position)!)
Thread.sleep(forTimeInterval: 0.2)
if canMove != .success || !settable.boolValue || moved != .success || bounds().origin != position {
    errors.append("Accessibility move failed: settable=\(settable), result=\(moved.rawValue), bounds=\(bounds())")
}
position = original.origin
_ = AXUIElementSetAttributeValue(window, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &position)!)
Thread.sleep(forTimeInterval: 0.2)

if action == "Maximize" {
    // Upper native strip and lower custom header; quick and held second clicks.
    let cases = ProcessInfo.processInfo.environment["RAWMAKASE_TITLEBAR_MODAL"] == nil
        ? [(10.0, 0.08), (40.0, 0.08), (10.0, 1.2), (10.0, 0.04)]
        : [(10.0, 0.08)] // native strip only; AppKit cancels a held double-click behind a modal
    for (depth, hold) in cases {
        doubleClick(depth: depth, hold: hold)
        let zoomed = bounds()
        if zoomed.width <= original.width && zoomed.height <= original.height {
            errors.append("\(action) did not stay enlarged at depth \(depth), hold \(hold)")
            break
        }
        doubleClick(depth: depth, hold: hold)
        if bounds() != original {
            errors.append("\(action) did not restore at depth \(depth), hold \(hold)")
            break
        }
    }
} else if action == "Fill" {
    doubleClick(depth: 10)
    let filled = bounds()
    if filled.width <= original.width && filled.height <= original.height { errors.append("did not fill") }
} else if action == "Minimize" {
    doubleClick(depth: 10)
    if attribute(window, kAXMinimizedAttribute) as? Bool != true { errors.append("did not minimize") }
} else {
    doubleClick(depth: 10)
    if bounds() != original { errors.append("\(action) changed the window") }
}
if action != "Minimize" && errors.isEmpty {
    let b = bounds()
    // Below AppKit's strip proves explicit fastframe dragging. A modal
    // blocks that handler, so its check uses the native strip instead.
    let depth = ProcessInfo.processInfo.environment["RAWMAKASE_TITLEBAR_MODAL"] == nil ? 40.0 : 20.0
    let p = CGPoint(x: b.midX, y: b.minY + depth)
    event(.leftMouseDown, p, 1)
    Thread.sleep(forTimeInterval: 0.15)
    for dx in [30.0, 60.0, 90.0] {
        event(.leftMouseDragged, CGPoint(x: p.x + dx, y: p.y), 1)
        Thread.sleep(forTimeInterval: 0.15)
    }
    event(.leftMouseUp, CGPoint(x: p.x + 90, y: p.y), 1)
    Thread.sleep(forTimeInterval: 0.5)
    if bounds().origin == b.origin { errors.append("explicit title-bar drag did not move") }
}
process.terminate()
process.waitUntilExit()
if !errors.isEmpty { fail(errors.joined(separator: "; ")) }
print("PASS: \(action), native title-bar gestures")
