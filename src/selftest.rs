//! `--self-test`: exercises the real windows and encoders without a user, for CI on all three
//! systems. `--demo-shots DIR`: renders the README screenshots from a made-up desktop, so no real
//! screen content ever ends up in the docs.

use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

use image::RgbaImage;

use snapsmith::editor::{Op, P, Shape, render};
use snapsmith::gifenc::GifWriter;

use crate::capture::{self, Shot};
use crate::overlay::{self, Mode, Script};
use crate::platform::DesktopRect;
use crate::tasks::Progress;
use crate::toast::{self, Toast};

pub fn run() -> Result<(), String> {
    let started = Instant::now();
    let mut report = Vec::new();

    // 1. Capture. macOS CI has no Screen Recording permission and gets a blank or failed capture;
    // that's reported, and a synthetic picture is used so the rest still runs.
    let shot = match capture::shoot_monitor_under_cursor(true) {
        Ok(s) if !capture::looks_blank(&s.image) => {
            report.push(format!("capture: {}x{} px, {} windows", s.image.width(), s.image.height(), s.windows.len()));
            s
        }
        Ok(s) => {
            report.push("capture: blank image (no permission?), using a synthetic desktop".into());
            synthetic_shot(Some(s.rect), s.scale)
        }
        Err(e) => {
            report.push(format!("capture failed ({e}), using a synthetic desktop"));
            synthetic_shot(None, 1.0)
        }
    };
    let (w, h) = shot.image.dimensions();
    let base = shot.image.clone();

    // 2. The overlay, driven by a script: select, annotate, confirm.
    let picked = overlay::run(shot, Mode::Capture, true, Some(Script { annotate: true, screenshot: None }))?
        .ok_or("overlay: closed without a result")?;
    let (rx, ry, rw, rh) = picked.region;
    if picked.image.dimensions() != (rw, rh) || rw < w / 3 || rh < h / 3 {
        return Err(format!(
            "overlay: unexpected result {}x{} for region {rw}x{rh}",
            picked.image.width(),
            picked.image.height()
        ));
    }
    let plain = image::imageops::crop_imm(&base, rx, ry, rw, rh).to_image();
    let changed = plain.pixels().zip(picked.image.pixels()).filter(|(a, b)| a != b).count();
    if changed < 500 {
        return Err(format!("overlay: annotations missing from the result ({changed} pixels changed)"));
    }
    report.push(format!("overlay: {rw}x{rh} selection, {changed} annotated pixels"));

    // 3. GIF encoding of a short synthetic animation.
    let mut gif = GifWriter::new(Vec::new(), 160, 90)?;
    for i in 0..8u32 {
        let mut f = RgbaImage::from_pixel(160, 90, image::Rgba([235, 236, 240, 255]));
        let op = Op {
            shape: Shape::Rect { a: P::new(10.0 + i as f32 * 15.0, 30.0), b: P::new(40.0 + i as f32 * 15.0, 60.0) },
            color: [220, 50, 50, 255],
            width: 4.0,
        };
        f = render::compose(&f, &[op]);
        gif.add(&f, 66)?;
    }
    let bytes = gif.finish()?;
    let decoded = image::load_from_memory(&bytes).map_err(|e| format!("gif: {e}"))?;
    report.push(format!("gif: {} bytes, {}x{}", bytes.len(), decoded.width(), decoded.height()));

    // 4. Region capture speed, used by recording.
    if let Some(m) = crate::platform::monitor_under_cursor() {
        let t = Instant::now();
        let mut ok = 0;
        for _ in 0..5 {
            if m.capture_region(0, 0, 320.min(m.width().unwrap_or(320)), 200.min(m.height().unwrap_or(200))).is_ok() {
                ok += 1;
            }
        }
        report.push(format!("region capture: {ok}/5 ok, {:.0} ms each", t.elapsed().as_secs_f64() * 1000.0 / 5.0));
    }

    // 5. The notification window, closing itself.
    let (tx, rx) = mpsc::channel();
    let _ = tx.send(Progress::Copied("Image"));
    let _ = tx.send(Progress::Done);
    toast::run(Toast { title: "Self-test".into(), thumbnail: Some(picked.image), anchor: None, seconds: 1.0 }, rx)?;
    report.push("notification: shown and closed".into());

    for line in &report {
        println!("{line}");
    }
    println!("self-test passed in {:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}

/// A made-up desktop: two windows with text, for tests and screenshots.
pub fn synthetic_desktop(w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let t = (x as f32 / w as f32) * 0.6 + (y as f32 / h as f32) * 0.4;
        let lerp = |a: f32, b: f32| (a + (b - a) * t) as u8;
        *p = image::Rgba([lerp(38.0, 92.0), lerp(54.0, 63.0), lerp(102.0, 140.0), 255]);
    }
    let s = (w as f32 / 1600.0).max(0.5);
    let mut ops = Vec::new();
    let fill = |img: &mut RgbaImage, x: f32, y: f32, ww: f32, hh: f32, c: [u8; 4]| {
        for yy in (y.max(0.0) as u32)..((y + hh) as u32).min(img.height()) {
            for xx in (x.max(0.0) as u32)..((x + ww) as u32).min(img.width()) {
                img.put_pixel(xx, yy, image::Rgba(c));
            }
        }
    };
    let mut window =
        |img: &mut RgbaImage, x: f32, y: f32, ww: f32, hh: f32, title: &str, dark: bool, lines: &[&str]| {
            let body = if dark { [30, 31, 36, 255] } else { [250, 250, 252, 255] };
            let bar = if dark { [48, 50, 58, 255] } else { [226, 228, 234, 255] };
            let text = if dark { [210, 215, 220, 255] } else { [40, 42, 48, 255] };
            fill(img, x, y - 34.0 * s, ww, 34.0 * s, bar);
            fill(img, x, y, ww, hh, body);
            ops.push(Op {
                shape: Shape::Text { at: P::new(x + 14.0 * s, y - 29.0 * s), text: title.into(), font_px: 17.0 * s },
                color: text,
                width: 1.0,
            });
            for (i, l) in lines.iter().enumerate() {
                ops.push(Op {
                    shape: Shape::Text {
                        at: P::new(x + 22.0 * s, y + 22.0 * s + i as f32 * 34.0 * s),
                        text: (*l).into(),
                        font_px: 21.0 * s,
                    },
                    color: text,
                    width: 1.0,
                });
            }
        };
    let (fw, fh) = (w as f32, h as f32);
    window(
        &mut img,
        fw * 0.08,
        fh * 0.12,
        fw * 0.55,
        fh * 0.6,
        "Release notes - Notes",
        false,
        &[
            "Snapsmith 0.1",
            "",
            "- Region, window and full-screen capture",
            "- Arrows, steps, text, highlighter, blur",
            "- Upload to Catbox or any ShareX .sxcu host",
            "- Copy text from any part of the screen",
            "- GIF and MP4 recording",
        ],
    );
    window(
        &mut img,
        fw * 0.58,
        fh * 0.45,
        fw * 0.36,
        fh * 0.42,
        "Terminal",
        true,
        &[
            "$ snapsmith capture region",
            "$ snapsmith upload notes.png",
            "https://files.catbox.moe/x1y2z3.png",
            "$ snapsmith record --gif",
        ],
    );
    render::compose(&img, &ops)
}

fn synthetic_shot(rect: Option<DesktopRect>, scale: f32) -> Shot {
    let rect = rect.unwrap_or(DesktopRect { x: 0, y: 0, w: 1280, h: 800 });
    let (w, h) = ((rect.w as f32 * scale) as u32, (rect.h as f32 * scale) as u32);
    Shot { image: synthetic_desktop(w.max(320), h.max(200)), rect, scale, windows: Vec::new() }
}

/// README screenshots: the overlay mid-annotation, and the main window.
pub fn demo_shots(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let monitor = crate::platform::monitor_under_cursor().ok_or("no monitor")?;
    let rect = capture::monitor_rect(&monitor)?;
    let scale = monitor.scale_factor().unwrap_or(1.0);
    let px_scale = if cfg!(target_os = "macos") { scale } else { 1.0 };
    let (w, h) = ((rect.w as f32 * px_scale) as u32, (rect.h as f32 * px_scale) as u32);
    let mut shot = Shot { image: synthetic_desktop(w, h), rect, scale: px_scale, windows: Vec::new() };
    shot.windows.push(crate::capture::WindowRect {
        x: w as f32 * 0.08,
        y: h as f32 * 0.12 - 34.0,
        w: w as f32 * 0.55,
        h: h as f32 * 0.6 + 34.0,
        title: "Release notes - Notes".into(),
    });
    overlay::run(
        shot,
        Mode::Capture,
        true,
        Some(Script { annotate: true, screenshot: Some(dir.join("overlay.png")) }),
    )?;
    crate::main_window::demo_shot(&dir.join("window.png"))?;
    crop_fraction(&dir.join("overlay.png"), (0.05, 0.04, 0.95, 0.84))?;
    for name in ["overlay.png", "window.png"] {
        shrink(&dir.join(name), 1600)?;
    }
    println!("wrote screenshots to {}", dir.display());
    Ok(())
}

fn crop_fraction(path: &Path, (x0, y0, x1, y1): (f32, f32, f32, f32)) -> Result<(), String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (img.width() as f32, img.height() as f32);
    let c = img.crop_imm((x0 * w) as u32, (y0 * h) as u32, ((x1 - x0) * w) as u32, ((y1 - y0) * h) as u32);
    c.save(path).map_err(|e| e.to_string())
}

/// Keeps README images a sensible size.
fn shrink(path: &Path, max_width: u32) -> Result<(), String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if img.width() > max_width {
        let h = (img.height() as f64 * max_width as f64 / img.width() as f64).round() as u32;
        img.resize_exact(max_width, h, image::imageops::FilterType::Lanczos3).save(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
