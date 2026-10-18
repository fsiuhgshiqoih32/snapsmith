//! The notification after a capture: a thumbnail and what happened (saved, copied, uploaded),
//! updated live while an upload runs. Click it to open the link or the file.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui::{self, Color32, RichText, Sense, Stroke, TextureOptions, vec2};
use image::RgbaImage;

use crate::platform::{self, DesktopRect};
use crate::tasks::Progress;
use crate::ui;
use crate::ui::icons::{self, Icon};

const W: f32 = 390.0;
const H: f32 = 118.0;

pub struct Toast {
    pub title: String,
    pub thumbnail: Option<RgbaImage>,
    /// A desktop point on the monitor to show it on (the capture's location).
    pub anchor: Option<(i32, i32)>,
    pub seconds: f32,
}

/// Shows the notification until it times out or is closed. Returns once the window is gone.
pub fn run(toast: Toast, rx: Receiver<Progress>) -> Result<(), String> {
    if platform::is_wayland() {
        return run_system_notification(toast, rx);
    }
    let viewport = egui::ViewportBuilder::default()
        .with_title("Snapsmith")
        .with_app_id("snapsmith")
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_active(false)
        .with_inner_size(vec2(W, H));
    let mut parts = Some((toast, rx));
    crate::app::run_window("Snapsmith", viewport, crate::app::Kind::Accessory, |_cc| {
        let (toast, rx) = parts.take().expect("notification built once");
        ToastApp::new(toast, rx)
    })
    .map_err(|e| format!("could not show the notification: {e}"))
}

struct ToastApp {
    toast: Toast,
    rx: Receiver<Progress>,
    lines: Vec<(String, Color32)>,
    busy: Option<String>,
    saved: Option<PathBuf>,
    url: Option<String>,
    failed: bool,
    done_at: Option<f64>,
    texture: Option<egui::TextureHandle>,
    frame_no: u64,
    target: Option<DesktopRect>,
}

impl ToastApp {
    fn new(toast: Toast, rx: Receiver<Progress>) -> Self {
        ToastApp {
            toast,
            rx,
            lines: Vec::new(),
            busy: None,
            saved: None,
            url: None,
            failed: false,
            done_at: None,
            texture: None,
            frame_no: 0,
            target: None,
        }
    }

    fn poll(&mut self, now: f64) {
        while let Ok(p) = self.rx.try_recv() {
            match p {
                Progress::Saved(path) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.lines.push((format!("Saved  {name}"), ui::TEXT));
                    self.saved = Some(path);
                }
                Progress::Copied(what) => {
                    let text = if what == "Link" { "Link copied".to_string() } else { format!("{what} copied") };
                    self.lines.push((text, ui::TEXT));
                }
                Progress::Uploading(dest) => self.busy = Some(format!("Uploading to {dest}...")),
                Progress::Uploaded { url, .. } => {
                    self.busy = None;
                    self.lines.push((url.clone(), ui::ACCENT));
                    self.url = Some(url);
                }
                Progress::Failed(e) => {
                    self.busy = None;
                    self.failed = true;
                    self.lines.push((e, ui::DANGER));
                }
                Progress::Done => {
                    self.busy = None;
                    self.done_at = Some(now);
                }
            }
        }
    }

    fn place(&mut self, frame: &eframe::Frame) {
        let Some(window) = frame.winit_window() else { return };
        let target = *self.target.get_or_insert_with(|| {
            let (x, y) = self.toast.anchor.or_else(platform::cursor_pos).unwrap_or((0, 0));
            let area = platform::work_area_at(x, y).unwrap_or(DesktopRect { x: 0, y: 0, w: 1280, h: 800 });
            let s = if cfg!(target_os = "macos") {
                1.0
            } else {
                xcap::Monitor::from_point(x, y).ok().and_then(|m| m.scale_factor().ok()).unwrap_or(1.0)
            };
            let (w, h) = ((W * s).round() as u32, (H * s).round() as u32);
            let margin = (16.0 * s) as i32;
            DesktopRect {
                x: area.x + area.w as i32 - w as i32 - margin,
                y: area.y + area.h as i32 - h as i32 - margin,
                w,
                h,
            }
        });
        platform::place(window, target);
        platform::float_window(window);
    }

    fn open_primary(&self) {
        if let Some(url) = &self.url {
            crate::app::open(url);
        } else if let Some(p) = &self.saved {
            crate::app::open(&p.to_string_lossy());
        }
    }
}

