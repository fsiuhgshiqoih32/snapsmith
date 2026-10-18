use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;

use super::DesktopRect;

pub fn init_process() {}

pub fn show_error(message: &str) {
    eprintln!("snapsmith: {message}");
}

/// The pointer from X11. Wayland doesn't tell clients where the pointer is.
pub fn cursor_pos() -> Option<(i32, i32)> {
    if super::is_wayland() || std::env::var_os("DISPLAY").is_none() {
        return None;
    }
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let reply = conn.query_pointer(root).ok()?.reply().ok()?;
    Some((reply.root_x as i32, reply.root_y as i32))
}

pub fn screen_capture_allowed() -> bool {
    true
}

pub fn request_screen_capture() -> bool {
    true
}

pub fn prepare_overlay(_window: &winit::window::Window) {}

pub fn float_window(_window: &winit::window::Window) {}

pub fn allow_child_foreground() {}

pub(super) fn native_work_area(_x: i32, _y: i32) -> Option<DesktopRect> {
    None
}
