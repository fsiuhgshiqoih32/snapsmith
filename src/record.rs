//! Screen recording to GIF (built in) or MP4 (through ffmpeg).
//!
//! `snapsmith record` picks an area, then records it until Stop is pressed in the small control
//! bar, the recording shortcut is pressed again, `snapsmith record --stop` runs, or the time
//! limit is reached.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, vec2};
use image::RgbaImage;

use snapsmith::config::{self, Settings, Task};
use snapsmith::gifenc::GifWriter;
use snapsmith::naming::{self, NameContext};

use crate::commands;
use crate::log::{info, warning};
use crate::overlay::{self, Mode};
use crate::platform::{self, DesktopRect};
use crate::single_instance;
use crate::tasks::Progress;
use crate::toast::{self, Toast};
use crate::ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Gif,
    Mp4,
}

impl Format {
    fn ext(self) -> &'static str {
        match self {
            Format::Gif => "gif",
            Format::Mp4 => "mp4",
        }
    }
}

pub fn stop_file() -> PathBuf {
    config::config_dir().join("recording.stop")
}

/// Asks the running recording to finish.
pub fn request_stop() {
    let _ = std::fs::create_dir_all(config::config_dir());
    let _ = std::fs::write(stop_file(), b"stop");
}

pub fn command(mp4: bool, stop: bool) -> Result<(), String> {
    if stop || single_instance::is_held(single_instance::RECORDING) {
        request_stop();
        return Ok(());
    }
    let Some(_lock) = single_instance::acquire(single_instance::RECORDING)? else {
        request_stop();
        return Ok(());
    };
    let _ = std::fs::remove_file(stop_file());
    let settings = commands::load_settings();
    if platform::is_wayland() {
        return Err("Screen recording isn't available on Wayland yet: the desktop asks for permission on every \
                    frame. It works in an X11 session."
            .into());
    }
    if !platform::screen_capture_allowed() && !platform::request_screen_capture() {
        return Err("Snapsmith needs permission to record the screen (System Settings > Privacy & Security > \
                    Screen Recording)."
            .into());
    }
    let format = if mp4 { Format::Mp4 } else { Format::Gif };
    let ffmpeg = match format {
        Format::Mp4 => Some(find_ffmpeg(&settings)?),
        Format::Gif => None,
    };

    // Pick the area on a frozen frame, then record the live screen there.
    let monitor = platform::monitor_under_cursor().ok_or("no monitor found")?;
    let mon_rect = crate::capture::monitor_rect(&monitor)?;
    let (area, size) = match std::env::var("SNAPSMITH_TEST_AREA").ok().and_then(|s| parse_area(&s)) {
        Some(a) => (a, (a.w, a.h)),
        None => {
            let Some(_overlay) = single_instance::acquire(single_instance::OVERLAY)? else { return Ok(()) };
            let shot = crate::capture::shoot_monitor(&monitor, true)?;
            match overlay::run(shot, Mode::Area, settings.capture.magnifier, None)? {
                Some(p) => (p.desktop, p.image.dimensions()),
                None => return Ok(()),
            }
        }
    };
    // xcap wants the area relative to the monitor, in desktop units.
    let rel = (
        (area.x - mon_rect.x).max(0) as u32,
        (area.y - mon_rect.y).max(0) as u32,
        area.w.min(mon_rect.w),
        area.h.min(mon_rect.h),
    );
    if rel.2 < 8 || rel.3 < 8 {
        return Err("the area is too small to record".into());
    }

    let path = output_path(&settings, format, size.0, size.1);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    info!("recording {:?} to {}", area, path.display());

    let shared = Arc::new(Shared::default());
    let fps = match format {
        Format::Gif => settings.record.gif_fps,
        Format::Mp4 => settings.record.video_fps,
    };
    let worker = {
        let shared = shared.clone();
        let path = path.clone();
        let max = Duration::from_secs(settings.record.max_seconds as u64);
        let show_cursor = settings.record.show_cursor;
        let (cx, cy) = mon_rect.center();
        std::thread::spawn(move || {
            // Monitor handles can't cross threads on every system; look it up again here.
            let r = xcap::Monitor::from_point(cx, cy).map_err(|e| format!("capture failed: {e}")).and_then(|m| {
                record_loop(&m, rel, area, fps, max, show_cursor, format, ffmpeg.as_deref(), &path, &shared)
            });
            if let Err(e) = &r {
                *shared.error.lock().unwrap() = Some(e.clone());
            }
            shared.finished.store(true, Ordering::Relaxed);
            r
        })
    };

    control_bar(shared.clone(), area, format)?;
    shared.stop.store(true, Ordering::Relaxed);
    let result = worker.join().map_err(|_| "the recorder crashed".to_string())?;
    let _ = std::fs::remove_file(stop_file());
    if shared.cancelled.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let first_frame = result?;
    after_recording(&settings, &path, first_frame, area)
}

