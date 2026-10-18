//! Grabbing pixels with xcap: one monitor (for the selection overlay), all monitors stitched
//! together, or the focused window.

use image::RgbaImage;

use crate::log::warning;
use crate::platform::{self, DesktopRect};

/// A window's outline in the pixel coordinates of a [`Shot`], for snapping the selection to it.
#[derive(Clone, Debug)]
pub struct WindowRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub title: String,
}

impl WindowRect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// A frozen picture of one monitor.
pub struct Shot {
    pub image: RgbaImage,
    /// Where the monitor is, in desktop units.
    pub rect: DesktopRect,
    /// Image pixels per desktop unit (2 on a Retina display, 1 elsewhere).
    pub scale: f32,
    /// Front to back.
    pub windows: Vec<WindowRect>,
}

fn err(e: impl std::fmt::Display) -> String {
    format!("screen capture failed: {e}")
}

pub fn monitor_rect(m: &xcap::Monitor) -> Result<DesktopRect, String> {
    Ok(DesktopRect {
        x: m.x().map_err(err)?,
        y: m.y().map_err(err)?,
        w: m.width().map_err(err)?,
        h: m.height().map_err(err)?,
    })
}

/// The monitor under the pointer, with the window outlines on it.
pub fn shoot_monitor_under_cursor(with_windows: bool) -> Result<Shot, String> {
    let monitor = platform::monitor_under_cursor().ok_or("no monitor found")?;
    shoot_monitor(&monitor, with_windows)
}

pub fn shoot_monitor(monitor: &xcap::Monitor, with_windows: bool) -> Result<Shot, String> {
    let rect = monitor_rect(monitor)?;
    let image = monitor.capture_image().map_err(err)?;
    let scale = if rect.w > 0 { image.width() as f32 / rect.w as f32 } else { 1.0 };
    let windows = if with_windows { window_rects(rect, scale) } else { Vec::new() };
    Ok(Shot { image, rect, scale, windows })
}

/// Visible windows overlapping `monitor`, front to back, in that monitor's image pixels.
fn window_rects(monitor: DesktopRect, scale: f32) -> Vec<WindowRect> {
    if platform::is_wayland() {
        return Vec::new();
    }
    let windows = match xcap::Window::all() {
        Ok(w) => w,
        Err(e) => {
            warning!("could not list windows: {e}");
            return Vec::new();
        }
    };
    let own_pid = std::process::id();
    let mut out = Vec::new();
    for w in windows {
        if w.is_minimized().unwrap_or(true) || w.pid().is_ok_and(|p| p == own_pid) {
            continue;
        }
        let (Ok(x), Ok(y), Ok(width), Ok(height)) = (w.x(), w.y(), w.width(), w.height()) else { continue };
        if width < 8 || height < 8 {
            continue;
        }
        // Clip to the monitor.
        let x0 = x.max(monitor.x);
        let y0 = y.max(monitor.y);
        let x1 = (x + width as i32).min(monitor.x + monitor.w as i32);
        let y1 = (y + height as i32).min(monitor.y + monitor.h as i32);
        if x1 - x0 < 8 || y1 - y0 < 8 {
            continue;
        }
        out.push(WindowRect {
            x: (x0 - monitor.x) as f32 * scale,
            y: (y0 - monitor.y) as f32 * scale,
            w: (x1 - x0) as f32 * scale,
            h: (y1 - y0) as f32 * scale,
            title: w.title().unwrap_or_default(),
        });
    }
    out
}

/// Every monitor in one image, laid out as on the desktop. Monitors with a lower pixel density
/// than the sharpest one are scaled up so the layout stays true.
pub fn shoot_all() -> Result<RgbaImage, String> {
    let monitors = xcap::Monitor::all().map_err(err)?;
    if monitors.is_empty() {
        return Err("no monitor found".into());
    }
    let mut shots = Vec::new();
    for m in &monitors {
        match shoot_monitor(m, false) {
            Ok(s) => shots.push(s),
            Err(e) => warning!("{e}"),
        }
    }
    if shots.is_empty() {
        return Err("screen capture failed on every monitor".into());
    }
    if shots.len() == 1 {
        return Ok(shots.pop().unwrap().image);
    }
    let scale = shots.iter().map(|s| s.scale).fold(1.0f32, f32::max);
    let min_x = shots.iter().map(|s| s.rect.x).min().unwrap();
    let min_y = shots.iter().map(|s| s.rect.y).min().unwrap();
    let max_x = shots.iter().map(|s| s.rect.x + s.rect.w as i32).max().unwrap();
    let max_y = shots.iter().map(|s| s.rect.y + s.rect.h as i32).max().unwrap();
    let w = ((max_x - min_x) as f32 * scale).round() as u32;
    let h = ((max_y - min_y) as f32 * scale).round() as u32;
    let mut canvas = RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255]));
    for s in shots {
        let tw = (s.rect.w as f32 * scale).round() as u32;
        let th = (s.rect.h as f32 * scale).round() as u32;
        let img = if s.image.dimensions() == (tw, th) {
            s.image
        } else {
            image::imageops::resize(&s.image, tw, th, image::imageops::FilterType::Triangle)
        };
        let ox = ((s.rect.x - min_x) as f32 * scale).round() as i64;
        let oy = ((s.rect.y - min_y) as f32 * scale).round() as i64;
        image::imageops::overlay(&mut canvas, &img, ox, oy);
    }
    Ok(canvas)
}

/// The focused window as it appears on screen (taken from its monitor, so it looks exactly as
/// seen, including anything on top of it). Falls back to the monitor under the pointer.
pub fn shoot_active_window() -> Result<(RgbaImage, String), String> {
    if !platform::is_wayland()
        && let Ok(windows) = xcap::Window::all()
    {
        let own_pid = std::process::id();
        let focused = windows.into_iter().find(|w| {
            w.is_focused().unwrap_or(false)
                && !w.is_minimized().unwrap_or(true)
                && w.pid().map_or(true, |p| p != own_pid)
        });
        if let Some(w) = focused
            && let (Ok(x), Ok(y), Ok(width), Ok(height)) = (w.x(), w.y(), w.width(), w.height())
        {
            let title = w.title().unwrap_or_default();
            let (cx, cy) = (x + width as i32 / 2, y + height as i32 / 2);
            if let Ok(m) = xcap::Monitor::from_point(cx, cy) {
                let shot = shoot_monitor(&m, false)?;
                let x0 = ((x - shot.rect.x) as f32 * shot.scale).round().max(0.0) as u32;
                let y0 = ((y - shot.rect.y) as f32 * shot.scale).round().max(0.0) as u32;
                let x1 =
                    (((x + width as i32 - shot.rect.x) as f32 * shot.scale).round() as u32).min(shot.image.width());
                let y1 =
                    (((y + height as i32 - shot.rect.y) as f32 * shot.scale).round() as u32).min(shot.image.height());
                if x1 > x0 + 4 && y1 > y0 + 4 {
                    let img = image::imageops::crop_imm(&shot.image, x0, y0, x1 - x0, y1 - y0).to_image();
                    return Ok((img, title));
                }
            }
        }
    }
    let shot = shoot_monitor_under_cursor(false)?;
    Ok((shot.image, String::new()))
}

/// True if an image is a single flat colour, which is what macOS returns without permission and
/// what some broken capture paths produce.
pub fn looks_blank(img: &RgbaImage) -> bool {
    let Some(first) = img.pixels().next() else { return true };
    let step = (img.width() * img.height() / 4096).max(1) as usize;
    img.pixels().step_by(step).all(|p| p == first)
}
