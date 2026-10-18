//! An image with annotations on it, and the mouse handling for drawing them. Used by the region
//! overlay and the editor window.
//!
//! Finished annotations are rasterised by `editor::render` (the same code that writes the file)
//! and shown as a texture; only the shape being dragged right now is drawn with egui.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, Key, Modifiers, Pos2, Rect, Shape as EShape, Stroke, StrokeKind, TextureHandle, TextureOptions,
    pos2, vec2,
};
use image::RgbaImage;

use snapsmith::editor::{Bounds, Document, Op, P, PALETTE, Rgba, Shape, Tool, render};

use super::color32;

/// Maps image pixels to screen points.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Screen position of image pixel (0, 0).
    pub origin: Pos2,
    /// Points per image pixel.
    pub zoom: f32,
}

impl View {
    pub fn to_screen(self, p: P) -> Pos2 {
        pos2(self.origin.x + p.x * self.zoom, self.origin.y + p.y * self.zoom)
    }

    pub fn to_image(self, s: Pos2) -> P {
        P::new((s.x - self.origin.x) / self.zoom, (s.y - self.origin.y) / self.zoom)
    }

    pub fn bounds_to_screen(self, b: Bounds) -> Rect {
        Rect::from_min_max(self.to_screen(P::new(b.x0, b.y0)), self.to_screen(P::new(b.x1, b.y1)))
    }
}

pub const SIZE_NAMES: [&str; 3] = ["Thin", "Medium", "Thick"];
/// Per size: stroke, font, step radius, pixelate block, blur radius, in points.
const STROKE: [f32; 3] = [2.0, 4.0, 7.0];
const FONT: [f32; 3] = [16.0, 24.0, 36.0];
const STEP: [f32; 3] = [10.0, 14.0, 20.0];
const BLOCK: [f32; 3] = [7.0, 12.0, 20.0];
const BLUR: [f32; 3] = [4.0, 8.0, 14.0];

enum Drawing {
    Drag { tool: Tool, start: P, end: P },
    Pen { points: Vec<P> },
}

struct TextState {
    at: P,
    buffer: String,
    font_px: f32,
    color: Rgba,
    started: bool,
}

pub struct Canvas {
    pub base: Arc<RgbaImage>,
    pub doc: Document,
    pub tool: Option<Tool>,
    pub size: usize,
    /// Image pixels per point at 100% (the monitor's scale), so sizes look alike on any screen.
    pub unit: f32,
    colors: HashMap<Tool, Rgba>,
    texture: Option<TextureHandle>,
    dirty: bool,
    /// How much the shown texture is shrunk, when the image is bigger than the GPU allows.
    preview_factor: u32,
    pub nearest: bool,
    drawing: Option<Drawing>,
    text: Option<TextState>,
}

impl Canvas {
    pub fn new(base: Arc<RgbaImage>, unit: f32) -> Self {
        Canvas {
            base,
            doc: Document::default(),
            tool: None,
            size: 1,
            unit: unit.max(0.25),
            colors: HashMap::new(),
            texture: None,
            dirty: true,
            preview_factor: 1,
            nearest: true,
            drawing: None,
            text: None,
        }
    }

    pub fn width(&self) -> u32 {
        self.base.width()
    }

    pub fn height(&self) -> u32 {
        self.base.height()
    }

    /// The colour for `tool` (each tool remembers its own; the highlighter starts yellow).
    pub fn color_for(&self, tool: Option<Tool>) -> Rgba {
        let tool = tool.unwrap_or(Tool::Rect);
        *self.colors.get(&tool).unwrap_or(if tool == Tool::Highlight { &PALETTE[2] } else { &PALETTE[0] })
    }

    pub fn color(&self) -> Rgba {
        self.color_for(self.tool)
    }

    pub fn set_color(&mut self, c: Rgba) {
        self.colors.insert(self.tool.unwrap_or(Tool::Rect), c);
        if let Some(t) = &mut self.text {
            t.color = c;
        }
    }