fn parse_area(s: &str) -> Option<DesktopRect> {
    let v: Vec<i64> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (v.len() == 4).then(|| DesktopRect { x: v[0] as i32, y: v[1] as i32, w: v[2] as u32, h: v[3] as u32 })
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    cancelled: AtomicBool,
    finished: AtomicBool,
    encoding: AtomicBool,
    frames: AtomicU64,
    started_ms: AtomicU64,
    error: Mutex<Option<String>>,
}

fn output_path(settings: &Settings, format: Format, w: u32, h: u32) -> PathBuf {
    let ctx = NameContext { width: w, height: h, title: String::new() };
    let pattern = settings.capture.file_pattern.replace("Snapsmith_", "Snapsmith_rec_");
    let expanded = naming::expand(&pattern, &ctx);
    naming::unique(naming::build_path(&settings.capture.save_dir(), &expanded, format.ext()))
}

pub fn find_ffmpeg(settings: &Settings) -> Result<PathBuf, String> {
    let configured = settings.record.ffmpeg.trim();
    let candidates: Vec<PathBuf> = if configured.is_empty() {
        let mut v = vec![PathBuf::from("ffmpeg")];
        if cfg!(target_os = "macos") {
            v.push("/opt/homebrew/bin/ffmpeg".into());
            v.push("/usr/local/bin/ffmpeg".into());
        }
        v
    } else {
        vec![PathBuf::from(configured)]
    };
    for c in candidates {
        let mut cmd = Command::new(&c);
        cmd.arg("-version").stdout(Stdio::null()).stderr(Stdio::null());
        no_window(&mut cmd);
        if cmd.status().is_ok_and(|s| s.success()) {
            return Ok(c);
        }
    }
    Err("MP4 recording needs ffmpeg. Install it (it's free), or set its location in Snapsmith's Recording \
         settings. GIF recording works without it."
        .into())
}

fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

/// Captures frames until asked to stop; returns the first frame for the notification thumbnail.
#[allow(clippy::too_many_arguments)]
fn record_loop(
    monitor: &xcap::Monitor,
    rel: (u32, u32, u32, u32),
    area: DesktopRect,
    fps: u32,
    max: Duration,
    show_cursor: bool,
    format: Format,
    ffmpeg: Option<&Path>,
    path: &Path,
    shared: &Shared,
) -> Result<Option<RgbaImage>, String> {
    let interval = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let grab = || monitor.capture_region(rel.0, rel.1, rel.2, rel.3).map_err(|e| format!("capture failed: {e}"));
    let probe = grab()?;
    // H.264 wants even dimensions.
    let (w, h) = match format {
        Format::Mp4 => (probe.width() & !1, probe.height() & !1),
        Format::Gif => (probe.width(), probe.height()),
    };
    let px_per_unit = probe.width() as f32 / rel.2.max(1) as f32;
    let cursor = cursor_sprite(px_per_unit);

    let mut sink: Box<dyn Sink> = match format {
        Format::Gif => {
            let file = std::fs::File::create(path).map_err(|e| format!("could not create {}: {e}", path.display()))?;
            Box::new(GifSink { writer: Some(GifWriter::new(BufWriter::new(file), w, h)?), last: None })
        }
        Format::Mp4 => Box::new(Mp4Sink::start(ffmpeg.unwrap_or(Path::new("ffmpeg")), w, h, fps, path)?),
    };

    let start = Instant::now();
    shared.started_ms.store(1, Ordering::Relaxed);
    let mut first = None;
    let mut written: u64 = 0;
    let mut next = start;
    loop {
        if shared.stop.load(Ordering::Relaxed) || stop_file().exists() || start.elapsed() >= max {
            shared.stop.store(true, Ordering::Relaxed);
            break;
        }
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
        }
        next += interval;
        let mut frame = match grab() {
            Ok(f) => f,
            Err(e) => {
                warning!("{e}");
                continue;
            }
        };
        if frame.dimensions() != (w, h) {
            if frame.width() < w || frame.height() < h {
                continue;
            }
            frame = image::imageops::crop_imm(&frame, 0, 0, w, h).to_image();
        }
        if show_cursor && let Some((cx, cy)) = platform::cursor_pos() {
            let x = ((cx - area.x) as f32 * px_per_unit) as i64;
            let y = ((cy - area.y) as f32 * px_per_unit) as i64;
            image::imageops::overlay(&mut frame, &cursor, x - 1, y - 1);
        }
        if first.is_none() {
            first = Some(frame.clone());
        }
        let elapsed = start.elapsed();
        // Video needs a constant rate: repeat the frame if capturing fell behind.
        let due = (elapsed.as_secs_f64() * fps as f64).floor() as u64 + 1;
        let copies = match format {
            Format::Mp4 => due.saturating_sub(written).max(1),
            Format::Gif => 1,
        };
        for _ in 0..copies {
            sink.push(&frame, elapsed)?;
        }
        written += copies;
        shared.frames.fetch_add(1, Ordering::Relaxed);
    }
    shared.encoding.store(true, Ordering::Relaxed);
    if shared.cancelled.load(Ordering::Relaxed) {
        sink.abort();
        return Ok(first);
    }
    sink.finish(start.elapsed())?;
    Ok(first)
}

