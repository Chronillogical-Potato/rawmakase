//! On macOS, Quit (Cmd-Q and the app menu's item) closes the window as its close
//! button does, so the app's close guard runs: it saves the edit, and refuses while
//! an export or Sync Settings runs (see docs/shutdown.md). winit's menu sends
//! `terminate:`, which closes the window without asking it first.

/// Points the app menu's Quit item at `performClose:` on the window of `app`, the
/// app being created; the event loop has built the menu by then. The window is
/// the item's target, not the responder chain, so Quit still works while the
/// window is minimized and not key. Elsewhere it does nothing.
#[cfg(target_os = "macos")]
pub fn through_close_guard(app: &impl winit::raw_window_handle::HasWindowHandle) {
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::{NSApplication, NSView};
    use winit::raw_window_handle::RawWindowHandle;
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let Ok(handle) = app.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: eframe supplies a live NSView, and this runs on its main thread.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    let Some(window) = view.window() else {
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
                // SAFETY: the window implements performClose:, outlives the menu
                // item (the app quits with it), and both are used on the main
                // thread.
                unsafe {
                    item.setTarget(Some(&window));
                    item.setAction(Some(sel!(performClose:)));
                }
            }
        }
    }
}

/// Elsewhere quitting already closes the window first.
#[cfg(not(target_os = "macos"))]
pub fn through_close_guard(_app: &impl winit::raw_window_handle::HasWindowHandle) {}
