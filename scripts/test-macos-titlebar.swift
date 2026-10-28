// Manual native regression for #616. Requires an unlocked desktop and
// Accessibility permission for the terminal. Never posts synthetic input:
// Force Touch pressure events must come from the real trackpad.
//
// cargo build --locked --example titlebar_probe
// swift scripts/test-macos-titlebar.swift target/debug/examples/titlebar_probe [Maximize|Minimize|None|Fill]
// RAWMAKASE_TITLEBAR_LEGACY=1 compares fastframe 0.13's currentEvent path.
// RAWMAKASE_TITLEBAR_MODAL=1 checks native-strip gestures behind an egui modal.
// Only the isolated probe is opened; no catalog, session or global defaults change.
import AppKit

var probeProcess: Process?
func fail(_ message: String) -> Never {
    if let process = probeProcess, process.isRunning { process.terminate() }
    fputs("FAIL: \(message)\n", stderr)
    exit(1)
}
guard CommandLine.arguments.count >= 2 else { fail("supply the probe executable") }
guard AXIsProcessTrusted() else { fail("Accessibility permission is required") }
let action = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "Maximize"
guard ["Maximize", "Minimize", "None", "Fill"].contains(action) else { fail("unknown action \(action)") }
let process = Process()
probeProcess = process
process.executableURL = URL(fileURLWithPath: CommandLine.arguments[1])
// NSArgumentDomain affects this process alone, including AppKit's own action.
process.arguments = ["-AppleActionOnDoubleClick", action]
process.standardOutput = FileHandle.nullDevice
do { try process.run() } catch { fail("cannot launch probe: \(error)") }
defer { if process.isRunning { process.terminate() } }
let app = AXUIElementCreateApplication(process.processIdentifier)
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value
}
var foundWindow: AXUIElement?
for _ in 0..<100 {
    if let windows = attribute(app, kAXWindowsAttribute) as? [AXUIElement], let first = windows.first {
        foundWindow = first
        break
    }
    Thread.sleep(forTimeInterval: 0.1)
}
guard let window = foundWindow else { fail("probe did not open a window") }
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
func step(_ instructions: String) {
    print("\nPID \(process.processIdentifier): \(instructions) Then return here and press Enter.")
    guard readLine() != nil else { fail("manual input ended; test incomplete") }
    guard process.isRunning else { fail("probe exited before the test finished") }
    Thread.sleep(forTimeInterval: 0.5)
    if attribute(window, "AXFullScreen") as? Bool == true { fail("window entered fullscreen; do not use the green button") }
    print("Observed bounds: \(bounds())")
}
step("Activate the probe by clicking its content. Do not use the green button")
let modal = ProcessInfo.processInfo.environment["RAWMAKASE_TITLEBAR_MODAL"] != nil
let depths = modal ? [10] : [10, 40]
for depth in depths {
    let original = bounds()
    if action == "Maximize" {
        for held in [false, true] {
            let gesture = held ? "double-click, holding the second press for one second before releasing" : "double-click normally"
            step("At the header's horizontal center, \(depth) px below the top edge, \(gesture)")
            let zoomed = bounds()
            if zoomed.width <= original.width && zoomed.height <= original.height { fail("Zoom did not stay enlarged at depth \(depth), held=\(held)") }
            step("At the same depth in the enlarged window, \(gesture) again")
            if bounds() != original { fail("Zoom did not restore at depth \(depth), held=\(held)") }
        }
    } else if action == "Minimize" {
        step("Double-click the header \(depth) px below its top edge")
        if attribute(window, kAXMinimizedAttribute) as? Bool != true { fail("did not minimize") }
        step("Restore the probe using its Dock icon")
        if attribute(window, kAXMinimizedAttribute) as? Bool == true { fail("probe is still minimized") }
    } else if action == "Fill" {
        step("Double-click the header \(depth) px below its top edge")
        let filled = bounds()
        if filled.width <= original.width && filled.height <= original.height { fail("did not fill at depth \(depth)") }
        // Fill is a separate native action; no toggle-to-restore assertion.
        break
    } else {
        step("Double-click the header \(depth) px below its top edge")
        if bounds() != original { fail("None changed the window") }
    }
}
if action != "Fill" {
    for depth in depths {
        let original = bounds()
        step("Hold the trackpad down on the header \(depth) px below the top edge and drag the window about 60 px sideways, away from screen edges")
        if bounds().origin == original.origin { fail("header drag did not move at depth \(depth)") }
    }
}
print("Did any gesture briefly resize and then reverse? Type yes or no:")
guard let reply = readLine(), reply.lowercased() == "no" else { fail("animation reversal reported, or observation incomplete") }
// The frame checks above establish the settled result. The operator must also
// report animation reversals; AX polling alone cannot establish their absence.
print("PASS: \(action), real trackpad gestures and observed window states")