trait Sink {
    fn push(&mut self, frame: &RgbaImage, at: Duration) -> Result<(), String>;
    fn finish(&mut self, at: Duration) -> Result<(), String>;
    fn abort(&mut self);
}

struct GifSink {
    writer: Option<GifWriter<BufWriter<std::fs::File>>>,
    /// The previous frame and when it was captured; written once its duration is known.
    last: Option<(RgbaImage, Duration)>,
}

impl Sink for GifSink {
    fn push(&mut self, frame: &RgbaImage, at: Duration) -> Result<(), String> {
        if let (Some((prev, t)), Some(w)) = (self.last.take(), self.writer.as_mut()) {
            w.add(&prev, (at - t).as_millis() as u32)?;
        }
        self.last = Some((frame.clone(), at));
        Ok(())
    }

    fn finish(&mut self, at: Duration) -> Result<(), String> {
        if let (Some((prev, t)), Some(w)) = (self.last.take(), self.writer.as_mut()) {
            w.add(&prev, ((at - t).as_millis() as u32).max(100))?;
        }
        if let Some(w) = self.writer.take() {
            let mut out = w.finish()?;
            out.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn abort(&mut self) {
        self.writer = None;
    }
}

struct Mp4Sink {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    w: u32,
    h: u32,
}

impl Mp4Sink {
    fn start(ffmpeg: &Path, w: u32, h: u32, fps: u32, path: &Path) -> Result<Self, String> {
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s"])
            .arg(format!("{w}x{h}"))
            .args(["-r", &fps.to_string(), "-i", "-", "-c:v", "libx264", "-preset", "veryfast", "-crf", "23"])
            .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        no_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("could not start ffmpeg: {e}"))?;
        let stdin = child.stdin.take();
        Ok(Mp4Sink { child, stdin, w, h })
    }
}

impl Sink for Mp4Sink {
    fn push(&mut self, frame: &RgbaImage, _at: Duration) -> Result<(), String> {
        let Some(stdin) = &mut self.stdin else { return Ok(()) };
        if frame.dimensions() != (self.w, self.h) {
            return Ok(());
        }
        stdin.write_all(frame.as_raw()).map_err(|e| format!("ffmpeg stopped accepting frames: {e}"))
    }

    fn finish(&mut self, _at: Duration) -> Result<(), String> {
        drop(self.stdin.take());
        let status = self.child.wait().map_err(|e| e.to_string())?;
        if status.success() { Ok(()) } else { Err(format!("ffmpeg failed ({status})")) }
    }

