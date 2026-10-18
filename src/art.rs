//! The app icon, drawn in code so every size is crisp: a viewfinder with a dot in the middle.

use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

/// Straight (non-premultiplied) RGBA, `size` x `size`.
pub fn app_icon(size: usize) -> Vec<u8> {
    draw(size, false, false)
}

/// The tray icon. On macOS a black template image the menu bar can tint; `recording` adds a red
/// dot on other systems and fills the centre on macOS.
pub fn tray_icon(size: usize, recording: bool) -> Vec<u8> {
    draw(size, cfg!(target_os = "macos"), recording)
}

fn draw(size: usize, template: bool, recording: bool) -> Vec<u8> {
    let s = size as f32;
    let mut pm = Pixmap::new(size as u32, size as u32).expect("icon size");
    let mut paint = Paint { anti_alias: true, ..Default::default() };

    if !template {
        let r = s * 0.22;
        let inset = s * 0.03;
        if let Some(path) = rounded_rect(inset, inset, s - 2.0 * inset, s - 2.0 * inset, r) {
            paint.shader = tiny_skia::LinearGradient::new(
                tiny_skia::Point::from_xy(0.0, 0.0),
                tiny_skia::Point::from_xy(s, s),
                vec![
                    tiny_skia::GradientStop::new(0.0, Color::from_rgba8(0x4A, 0x96, 0xFF, 0xFF)),
                    tiny_skia::GradientStop::new(1.0, Color::from_rgba8(0x1F, 0x5F, 0xD6, 0xFF)),
                ],
                tiny_skia::SpreadMode::Pad,
                Transform::identity(),
            )
            .unwrap_or_else(|| tiny_skia::Shader::SolidColor(Color::from_rgba8(0x3D, 0x8B, 0xFF, 0xFF)));
            pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    let fg = if template { Color::from_rgba8(0, 0, 0, 255) } else { Color::from_rgba8(255, 255, 255, 255) };
    paint.shader = tiny_skia::Shader::SolidColor(fg);
    let (a, b) = if template { (s * 0.12, s * 0.88) } else { (s * 0.24, s * 0.76) };
    let arm = (b - a) * 0.3;
    let width = if template { s * 0.11 } else { s * 0.075 };
    let stroke =
        Stroke { width: width.max(1.2), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
    let corners = [(a, a, 1.0, 1.0), (b, a, -1.0, 1.0), (a, b, 1.0, -1.0), (b, b, -1.0, -1.0)];
    for (x, y, dx, dy) in corners {
        let mut pb = PathBuilder::new();
        pb.move_to(x, y + dy * arm);
        pb.line_to(x, y);
        pb.line_to(x + dx * arm, y);
        if let Some(path) = pb.finish() {
            pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
    let dot = if template { s * 0.13 } else { s * 0.095 };
    if let Some(path) = PathBuilder::from_circle(s / 2.0, s / 2.0, dot) {
        pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    if recording && !template {
        paint.shader = tiny_skia::Shader::SolidColor(Color::from_rgba8(0xF2, 0x3D, 0x3D, 0xFF));
        if let Some(path) = PathBuilder::from_circle(s * 0.78, s * 0.22, s * 0.18) {
            pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    } else if recording && let Some(path) = PathBuilder::from_circle(s / 2.0, s / 2.0, s * 0.24) {
        pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }

    let mut data = pm.take();
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    data
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let _ = Rect::from_xywh(x, y, w, h)?;
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}