    pub fn set_tool(&mut self, tool: Option<Tool>) {
        self.commit_text();
        self.drawing = None;
        self.tool = tool;
    }

    fn stroke_px(&self) -> f32 {
        STROKE[self.size] * self.unit
    }

    pub fn undo(&mut self) {
        self.commit_text();
        if self.doc.undo() {
            self.dirty = true;
        }
    }

    pub fn redo(&mut self) {
        if self.doc.redo() {
            self.dirty = true;
        }
    }

    pub fn push(&mut self, op: Op) {
        self.doc.push(op);
        self.dirty = true;
    }

    pub fn is_editing_text(&self) -> bool {
        self.text.is_some()
    }

    /// The image with every annotation, optionally cropped to (x, y, w, h).
    pub fn export(&mut self, crop: Option<(u32, u32, u32, u32)>) -> RgbaImage {
        self.commit_text();
        render::export(&self.base, self.doc.ops(), crop)
    }

    /// The GPU texture for the image with annotations, rebuilt when they change.
    pub fn texture(&mut self, ctx: &egui::Context) -> &TextureHandle {
        if self.dirty || self.texture.is_none() {
            self.dirty = false;
            let composed;
            let img: &RgbaImage = if self.doc.ops().is_empty() {
                &self.base
            } else {
                composed = render::compose(&self.base, self.doc.ops());
                &composed
            };
            let max_side = ctx.input(|i| i.max_texture_side).max(1024) as u32;
            let longest = img.width().max(img.height());
            self.preview_factor = longest.div_ceil(max_side).max(1);
            let shrunk;
            let shown: &RgbaImage = if self.preview_factor > 1 {
                let f = self.preview_factor;
                shrunk = image::imageops::thumbnail(img, (img.width() / f).max(1), (img.height() / f).max(1));
                &shrunk
            } else {
                img
            };
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [shown.width() as usize, shown.height() as usize],
                shown.as_raw(),
            );
            let options =
                if self.nearest && self.preview_factor == 1 { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
            match &mut self.texture {
                Some(t) => t.set(color, options),
                None => self.texture = Some(ctx.load_texture("capture", color, options)),
            }
        }
        self.texture.as_ref().unwrap()
    }