    fn abort(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A plain arrow pointer, drawn into recordings (screen captures don't include the real one).
fn cursor_sprite(scale: f32) -> RgbaImage {
    use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
    let s = scale.max(1.0);
    let size = (24.0 * s).ceil() as u32;
    let mut pm = Pixmap::new(size, size).expect("cursor size");
    let pts = [(1.0, 1.0), (1.0, 17.0), (5.0, 13.2), (8.2, 20.0), (11.0, 18.8), (7.9, 12.2), (13.0, 12.2)];
    let mut pb = PathBuilder::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        let (x, y) = (x * s, y * s);
        if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
    pb.close();
    if let Some(path) = pb.finish() {
        let mut p = Paint { anti_alias: true, ..Default::default() };
        p.set_color(Color::WHITE);
        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
        p.set_color(Color::BLACK);
        pm.stroke_path(&path, &p, &Stroke { width: 1.3 * s, ..Default::default() }, Transform::identity(), None);
    }
    let mut data = pm.take();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(size, size, data).expect("cursor buffer")
}

/// The floating Stop / Cancel bar, next to the recorded area.
fn control_bar(shared: Arc<Shared>, area: DesktopRect, format: Format) -> Result<(), String> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Snapsmith - Recording")
        .with_app_id("snapsmith")
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_inner_size(vec2(BAR_W, BAR_H));
    let options = crate::app::native_options(viewport);
    eframe::run_native(
        "Snapsmith - Recording",
        options,
        Box::new(move |cc| {
            ui::apply_theme(&cc.egui_ctx);
            Ok(Box::new(Bar { shared, area, format, started: Instant::now(), frame_no: 0 }))
        }),
    )
    .map_err(|e| format!("could not show the recording controls: {e}"))
}

const BAR_W: f32 = 330.0;
const BAR_H: f32 = 46.0;

struct Bar {
    shared: Arc<Shared>,
    area: DesktopRect,
    format: Format,
    started: Instant,
    frame_no: u64,
}

impl Bar {
    fn place(&self, frame: &eframe::Frame) {
        let Some(w) = frame.winit_window() else { return };
        let (cx, cy) = self.area.center();
        let s = if cfg!(target_os = "macos") {
            1.0
        } else {
            xcap::Monitor::from_point(cx, cy).ok().and_then(|m| m.scale_factor().ok()).unwrap_or(1.0)
        };
        let (bw, bh) = ((BAR_W * s) as u32, (BAR_H * s) as u32);
        let screen = platform::work_area_at(cx, cy).unwrap_or(self.area);
        let gap = (10.0 * s) as i32;
        let x = (cx - bw as i32 / 2).clamp(screen.x, screen.x + screen.w as i32 - bw as i32);
        // Below the area if it fits, else above, else just inside the bottom edge.
        let below = self.area.y + self.area.h as i32 + gap;
        let above = self.area.y - gap - bh as i32;
        let y = if below + bh as i32 <= screen.y + screen.h as i32 {
            below
        } else if above >= screen.y {
            above
        } else {
            self.area.y + self.area.h as i32 - bh as i32 - gap
        };
        platform::place(w, DesktopRect { x, y, w: bw, h: bh });
        platform::float_window(w);
    }
}

impl eframe::App for Bar {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.frame_no == 0 || self.frame_no == 3 {
            self.place(frame);
        }
        self.frame_no += 1;
        crate::app::shot_hook(&ctx, self.frame_no, "record-bar");
        if self.shared.finished.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let screen = ctx.viewport_rect();
        ui.painter().rect_filled(screen, 8.0, ui::PANEL);
        ui.painter().rect_stroke(
            screen.shrink(0.5),
            8.0,
            egui::Stroke::new(1.0, Color32::from_white_alpha(30)),
            egui::StrokeKind::Inside,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(screen.shrink2(vec2(12.0, 6.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let encoding = self.shared.encoding.load(Ordering::Relaxed) || self.shared.stop.load(Ordering::Relaxed);
        if encoding {
            child.add(egui::Spinner::new().size(14.0));
            let what = match self.format {
                Format::Gif => "Saving GIF...",
                Format::Mp4 => "Saving video...",
            };
            child.label(RichText::new(what).color(ui::TEXT));
        } else {
            let t = self.started.elapsed().as_secs_f32();
            let blink = (t * 2.0) as i32 % 2 == 0;
            let (rect, _) = child.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
            child.painter().circle_filled(
                rect.center(),
                6.0,
                if blink { Color32::from_rgb(0xF2, 0x3D, 0x3D) } else { Color32::from_rgb(0x8A, 0x26, 0x26) },
            );
            let secs = t as u64;
            child.label(RichText::new(format!("{}:{:02}", secs / 60, secs % 60)).monospace().size(15.0));
            child.label(RichText::new(self.format.ext().to_uppercase()).color(ui::MUTED).size(12.0));
            child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    self.shared.cancelled.store(true, Ordering::Relaxed);
                    self.shared.stop.store(true, Ordering::Relaxed);
                }
                if ui.button(RichText::new("Stop").strong()).clicked() {
                    self.shared.stop.store(true, Ordering::Relaxed);
                }
            });
        }
        if let Some(e) = self.shared.error.lock().unwrap().clone() {
            child.label(RichText::new(e).color(ui::DANGER).size(11.0));
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = ui::PANEL.to_normalized_gamma_f32();
        [c[0], c[1], c[2], 1.0]
    }
}

/// History entry, optional upload, and the notification.
fn after_recording(
    settings: &Settings,
    path: &Path,
    first: Option<RgbaImage>,
    area: DesktopRect,
) -> Result<(), String> {
    let anchor = Some(area.center());
    let upload = settings.capture.after_capture.contains(&Task::Upload);
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(Progress::Saved(path.to_path_buf()));
    let worker = {
        let s = settings.clone();
        let p = path.to_path_buf();
        std::thread::spawn(move || {
            if upload {
                crate::tasks::upload_file(&s, &p, &tx);
            } else {
                let mut entry = snapsmith::history::Entry::now("file");
                entry.path = Some(p.clone());
                let _ = snapsmith::history::append(&snapsmith::history::default_path(), &entry);
                let _ = tx.send(Progress::Done);
            }
        })
    };
    let toast =
        Toast { title: "Recording".into(), thumbnail: first, anchor, seconds: settings.general.toast_seconds.max(5.0) };
    if let Err(e) = toast::run(toast, rx) {
        warning!("{e}");
    }
    let _ = worker.join();
    Ok(())
}