impl eframe::App for ToastApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if self.frame_no == 0 || self.frame_no == 3 {
            self.place(frame);
        }
        self.frame_no += 1;
        crate::app::shot_hook(&ctx, self.frame_no, "toast");
        self.poll(now);

        let screen = ctx.viewport_rect();
        let hovered = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| screen.contains(p));
        if let Some(done) = self.done_at {
            let limit = if self.failed { 20.0 } else { self.toast.seconds as f64 };
            if hovered {
                self.done_at = Some(now);
            } else if now - done > limit {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        let painter = ui.painter();
        painter.rect_filled(screen, 8.0, ui::PANEL);
        painter.rect_stroke(
            screen.shrink(0.5),
            8.0,
            Stroke::new(1.0, Color32::from_white_alpha(30)),
            egui::StrokeKind::Inside,
        );

        let body = ui.interact(screen, egui::Id::unique("toast-body"), Sense::click());
        if body.clicked() {
            self.open_primary();
        }

        let inner = screen.shrink(12.0);
        let thumb_rect = egui::Rect::from_min_size(inner.min, vec2(118.0, inner.height()));
        if let Some(img) = &self.toast.thumbnail {
            if self.texture.is_none() {
                let small = image::imageops::thumbnail(
                    img,
                    236.min(img.width()).max(1),
                    ((236.0 * img.height() as f32 / img.width().max(1) as f32) as u32).clamp(1, 188),
                );
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [small.width() as usize, small.height() as usize],
                    small.as_raw(),
                );
                self.texture = Some(ctx.load_texture("toast-thumb", ci, TextureOptions::LINEAR));
            }
            if let Some(t) = &self.texture {
                let size = t.size_vec2();
                let scale = (thumb_rect.width() / size.x).min(thumb_rect.height() / size.y);
                let r = egui::Rect::from_center_size(thumb_rect.center(), size * scale);
                ui.painter().rect_filled(thumb_rect, 4.0, Color32::from_black_alpha(80));
                ui.painter().image(
                    t.id(),
                    r,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }

        let text_rect = egui::Rect::from_min_max(egui::pos2(thumb_rect.max.x + 12.0, inner.min.y), inner.max);
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::top_down(egui::Align::Min)));
        child.horizontal(|ui| {
            ui.label(RichText::new(&self.toast.title).strong().size(14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if icons::button(ui, Icon::Cancel, false, true, "Close").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if let Some(p) = &self.saved
                    && icons::button(ui, Icon::Folder, false, true, "Show in folder").clicked()
                {
                    crate::app::reveal(p);
                }
                if let Some(url) = &self.url
                    && icons::button(ui, Icon::Link, false, true, "Copy link").clicked()
                {
                    crate::clipboard::copy_text(url);
                }
            });
        });
        child.spacing_mut().item_spacing.y = 2.0;
        let shown = self.lines.len().min(4);
        for (text, color) in &self.lines[self.lines.len() - shown..] {
            child.add(egui::Label::new(RichText::new(text).color(*color).size(12.5)).truncate());
        }
        if let Some(b) = &self.busy {
            child.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(RichText::new(b).size(12.5).color(ui::MUTED));
            });
        }
        if self.done_at.is_none() || hovered {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = ui::PANEL.to_normalized_gamma_f32();
        [c[0], c[1], c[2], 1.0]
    }
}

/// Wayland can't place our own window in a corner, so use the desktop's notifications there.
#[cfg(target_os = "linux")]
fn run_system_notification(toast: Toast, rx: Receiver<Progress>) -> Result<(), String> {
    let mut body = Vec::new();
    let mut saved = None;
    for p in rx {
        match p {
            Progress::Saved(path) => {
                body.push(format!(
                    "Saved {}",
                    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                ));
                saved = Some(path);
            }
            Progress::Copied(what) => body.push(format!("{what} copied")),
            Progress::Uploaded { url, .. } => body.push(url),
            Progress::Failed(e) => body.push(e),
            Progress::Uploading(_) => {}
            Progress::Done => break,
        }
    }
    let mut n = notify_rust::Notification::new();
    n.summary(&toast.title).body(&body.join("\n")).appname("Snapsmith").timeout((toast.seconds * 1000.0) as i32);
    if let Some(p) = &saved {
        n.image_path(&p.to_string_lossy());
    }
    n.show().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
fn run_system_notification(_toast: Toast, rx: Receiver<Progress>) -> Result<(), String> {
    for p in rx {
        if p == Progress::Done {
            break;
        }
    }
    Ok(())
}
