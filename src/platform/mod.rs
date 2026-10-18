//! Operating-system specifics.
//!
//! Positions on the desktop are in "desktop units": physical pixels on Windows and X11, points on
//! macOS. That's what xcap reports for monitors and windows, and what each system's window APIs
//! take, so a rectangle can go from a capture to a window position without conversion.
//!
//! Every backend provides:
//! - `init_process()`: DPI awareness and console attachment, called first thing in `main`.
//! - `show_error(msg)`: a fatal error the user will see even without a terminal.
//! - `cursor_pos()`: the pointer in desktop units, if the system tells us.
//! - `screen_capture_allowed()` / `request_screen_capture()`: macOS's Screen Recording permission.
//! - `prepare_overlay(window)`: last touches on a full-screen selection window.
//! - `allow_child_foreground()`: lets a process we start take keyboard focus (Windows).

use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use winit::window::{Fullscreen, Window};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use self::linux::*;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
compile_error!("Snapsmith supports Windows, macOS and Linux.");

/// A rectangle in desktop units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DesktopRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl DesktopRect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w as i32 && y < self.y + self.h as i32
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w as i32 / 2, self.y + self.h as i32 / 2)
    }
}

/// True on a Wayland session, where apps can't place their windows or read the pointer.
pub fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland")))
}

/// The winit monitor showing the centre of `rect` (desktop units).
fn monitor_for(window: &Window, rect: DesktopRect) -> Option<winit::monitor::MonitorHandle> {
    let (cx, cy) = rect.center();
    window.available_monitors().find(|m| {
        let pos = m.position();
        let size = m.size();
        let r = if cfg!(target_os = "macos") {
            let s = m.scale_factor();
            let p: LogicalPosition<f64> = pos.to_logical(s);
            let z: LogicalSize<f64> = size.to_logical(s);
            DesktopRect {
                x: p.x.round() as i32,
                y: p.y.round() as i32,
                w: z.width.round() as u32,
                h: z.height.round() as u32,
            }
        } else {
            DesktopRect { x: pos.x, y: pos.y, w: size.width, h: size.height }
        };
        r.contains(cx, cy)
    })
}

/// Covers the monitor at `rect` with `window`, above everything else.
pub fn cover_monitor(window: &Window, rect: DesktopRect) {
    if cfg!(target_os = "macos") {
        // Borderless "full screen" on macOS animates into its own Space; a plain window at the
        // screen's frame, raised above the menu bar, appears instantly.
        window.set_outer_position(LogicalPosition::new(rect.x, rect.y));
        let _ = window.request_inner_size(LogicalSize::new(rect.w, rect.h));
        prepare_overlay(window);
    } else {
        let monitor = monitor_for(window, rect).or_else(|| window.current_monitor());
        window.set_fullscreen(Some(Fullscreen::Borderless(monitor)));
        prepare_overlay(window);
    }
}

/// Moves and sizes a normal window (toast, pin, recording bar). Ignored on Wayland, which
/// doesn't let clients position themselves.
pub fn place(window: &Window, rect: DesktopRect) {
    if is_wayland() {
        return;
    }
    if cfg!(target_os = "macos") {
        window.set_outer_position(LogicalPosition::new(rect.x, rect.y));
        let _ = window.request_inner_size(LogicalSize::new(rect.w, rect.h));
    } else {
        window.set_outer_position(PhysicalPosition::new(rect.x, rect.y));
        let _ = window.request_inner_size(PhysicalSize::new(rect.w, rect.h));
    }
}

/// The usable area of the monitor at (x, y): without the taskbar, Dock or menu bar where known.
pub fn work_area_at(x: i32, y: i32) -> Option<DesktopRect> {
    native_work_area(x, y).or_else(|| {
        let m = xcap::Monitor::from_point(x, y).ok()?;
        Some(DesktopRect { x: m.x().ok()?, y: m.y().ok()?, w: m.width().ok()?, h: m.height().ok()? })
    })
}

/// The monitor under the pointer, or the primary one.
pub fn monitor_under_cursor() -> Option<xcap::Monitor> {
    if let Some((x, y)) = cursor_pos()
        && let Ok(m) = xcap::Monitor::from_point(x, y)
    {
        return Some(m);
    }
    let all = xcap::Monitor::all().ok()?;
    let primary = all.iter().position(|m| m.is_primary().unwrap_or(false)).unwrap_or(0);
    all.into_iter().nth(primary)
}

/// True when the system keeps the Print Screen key for itself (Windows 11's Snipping Tool).
pub fn print_screen_taken() -> bool {
    #[cfg(windows)]
    {
        snipping_tool_owns_print_screen()
    }
    #[cfg(not(windows))]
    {
        false
    }
}
