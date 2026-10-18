use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSScreen, NSView, NSWindow, NSWindowCollectionBehavior};
use objc2_core_graphics::CGEvent;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::DesktopRect;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// kCGOverlayWindowLevel: above menus, the menu bar and the Dock.
const OVERLAY_LEVEL: isize = 102;

pub fn init_process() {}

pub fn show_error(message: &str) {
    eprintln!("snapsmith: {message}");
}

/// The pointer in points, origin at the top-left of the main display.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let event = CGEvent::new(None)?;
    let p = CGEvent::location(Some(&event));
    Some((p.x.round() as i32, p.y.round() as i32))
}

/// Without Screen Recording permission, captures show only the desktop wallpaper.
pub fn screen_capture_allowed() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Shows the system prompt (once per app) and returns whether access is granted now.
pub fn request_screen_capture() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

fn ns_window(window: &winit::window::Window) -> Option<Retained<NSWindow>> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return None };
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

pub fn prepare_overlay(window: &winit::window::Window) {
    if let Some(w) = ns_window(window) {
        w.setLevel(OVERLAY_LEVEL);
        w.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary,
        );
        w.makeKeyAndOrderFront(None);
    }
}

/// Keeps a floating window (pin, recording bar, notification) above normal windows and on every
/// Space.
pub fn float_window(window: &winit::window::Window) {
    if let Some(w) = ns_window(window) {
        w.setLevel(objc2_app_kit::NSFloatingWindowLevel);
        w.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
    }
}

pub fn allow_child_foreground() {}

/// The visible frame (without menu bar and Dock) of the screen containing (x, y), in top-left
/// points.
pub(super) fn native_work_area(x: i32, y: i32) -> Option<DesktopRect> {
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    // Cocoa's origin is the bottom-left of the main (first) screen.
    let main_h = screens.firstObject()?.frame().size.height;
    for screen in screens.iter() {
        let f = screen.frame();
        let top = main_h - (f.origin.y + f.size.height);
        let r = DesktopRect { x: f.origin.x as i32, y: top as i32, w: f.size.width as u32, h: f.size.height as u32 };
        if r.contains(x, y) {
            let v = screen.visibleFrame();
            let vtop = main_h - (v.origin.y + v.size.height);
            return Some(DesktopRect {
                x: v.origin.x as i32,
                y: vtop as i32,
                w: v.size.width as u32,
                h: v.size.height as u32,
            });
        }
    }
    None
}
