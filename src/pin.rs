//! Pin to screen: an image in a borderless window that stays on top. Drag to move, scroll to
//! zoom, double-click or Esc to close, right-click for more.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Sense, Stroke, TextureOptions, vec2};
use image::RgbaImage;

use crate::platform::{self, DesktopRect};
use crate::ui;

pub fn run(path: &Path, at: Option<DesktopRect>, delete_after_load: bool) -> Result<(), String> {
    let image = image::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?.to_rgba8();
    if delete_after_load {
        let _ = std::fs::remove_file(path);
    }
    let source = (!delete_after_load).then(|| path.to_path_buf());
    // The window shows the image at its real pixel size; the first frame fixes the position.
    let scale = at
        .and_then(|r| xcap::Monitor::from_point(r.x, r.y).ok())
        .and_then(|m| m.scale_factor().ok())
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0);
    let logical = vec2(image.width() as f32, image.height() as f32)
        / if cfg!(target_os = "macos") { 2.0f32.min(scale.max(1.0)) } else { scale };
    let viewport = egui::ViewportBuilder::default()
        .with_title("Snapsmith - Pin")
        .with_app_id("snapsmith")
        .with_decorations(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_resizable(false)
        .with_inner_size(logical);
    // A pin as large as its monitor covers it, which needs the full-screen renderer.
    let monitor = at.and_then(|r| xcap::Monitor::from_point(r.x, r.y).ok());
    let covers = monitor
        .and_then(|m| Some((m.width().ok()?, m.height().ok()?)))
        .is_some_and(|(w, h)| image.width() >= w && image.height() >= h);
    let kind = if covers { crate::app::Kind::FullScreen } else { crate::app::Kind::Accessory };
    let mut image = Some(image);
    crate::app::run_window("Snapsmith - Pin", viewport, kind, |_cc| Pin {
        image: image.take().expect("pin built once"),
        source: source.clone(),
        at,
        zoom: 1.0,
        base_size: logical,
        texture: None,
        frame_no: 0,
    })
    .map_err(|e| format!("could not pin the image: {e}"))
}

struct Pin {
    image: RgbaImage,
    source: Option<PathBuf>,
    at: Option<DesktopRect>,
    zoom: f32,
    base_size: egui::Vec2,
    texture: Option<egui::TextureHandle>,
    frame_no: u64,
}

impl eframe::App for Pin {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.frame_no == 0
            && let Some(w) = frame.winit_window()
        {
            if let Some(r) = self.at {
                platform::place(w, r);
            }
            platform::float_window(w);
        }
        self.frame_no += 1;
        crate::app::shot_hook(&ctx, self.frame_no, "pin");

        let texture = self
            .texture
            .get_or_insert_with(|| {
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [self.image.width() as usize, self.image.height() as usize],
                    self.image.as_raw(),
                );
                ctx.load_texture("pin", ci, TextureOptions::LINEAR)
            })
            .clone();
        let screen = ctx.viewport_rect();
        ui.painter().image(
            texture.id(),
            screen,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        ui.painter().rect_stroke(screen, 0.0, Stroke::new(1.0, ui::ACCENT), egui::StrokeKind::Inside);

        let resp = ui.interact(screen, egui::Id::unique("pin"), Sense::click_and_drag());
        if resp.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if resp.hovered() && scroll.abs() > 0.5 {
            self.zoom = (self.zoom * (1.0 + scroll * 0.0015)).clamp(0.1, 8.0);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.base_size * self.zoom));
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)) {
            let _ = crate::clipboard::copy_image(&self.image);
        }
        resp.context_menu(|ui| {
            if ui.button("Copy").clicked() {
                let _ = crate::clipboard::copy_image(&self.image);
                ui.close();
            }
            if ui.button("Save as...").clicked() {
                ui.close();
                if let Some(path) =
                    rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name("Snapsmith.png").save_file()
                    && let Err(e) = self.image.save(&path)
                {
                    crate::log::warning!("could not save {}: {e}", path.display());
                }
            }
            if let Some(src) = &self.source
                && ui.button("Show in folder").clicked()
            {
                crate::app::reveal(src);
                ui.close();
            }
            if ui.button("Actual size").clicked() {
                self.zoom = 1.0;
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.base_size));
                ui.close();
            }
            ui.separator();
            if ui.button("Close").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}
