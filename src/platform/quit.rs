//! On macOS, Quit (Cmd-Q and the app menu's item) closes the window as its close
//! button does, so the app's close guard runs: it saves the edit, and refuses while
//! an export or Sync Settings runs (see docs/shutdown.md). winit's menu sends
//! `terminate:`, which closes the window without asking it first.

/// Points the app menu's Quit item at the key window's `performClose:`. Call it once
/// the event loop has built the menu, on the main thread; elsewhere it does nothing.
#[cfg(target_os = "macos")]
pub fn through_close_guard() {
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::NSApplication;
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let Some(menu) = NSApplication::sharedApplication(main_thread).mainMenu() else {
        return;
    };
    for top in menu.itemArray() {
        let Some(submenu) = top.submenu() else {
            continue;
        };
        for item in submenu.itemArray() {
            if item.action() == Some(sel!(terminate:)) {
                // SAFETY: NSWindow implements performClose:, and the menu sends it
                // up the responder chain to the key window, on the main thread.
                unsafe { item.setAction(Some(sel!(performClose:))) };
            }
        }
    }
}

/// Elsewhere quitting already closes the window first.
#[cfg(not(target_os = "macos"))]
pub fn through_close_guard() {}
