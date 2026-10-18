use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, MB_ICONERROR, MB_OK, MessageBoxW,
    SetWindowLongPtrW, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::DesktopRect;

pub fn init_process() {
    unsafe {
        // Physical pixels everywhere, so captures, the pointer and window positions agree on
        // every monitor whatever its scaling.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Release builds have no console; reattach to the launching terminal so CLI output shows.
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

pub fn show_error(message: &str) {
    let text = wide(message);
    let title = wide("Snapsmith");
    unsafe { MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR) };
}

pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

pub fn screen_capture_allowed() -> bool {
    true
}

pub fn request_screen_capture() -> bool {
    true
}

/// No open animation for the selection: it should appear at once, not fade in.
pub fn prepare_overlay(window: &winit::window::Window) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let hwnd = h.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let disabled: i32 = 1;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            (&disabled as *const i32).cast(),
            size_of::<i32>() as u32,
        );
    }
}

pub fn float_window(_window: &winit::window::Window) {}

fn hwnd_of(window: &winit::window::Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return None };
    Some(h.hwnd.get() as windows_sys::Win32::Foundation::HWND)
}

/// Moves a window to `rect` underneath every other window, without activating it.
pub fn place_behind(window: &winit::window::Window, rect: DesktopRect) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_BOTTOM, SWP_NOACTIVATE, SetWindowPos};
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        SetWindowPos(hwnd, HWND_BOTTOM, rect.x, rect.y, rect.w as i32, rect.h as i32, SWP_NOACTIVATE);
    }
}

/// Raises a window above everything (topmost) and gives it the keyboard.
pub fn bring_to_front(window: &winit::window::Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos,
    };
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        SetForegroundWindow(hwnd);
    }
}

/// Keeps a window out of the taskbar and Alt+Tab (the parked standby window).
pub fn tool_window(window: &winit::window::Window) {
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let hwnd = h.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let wanted = (style | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize);
        if wanted != style {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
        }
    }
}

/// The process that received a hotkey may hand foreground rights to the capture process it
/// starts, so the selection overlay gets the keyboard straight away.
pub fn allow_child_foreground() {
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

pub(super) fn native_work_area(x: i32, y: i32) -> Option<DesktopRect> {
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut info) == 0 {
            return None;
        }
        let RECT { left, top, right, bottom } = info.rcWork;
        Some(DesktopRect { x: left, y: top, w: (right - left).max(0) as u32, h: (bottom - top).max(0) as u32 })
    }
}

/// Windows 11 can give the Print Screen key to the Snipping Tool, and then no other app sees it.
pub fn snipping_tool_owns_print_screen() -> bool {
    let key = wide("Control Panel\\Keyboard");
    let value = wide("PrintScreenKeyForSnippingEnabled");
    let mut data: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    // On Windows 11 the setting defaults to on when the value is absent.
    if status != 0 { is_windows_11() } else { data != 0 }
}

fn is_windows_11() -> bool {
    // Build 22000 and later is Windows 11; the build number is in the registry as a string.
    let key = wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let value = wide("CurrentBuildNumber");
    let mut buf = [0u16; 32];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            windows_sys::Win32::System::Registry::RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return false;
    }
    let s = String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]);
    s.trim().parse::<u32>().is_ok_and(|b| b >= 22000)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
