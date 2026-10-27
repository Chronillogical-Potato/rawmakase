//! Keep AppKit and fastframe from both acting on a title-bar double-click.

/// Fastframe performs Zoom and Minimize itself on the second press. AppKit
/// can also perform them on release, immediately undoing Zoom (#616).
/// Suppress the implicit action only while the second press is held; otherwise
/// keep native dragging (including over modal backdrops) and window management.
/// Fill is different: fastframe leaves that action entirely to AppKit.
#[cfg(target_os = "macos")]
pub(crate) fn configure(
    window: &impl winit::raw_window_handle::HasWindowHandle,
    ctx: &eframe::egui::Context,
) {
    use objc2_app_kit::NSView;
    use winit::raw_window_handle::RawWindowHandle;

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: eframe supplies a live NSView; this runs in its main-thread UI callback.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    let Some(window) = view.window() else {
        return;
    };
    if !ctx.input(|input| input.focused && input.pointer.primary_down()) && !window.isMovable() {
        // AppKit's implicit action is decided before delivering mouseUp.
        // Restore even if the release was lost on focus change.
        window.setMovable(true);
    }
}

/// Use fastframe's hit-test result, not the whole window's pointer state: an
/// egui modal, a disabled header or another widget can own the second press.
/// In those cases only AppKit acts, so there is nothing to suppress.
pub(crate) fn drag(ui: &mut eframe::egui::Ui, rect: eframe::egui::Rect) {
    fastframe_titlebar::drag(ui, rect);
    #[cfg(target_os = "macos")]
    if !ui.input(|input| input.viewport().fullscreen.unwrap_or(false))
        && ui.input(|input| input.pointer.primary_pressed())
        // fastframe 0.13's drag widget ID. Keep this adapter and the native
        // regression probe in step when updating that pinned dependency.
        && let Some(response) = ui.ctx().read_response(ui.id().with("fastframe-titlebar-drag"))
        && response.is_pointer_button_down_on()
    {
        suppress_native_action();
    }
}

#[cfg(target_os = "macos")]
fn suppress_native_action() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEventType};
    use objc2_foundation::{NSString, NSUserDefaults};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
        return;
    };
    if event.r#type() != NSEventType::LeftMouseDown || event.clickCount() != 2 {
        return;
    }
    // Read on the same press as fastframe, including changes made in Settings
    // while the window is open. Do not suppress Fill's sole native owner.
    let action = NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
        .map(|value| value.to_string());
    if !native_action(action.as_deref())
        && let Some(window) = event.window(mtm)
    {
        window.setMovable(false);
    }
}

/// This is ownership, not a second parser of the system preference: all
/// other values (including the legacy default) stay fastframe's decision.
#[cfg(any(target_os = "macos", test))]
fn native_action(action: Option<&str>) -> bool {
    action == Some("Fill")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fill_needs_appkits_implicit_titlebar_action() {
        assert!(native_action(Some("Fill")));
        for value in [
            None,
            Some("Maximize"),
            Some("Minimize"),
            Some("None"),
            Some("unknown"),
        ] {
            assert!(!native_action(value), "{value:?}");
        }
    }
}
