//! The image editor: annotate an existing image (from history, a file, or the clipboard flow),
//! crop it, then copy, save, upload, read text or pin it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, CursorIcon, Key, Modifiers, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use image::RgbaImage;

use snapsmith::config::Settings;
use snapsmith::editor::{Bounds, P};

use crate::overlay::Action;
use crate::ui::canvas::{Canvas, View};
use crate::ui::icons::{self, Icon};
use crate::ui::{self, toolbar};

/// Shows the editor. Returns the edited image and the action that closed it, if any.
pub fn run(
    image: RgbaImage,
    source: Option<PathBuf>,
    settings: Settings,
) -> Result<Option<(RgbaImage, Action)>, String> {
    let result: Arc<Mutex<Option<(RgbaImage, Action)>>> = Arc::default();
    let out = result.clone();
    let title = match &source {
        Some(p) => {
            format!("{} - Snapsmith", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        }
        None => "Snapsmith - Editor".to_string(),
    };
    let (w, h) = (image.width() as f32, image.height() as f32);
    let size = vec2((w + 40.0).clamp(1060.0, 1500.0), (h + 130.0).clamp(560.0, 950.0));
    let viewport = egui::ViewportBuilder::default()
        .with_title(title.clone())
        .with_app_id("snapsmith")
        .with_inner_size(size)
        .with_min_inner_size(vec2(640.0, 360.0));
    let image = Arc::new(image);
    crate::app::run_window(&title, viewport, crate::app::Kind::Normal, |cc| {
        let mut canvas = Canvas::new(image.clone(), cc.egui_ctx.pixels_per_point());
        canvas.nearest = false;
        Editor {
            canvas,
            source: source.clone(),
            settings: settings.clone(),
            result: out.clone(),
            zoom: None,
            pan: vec2(0.0, 0.0),
            crop: None,
            crop_tool: false,
            crop_drag: None,
            status: None,
            closing: false,
            frame_no: 0,
        }
    })
    .map_err(|e| format!("could not open the editor: {e}"))?;
    let r = result.lock().unwrap().take();
    Ok(r)
}

struct Editor {
    canvas: Canvas,
    source: Option<PathBuf>,
    settings: Settings,
    result: Arc<Mutex<Option<(RgbaImage, Action)>>>,
    /// None = fit to window.
    zoom: Option<f32>,
    pan: egui::Vec2,
    crop: Option<Bounds>,
    crop_tool: bool,
    crop_drag: Option<P>,
    status: Option<(String, f64, bool)>,
    closing: bool,
    frame_no: u64,
}

impl Editor {
    fn crop_px(&self) -> Option<(u32, u32, u32, u32)> {
        self.crop.and_then(|b| b.to_pixels(self.canvas.width(), self.canvas.height()))
    }

    fn export(&mut self) -> RgbaImage {
        let crop = self.crop_px();
        self.canvas.export(crop)
    }

    fn say(&mut self, ctx: &egui::Context, msg: impl Into<String>, error: bool) {
        self.status = Some((msg.into(), ctx.input(|i| i.time), error));
    }

    fn act(&mut self, ctx: &egui::Context, action: Action) {
        let img = self.export();
        match action {
            Action::Copy => match crate::clipboard::copy_image(&img) {
                Ok(()) => self.say(ctx, "Copied", false),
                Err(e) => self.say(ctx, e, true),
            },
            Action::Save => {
                let path = match &self.source {
                    Some(p) => p.clone(),
                    None => crate::tasks::planned_path(&self.settings, img.width(), img.height(), ""),
                };
                self.save_to(ctx, &img, path);
            }
            Action::SaveAs => {
                let mut dialog =
                    rfd::FileDialog::new().add_filter("PNG", &["png"]).add_filter("JPEG", &["jpg", "jpeg"]);
                let name = self
                    .source
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Snapsmith.png".into());
                dialog = dialog.set_file_name(name);
                if let Some(dir) = self.source.as_ref().and_then(|p| p.parent()) {
                    dialog = dialog.set_directory(dir);
                } else {
                    dialog = dialog.set_directory(self.settings.capture.save_dir());
                }
                if let Some(path) = dialog.save_file() {
                    self.save_to(ctx, &img, path);
                }
            }
            Action::Upload | Action::Ocr | Action::Pin | Action::Default => {
                *self.result.lock().unwrap() = Some((img, action));
                self.closing = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn save_to(&mut self, ctx: &egui::Context, img: &RgbaImage, path: PathBuf) {
        let format =
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg")) {
                snapsmith::config::ImageFormat::Jpg
            } else {
                snapsmith::config::ImageFormat::Png
            };
        let result = crate::app::encode(img, format, self.settings.capture.jpeg_quality).and_then(|bytes| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, bytes).map_err(|e| e.to_string())
        });
        match result {
            Ok(()) => {
                let mut entry = snapsmith::history::Entry::now("image");
                entry.path = Some(path.clone());
                entry.width = img.width();
                entry.height = img.height();
                let _ = snapsmith::history::append(&snapsmith::history::default_path(), &entry);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.say(ctx, format!("Saved {name}"), false);
            }
            Err(e) => self.say(ctx, format!("Could not save: {e}"), true),
        }
    }

    fn view(&self, area: Rect) -> View {
        let (w, h) = (self.canvas.width() as f32, self.canvas.height() as f32);
        let fit = ((area.width() - 24.0) / w).min((area.height() - 24.0) / h).clamp(0.01, 1.0);
        let zoom = self.zoom.unwrap_or(fit);
        let size = vec2(w, h) * zoom;
        let origin = area.center() - size / 2.0 + self.pan;
        View { origin, zoom }
    }
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.closing {
            return;
        }
        self.frame_no += 1;
        crate::app::shot_hook(&ctx, self.frame_no, "editor");
        let typing = ctx.egui_wants_keyboard_input() || self.canvas.is_editing_text();
        if !typing {
            toolbar::handle_tool_keys(&ctx, &mut self.canvas);
            let shortcuts = [
                (Modifiers::COMMAND, Key::C, Action::Copy),
                (Modifiers::COMMAND, Key::S, Action::Save),
                (Modifiers::COMMAND | Modifiers::SHIFT, Key::S, Action::SaveAs),
                (Modifiers::COMMAND, Key::U, Action::Upload),
                (Modifiers::COMMAND, Key::T, Action::Ocr),
                (Modifiers::COMMAND, Key::P, Action::Pin),
            ];
            for (m, k, a) in shortcuts {
                if ctx.input_mut(|i| i.consume_key(m, k)) {
                    self.act(&ctx, a);
                }
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                if self.crop.is_some() || self.crop_tool {
                    self.crop = None;
                    self.crop_tool = false;
                } else {
                    self.canvas.set_tool(None);
                }
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::C)) {
                self.crop_tool = !self.crop_tool;
                self.canvas.set_tool(None);
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Num0)) {
                self.zoom = None;
                self.pan = vec2(0.0, 0.0);
            }
        }

        egui::Panel::top("editor-toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                toolbar::annotation_controls(ui, &mut self.canvas, "Move and zoom (V)");
                if self.canvas.tool.is_some() {
                    self.crop_tool = false;
                }
                toolbar::separator(ui);
                if icons::button(ui, Icon::Crop, self.crop_tool, true, "Crop (C), Esc removes it").clicked() {
                    self.crop_tool = !self.crop_tool;
                    self.canvas.set_tool(None);
                }
                toolbar::separator(ui);
                let buttons = [
                    (Icon::Copy, Action::Copy, "Copy (Ctrl+C)"),
                    (
                        Icon::Save,
                        Action::Save,
                        if self.source.is_some() { "Save, replacing the file (Ctrl+S)" } else { "Save (Ctrl+S)" },
                    ),
                    (Icon::SaveAs, Action::SaveAs, "Save as... (Ctrl+Shift+S)"),
                    (Icon::Upload, Action::Upload, "Upload (Ctrl+U)"),
                    (Icon::Ocr, Action::Ocr, "Copy text from image (Ctrl+T)"),
                    (Icon::Pin, Action::Pin, "Pin to screen (Ctrl+P)"),
                ];
                for (icon, a, tip) in buttons {
                    if icons::button(ui, icon, false, true, tip).clicked() {
                        self.act(&ctx, a);
                    }
                }
            });
            ui.add_space(2.0);
        });

        egui::Panel::bottom("editor-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let (w, h) = match self.crop_px() {
                    Some((_, _, w, h)) => (w, h),
                    None => (self.canvas.width(), self.canvas.height()),
                };
                ui.label(RichText::new(format!("{w} x {h}")).color(ui::MUTED).size(12.0));
                let now = ctx.input(|i| i.time);
                if let Some((msg, t, err)) = &self.status
                    && now - t < 4.0
                {
                    ui.label(RichText::new(msg).size(12.0).color(if *err { ui::DANGER } else { ui::OK }));
                    ctx.request_repaint_after(std::time::Duration::from_millis(500));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let area = ctx.content_rect();
                    let z = self.view(area).zoom;
                    if ui.small_button("Fit").clicked() {
                        self.zoom = None;
                        self.pan = vec2(0.0, 0.0);
                    }
                    if ui.small_button("1:1").clicked() {
                        self.zoom = Some(1.0 / ctx.pixels_per_point());
                        self.pan = vec2(0.0, 0.0);
                    }
                    ui.label(
                        RichText::new(format!("{:.0}%", z * ctx.pixels_per_point() * 100.0))
                            .color(ui::MUTED)
                            .size(12.0),
                    );
                });
            });
        });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::from_rgb(0x15, 0x16, 0x19))).show(
            ui,
            |ui| {
                let area = ui.max_rect();
                let view = self.view(area);
                let resp = ui.interact(area, egui::Id::unique("editor-canvas"), Sense::click_and_drag());

                // Ctrl+wheel (or pinch) zooms around the pointer; the wheel alone pans.
                let (zoom_delta, scroll) = ctx.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
                if resp.hovered() {
                    if (zoom_delta - 1.0).abs() > 1e-3 {
                        let old = view.zoom;
                        let new = (old * zoom_delta).clamp(0.02, 32.0);
                        if let Some(p) = resp.hover_pos() {
                            let img_p = view.to_image(p);
                            let (w, h) = (self.canvas.width() as f32, self.canvas.height() as f32);
                            // Keep the image point under the pointer still.
                            let new_origin = p - vec2(img_p.x, img_p.y) * new;
                            let centered = area.center() - vec2(w, h) * new / 2.0;
                            self.pan = new_origin - centered;
                        }
                        self.zoom = Some(new);
                    } else if scroll != egui::Vec2::ZERO {
                        self.pan += scroll;
                    }
                }
                let view = self.view(area);

                let drawn = !self.crop_tool && self.canvas.interact(ui, &resp, view);
                if self.crop_tool {
                    ctx.set_cursor_icon(CursorIcon::Crosshair);
                    let p = resp.interact_pointer_pos().map(|s| {
                        let p = view.to_image(s);
                        P::new(p.x.clamp(0.0, self.canvas.width() as f32), p.y.clamp(0.0, self.canvas.height() as f32))
                    });
                    if resp.drag_started() {
                        self.crop_drag = p;
                    }
                    if let (Some(a), Some(b)) = (self.crop_drag, p)
                        && resp.dragged()
                    {
                        self.crop = Some(Bounds::from_points(a, b)).filter(|c| !c.is_empty());
                    }
                    if resp.drag_stopped() {
                        self.crop_drag = None;
                    }
                } else if !drawn && resp.dragged() {
                    // No tool: dragging pans.
                    self.pan += resp.drag_delta();
                    ctx.set_cursor_icon(CursorIcon::Grabbing);
                }

                let painter = ui.painter_at(area);
                let img_rect = Rect::from_min_size(
                    view.origin,
                    vec2(self.canvas.width() as f32, self.canvas.height() as f32) * view.zoom,
                );
                painter.rect_filled(img_rect.translate(vec2(0.0, 2.0)).expand(2.0), 2.0, Color32::from_black_alpha(80));
                let mut sub = ui.new_child(egui::UiBuilder::new().max_rect(area));
                sub.set_clip_rect(area);
                self.canvas.paint_image(&sub, view);
                if let Some(c) = self.crop {
                    let r = view.bounds_to_screen(c);
                    let dim = Color32::from_black_alpha(140);
                    for part in [
                        Rect::from_min_max(img_rect.min, pos2(img_rect.max.x, r.min.y)),
                        Rect::from_min_max(pos2(img_rect.min.x, r.max.y), img_rect.max),
                        Rect::from_min_max(pos2(img_rect.min.x, r.min.y), pos2(r.min.x, r.max.y)),
                        Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(img_rect.max.x, r.max.y)),
                    ] {
                        painter.rect_filled(part.intersect(img_rect), 0.0, dim);
                    }
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, ui::ACCENT), StrokeKind::Outside);
                }
                self.canvas.paint_live(&mut sub, view);
            },
        );
    }
}