    pub fn paint_image(&mut self, ui: &egui::Ui, view: View) {
        let rect = Rect::from_min_size(view.origin, vec2(self.width() as f32, self.height() as f32) * view.zoom);
        let id = self.texture(ui.ctx()).id();
        ui.painter().image(id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }

    /// Draws a magnified part of the image (for the loupe). `src` is in image pixels.
    pub fn paint_zoomed(&mut self, ui: &egui::Ui, dest: Rect, src: Bounds) {
        let (w, h) = (self.width() as f32, self.height() as f32);
        let uv = Rect::from_min_max(pos2(src.x0 / w, src.y0 / h), pos2(src.x1 / w, src.y1 / h));
        let id = self.texture(ui.ctx()).id();
        ui.painter().image(id, dest, uv, Color32::WHITE);
    }

    /// Handles drawing with the current tool. Returns true if the pointer was used for drawing.
    pub fn interact(&mut self, ui: &egui::Ui, response: &egui::Response, view: View) -> bool {
        let Some(tool) = self.tool else { return false };
        let pointer = response.interact_pointer_pos().or_else(|| response.hover_pos());
        let shift = ui.input(|i| i.modifiers.shift);

        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pos) = pointer
        {
            self.commit_text();
            let p = view.to_image(pos);
            self.drawing = match tool {
                Tool::Pen => Some(Drawing::Pen { points: vec![p] }),
                Tool::Text | Tool::Step => None,
                _ => Some(Drawing::Drag { tool, start: p, end: p }),
            };
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = pointer
        {
            let p = view.to_image(pos);
            match &mut self.drawing {
                Some(Drawing::Drag { tool, start, end }) => *end = constrain(*tool, *start, p, shift),
                Some(Drawing::Pen { points }) if points.last().is_none_or(|l| l.dist(p) * view.zoom >= 1.5) => {
                    points.push(p);
                }
                _ => {}
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            if let Some(op) = self.drawing.take().and_then(|d| self.finish(d)) {
                self.push(op);
            }
            return true;
        }
        if response.clicked_by(egui::PointerButton::Primary)
            && let Some(pos) = pointer
        {
            let p = view.to_image(pos);
            match tool {
                Tool::Text => {
                    let was_editing = self.text.is_some();
                    self.commit_text();
                    if !was_editing {
                        self.text = Some(TextState {
                            at: p,
                            buffer: String::new(),
                            font_px: FONT[self.size] * self.unit,
                            color: self.color(),
                            started: false,
                        });
                    }
                }
                Tool::Step => {
                    let op = Op {
                        shape: Shape::Step { at: p, number: self.doc.next_step(), radius: STEP[self.size] * self.unit },
                        color: self.color(),
                        width: self.stroke_px(),
                    };
                    self.push(op);
                }
                _ => {}
            }
        }
        true
    }

    fn finish(&self, d: Drawing) -> Option<Op> {
        let color = self.color();
        let width = self.stroke_px();
        let shape = match d {
            Drawing::Pen { points } => Shape::Pen { points },
            Drawing::Drag { tool, start: a, end: b } => match tool {
                Tool::Rect => Shape::Rect { a, b },
                Tool::Ellipse => Shape::Ellipse { a, b },
                Tool::Arrow => Shape::Arrow { a, b },
                Tool::Line => Shape::Line { a, b },
                Tool::Highlight => Shape::Highlight { a, b },
                Tool::Pixelate => Shape::Pixelate { a, b, block: BLOCK[self.size] * self.unit },
                Tool::Blur => Shape::Blur { a, b, radius: BLUR[self.size] * self.unit },
                Tool::Pen | Tool::Text | Tool::Step => return None,
            },
        };
        Some(Op { shape, color, width })
    }

    /// Draws the shape being dragged, and the text box being typed in.
    pub fn paint_live(&mut self, ui: &mut egui::Ui, view: View) {
        let painter = ui.painter().clone();
        let color = color32(self.color());
        let w = (self.stroke_px() * view.zoom).max(1.0);
        match &self.drawing {
            Some(Drawing::Drag { tool, start, end }) => {
                let (a, b) = (view.to_screen(*start), view.to_screen(*end));
                let r = Rect::from_two_pos(a, b);
                match tool {
                    Tool::Rect => {
                        painter.rect_stroke(r, 0.0, Stroke::new(w, color), StrokeKind::Middle);
                    }
                    Tool::Ellipse => {
                        painter.add(EShape::ellipse_stroke(r.center(), r.size() / 2.0, Stroke::new(w, color)));
                    }
                    Tool::Line => {
                        painter.line_segment([a, b], Stroke::new(w, color));
                    }
                    Tool::Arrow => {
                        let len = a.distance(b);
                        if len > 1.0 {
                            let (head, half) = render::arrow_head(self.stroke_px(), start.dist(*end));
                            let (head, half) = (head * view.zoom, half * view.zoom);
                            let dir = (b - a) / len;
                            let perp = vec2(-dir.y, dir.x);
                            let base = b - dir * head;
                            painter.line_segment([a, b - dir * head * 0.7], Stroke::new(w, color));
                            painter.add(EShape::convex_polygon(
                                vec![b, base + perp * half, base - perp * half],
                                color,
                                Stroke::NONE,
                            ));
                        }
                    }
                    Tool::Highlight => {
                        painter.rect_filled(r, 0.0, color.gamma_multiply(0.4));
                    }
                    Tool::Pixelate | Tool::Blur => {
                        painter.rect_filled(r, 0.0, Color32::from_black_alpha(60));
                        painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);
                        painter.rect_stroke(r.expand(1.0), 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Outside);
                    }
                    _ => {}
                }
            }
            Some(Drawing::Pen { points }) => {
                let pts: Vec<Pos2> = points.iter().map(|p| view.to_screen(*p)).collect();
                painter.add(EShape::line(pts, Stroke::new(w, color)));
            }
            None => {}
        }
        self.text_box(ui, view);
    }

    fn text_box(&mut self, ui: &mut egui::Ui, view: View) {
        let Some(state) = &mut self.text else { return };
        let ctx = ui.ctx().clone();
        let mut commit = false;
        let mut cancel = false;
        // Enter finishes the text; Shift+Enter starts a new line.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            commit = true;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            cancel = true;
        }
        let pos = view.to_screen(state.at);
        let font = egui::FontId::proportional((state.font_px * view.zoom).max(6.0));
        let id = egui::Id::unique("annotation-text");
        let color = color32(state.color);
        let area = egui::Area::new(id.with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain(false)
            .show(&ctx, |ui| {
                let edit = egui::TextEdit::multiline(&mut state.buffer)
                    .id(id)
                    .font(font)
                    .text_color(color)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .desired_rows(1)
                    .desired_width((state.font_px * view.zoom * 12.0).max(80.0))
                    .hint_text("Type, then Enter");
                let r = ui.add(edit);
                if !state.started {
                    r.request_focus();
                    state.started = true;
                }
                r
            });
        let r = area.inner;
        ui.painter().rect_stroke(
            r.rect.expand(3.0),
            2.0,
            Stroke::new(1.0, Color32::from_white_alpha(170)),
            StrokeKind::Outside,
        );
        if r.lost_focus() && !commit && !cancel {
            commit = true;
        }
        if cancel {
            self.text = None;
        } else if commit {
            self.commit_text();
        }
    }

    /// Turns the text being typed into an annotation.
    pub fn commit_text(&mut self) {
        if let Some(t) = self.text.take() {
            let text = t.buffer.trim_end().to_string();
            if !text.trim().is_empty() {
                self.push(Op { shape: Shape::Text { at: t.at, text, font_px: t.font_px }, color: t.color, width: 1.0 });
            }
        }
    }
}

/// Shift keeps rectangles square and lines at 45-degree steps.
fn constrain(tool: Tool, a: P, b: P, shift: bool) -> P {
    if !shift {
        return b;
    }
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    match tool {
        Tool::Line | Tool::Arrow => {
            let angle = dy.atan2(dx);
            let step = std::f32::consts::FRAC_PI_4;
            let snapped = (angle / step).round() * step;
            let len = (dx * dx + dy * dy).sqrt();
            P::new(a.x + snapped.cos() * len, a.y + snapped.sin() * len)
        }
        _ => {
            let side = dx.abs().max(dy.abs());
            P::new(a.x + side * dx.signum(), a.y + side * dy.signum())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_round_trips() {
        let v = View { origin: pos2(10.0, 20.0), zoom: 0.5 };
        let p = P::new(100.0, 40.0);
        let s = v.to_screen(p);
        assert_eq!(s, pos2(60.0, 40.0));
        assert_eq!(v.to_image(s), p);
    }

    #[test]
    fn shift_constrains() {
        let a = P::new(0.0, 0.0);
        let sq = constrain(Tool::Rect, a, P::new(10.0, -4.0), true);
        assert_eq!(sq, P::new(10.0, -10.0));
        let l = constrain(Tool::Line, a, P::new(10.0, 1.0), true);
        assert!((l.y).abs() < 1e-3 && (l.x - 101f32.sqrt()).abs() < 1e-3);
        assert_eq!(constrain(Tool::Line, a, P::new(3.0, 4.0), false), P::new(3.0, 4.0));
    }
}
