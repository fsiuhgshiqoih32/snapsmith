//! The full-screen region selector, shown over a frozen picture of the monitor.
//!
//! Drag to select, or click a window to take it. After selecting, a toolbar next to the
//! selection offers annotation tools and what to do with the result. Esc or right-click cancels.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, CursorIcon, Key, Modifiers, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use image::RgbaImage;

use snapsmith::editor::{Bounds, Op, P, PALETTE, Shape};

use crate::capture::{Shot, WindowRect};
use crate::platform::{self, DesktopRect};
use crate::ui::canvas::{Canvas, View};
use crate::ui::icons::{self, Icon};
use crate::ui::{self, toolbar};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Screenshot with annotation tools.
    Capture,
    /// Just pick an area (for recording or text recognition).
    Area,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Enter, double-click or the check mark: the after-capture tasks from the settings.
    Default,
    Copy,
    Save,
    SaveAs,
    Upload,
    Ocr,
    Pin,
}

pub struct Picked {
    /// The selected area with annotations.
    pub image: RgbaImage,
    pub action: Action,
    /// The selection in the shot's pixels: x, y, w, h.
    pub region: (u32, u32, u32, u32),
    /// The selection on the desktop.
    pub desktop: DesktopRect,
    /// The monitor the selection was made on.
    pub monitor: DesktopRect,
}

/// For the self-test and README screenshots: select, annotate and confirm without a user.
#[derive(Clone, Debug, Default)]
pub struct Script {
    pub annotate: bool,
    /// Save a picture of the overlay window here before confirming.
    pub screenshot: Option<std::path::PathBuf>,
}

/// Shows the overlay and waits. `Ok(None)` if the user cancelled.
/// A frozen screen, or one still being captured.
pub enum Source {
    Ready(Shot),
    Capturing(std::thread::JoinHandle<Result<Shot, String>>),
}

impl Source {
    fn get(self) -> Result<Shot, String> {
        match self {
            Source::Ready(s) => Ok(s),
            Source::Capturing(h) => h.join().map_err(|_| "screen capture crashed".to_string())?,
        }
    }
}

pub fn run(shot: Shot, mode: Mode, magnifier: bool, script: Option<Script>) -> Result<Option<Picked>, String> {
    let rect = shot.rect;
    run_source(rect, Source::Ready(shot), mode, magnifier, script)
}

/// Opens the overlay for the monitor at `rect` while its capture may still be running.
pub fn run_source(
    rect: DesktopRect,
    source: Source,
    mode: Mode,
    magnifier: bool,
    script: Option<Script>,
) -> Result<Option<Picked>, String> {
    let result: Arc<Mutex<Option<Picked>>> = Arc::default();
    let out = result.clone();
    // A first guess at the monitor in logical points; the first frame puts it exactly in place.
    let monitor_scale = if cfg!(target_os = "macos") { 1.0 } else { scale_of(rect) };
    let viewport = egui::ViewportBuilder::default()
        .with_title("Snapsmith")
        .with_app_id("snapsmith")
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_has_shadow(false)
        .with_position(pos2(rect.x as f32 / monitor_scale, rect.y as f32 / monitor_scale))
        .with_inner_size(vec2(rect.w as f32 / monitor_scale, rect.h as f32 / monitor_scale));
    crate::log::timing("overlay starting");
    let mut source = Some(source);
    crate::app::run_window_try("Snapsmith", viewport, true, |_cc| {
        let shot = source.take().ok_or("overlay built twice")?.get()?;
        Ok(Overlay::new(shot, mode, magnifier, script.clone(), out.clone()))
    })
    .map_err(|e| format!("could not open the capture window: {e}"))?;
    let picked = result.lock().unwrap().take();
    Ok(picked)
}

/// The monitor's own scaling on Windows and X11, to turn desktop pixels into logical points.
fn scale_of(rect: DesktopRect) -> f32 {
    let (cx, cy) = rect.center();
    xcap::Monitor::from_point(cx, cy).ok().and_then(|m| m.scale_factor().ok()).filter(|s| *s > 0.0).unwrap_or(1.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Handle {
    const ALL: [Handle; 8] =
        [Handle::N, Handle::S, Handle::E, Handle::W, Handle::NE, Handle::NW, Handle::SE, Handle::SW];

    fn point(self, r: Rect) -> Pos2 {
        let (x0, x1, y0, y1, cx, cy) = (r.min.x, r.max.x, r.min.y, r.max.y, r.center().x, r.center().y);
        match self {
            Handle::N => pos2(cx, y0),
            Handle::S => pos2(cx, y1),
            Handle::E => pos2(x1, cy),
            Handle::W => pos2(x0, cy),
            Handle::NE => pos2(x1, y0),
            Handle::NW => pos2(x0, y0),
            Handle::SE => pos2(x1, y1),
            Handle::SW => pos2(x0, y1),
        }
    }

    fn cursor(self) -> CursorIcon {
        match self {
            Handle::N | Handle::S => CursorIcon::ResizeVertical,
            Handle::E | Handle::W => CursorIcon::ResizeHorizontal,
            Handle::NE | Handle::SW => CursorIcon::ResizeNeSw,
            Handle::NW | Handle::SE => CursorIcon::ResizeNwSe,
        }
    }

    fn apply(self, b: Bounds, p: P) -> Bounds {
        let mut n = b;
        match self {
            Handle::N => n.y0 = p.y,
            Handle::S => n.y1 = p.y,
            Handle::E => n.x1 = p.x,
            Handle::W => n.x0 = p.x,
            Handle::NE => (n.x1, n.y0) = (p.x, p.y),
            Handle::NW => (n.x0, n.y0) = (p.x, p.y),
            Handle::SE => (n.x1, n.y1) = (p.x, p.y),
            Handle::SW => (n.x0, n.y1) = (p.x, p.y),
        }
        Bounds::from_points(P::new(n.x0, n.y0), P::new(n.x1, n.y1))
    }
}

enum SelDrag {
    New { start: P },
    Move { grab: P, orig: Bounds },
    Resize { handle: Handle },
}

pub struct Overlay {
    rect: DesktopRect,
    /// Cover the monitor on the first frame (a fresh window). The standby places it itself.
    pub self_place: bool,
    /// Show only the frozen picture, undimmed and without tools: indistinguishable from the
    /// screen itself, which lets the standby get a window on screen before raising it.
    pub plain: bool,
    scale: f32,
    windows: Vec<WindowRect>,
    canvas: Canvas,
    mode: Mode,
    magnifier: bool,
    script: Option<Script>,
    result: Arc<Mutex<Option<Picked>>>,
    frame_no: u64,
    sel: Option<Bounds>,
    drag: Option<SelDrag>,
    hover_window: Option<usize>,
    closing: bool,
    copied_color: Option<(String, f64)>,
    screenshot_pending: bool,
}

impl Overlay {
    /// The monitor this overlay covers, in desktop units.
    pub fn monitor_rect(&self) -> DesktopRect {
        self.rect
    }

    pub fn new(
        shot: Shot,
        mode: Mode,
        magnifier: bool,
        script: Option<Script>,
        result: Arc<Mutex<Option<Picked>>>,
    ) -> Self {
        let scale = shot.scale;
        let canvas = Canvas::new(Arc::new(shot.image), scale);
        Overlay {
            rect: shot.rect,
            scale,
            windows: shot.windows,
            canvas,
            mode,
            magnifier,
            script,
            result,
            frame_no: 0,
            sel: None,
            drag: None,
            hover_window: None,
            closing: false,
            self_place: true,
            plain: false,
            copied_color: None,
            screenshot_pending: false,
        }
    }

    fn image_bounds(&self) -> Bounds {
        Bounds { x0: 0.0, y0: 0.0, x1: self.canvas.width() as f32, y1: self.canvas.height() as f32 }
    }

    fn clamp(&self, p: P) -> P {
        P::new(p.x.clamp(0.0, self.canvas.width() as f32), p.y.clamp(0.0, self.canvas.height() as f32))
    }

    fn snap_target(&self) -> Bounds {
        match self.hover_window.and_then(|i| self.windows.get(i)) {
            Some(w) => Bounds { x0: w.x, y0: w.y, x1: w.x + w.w, y1: w.y + w.h },
            None => self.image_bounds(),
        }
    }

    fn confirm(&mut self, ctx: &egui::Context, action: Action) {
        let sel = match self.sel {
            Some(s) => s,
            None => self.snap_target(),
        };
        let Some(region) = sel.to_pixels(self.canvas.width(), self.canvas.height()) else { return };
        let image = if self.mode == Mode::Capture {
            self.canvas.export(Some(region))
        } else {
            image::imageops::crop_imm(&*self.canvas.base, region.0, region.1, region.2, region.3).to_image()
        };
        let desktop = DesktopRect {
            x: self.rect.x + (region.0 as f32 / self.scale).round() as i32,
            y: self.rect.y + (region.1 as f32 / self.scale).round() as i32,
            w: (region.2 as f32 / self.scale).round().max(1.0) as u32,
            h: (region.3 as f32 / self.scale).round().max(1.0) as u32,
        };
        *self.result.lock().unwrap() = Some(Picked { image, action, region, desktop, monitor: self.rect });
        self.close(ctx);
    }

    fn close(&mut self, ctx: &egui::Context) {
        self.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn handle_keys(&mut self, ctx: &egui::Context, pointer_img: Option<P>) {
        if self.canvas.is_editing_text() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.close(ctx);
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            self.confirm(ctx, Action::Default);
            return;
        }
        if self.mode == Mode::Capture {
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
                    self.confirm(ctx, a);
                    return;
                }
            }
            if self.sel.is_some() {
                toolbar::handle_tool_keys(ctx, &mut self.canvas);
            }
        }
        // C copies the colour under the pointer while nothing is selected.
        if self.sel.is_none()
            && let Some(p) = pointer_img
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::C))
        {
            let (x, y) = (p.x.floor() as u32, p.y.floor() as u32);
            if x < self.canvas.width() && y < self.canvas.height() {
                let px = self.canvas.base.get_pixel(x, y).0;
                let hex = format!("#{:02X}{:02X}{:02X}", px[0], px[1], px[2]);
                crate::clipboard::copy_text(&hex);
                self.copied_color = Some((hex, ctx.input(|i| i.time)));
            }
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::M)) {
            self.magnifier = !self.magnifier;
        }
        // Arrow keys nudge the selection; Shift moves 10 pixels; Alt resizes.
        if let Some(sel) = &mut self.sel {
            let (w, h) = (self.canvas.width() as f32, self.canvas.height() as f32);
            let (shift, alt) = ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
            let step = if shift { 10.0 } else { 1.0 };
            for (key, dx, dy) in [
                (Key::ArrowLeft, -1.0, 0.0),
                (Key::ArrowRight, 1.0, 0.0),
                (Key::ArrowUp, 0.0, -1.0),
                (Key::ArrowDown, 0.0, 1.0),
            ] {
                let pressed = ctx.input_mut(|i| {
                    let mods = i.modifiers;
                    i.consume_key(mods, key)
                });
                if pressed {
                    if alt {
                        sel.x1 = (sel.x1 + dx * step).clamp(sel.x0 + 1.0, w);
                        sel.y1 = (sel.y1 + dy * step).clamp(sel.y0 + 1.0, h);
                    } else {
                        let (bw, bh) = (sel.width(), sel.height());
                        sel.x0 = (sel.x0 + dx * step).clamp(0.0, w - bw);
                        sel.y0 = (sel.y0 + dy * step).clamp(0.0, h - bh);
                        sel.x1 = sel.x0 + bw;
                        sel.y1 = sel.y0 + bh;
                    }
                }
            }
        }
    }

    fn handle_pointer(&mut self, ui: &egui::Ui, resp: &egui::Response, view: View) {
        let pos = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let p = pos.map(|s| self.clamp(view.to_image(s)));

        // Right-click: drop the selection, or leave.
        if resp.secondary_clicked() {
            if self.sel.is_some() {
                self.sel = None;
                self.canvas.set_tool(None);
            } else {
                self.close(ui.ctx());
            }
            return;
        }

        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = p
        {
            self.drag = Some(match (self.sel, pos) {
                (Some(sel), Some(s)) => {
                    let r = view.bounds_to_screen(sel);
                    if let Some(h) = self.handle_at(r, s) {
                        SelDrag::Resize { handle: h }
                    } else if r.contains(s) {
                        SelDrag::Move { grab: p, orig: sel }
                    } else {
                        SelDrag::New { start: p }
                    }
                }
                _ => SelDrag::New { start: p },
            });
        }
        if resp.dragged_by(egui::PointerButton::Primary)
            && let (Some(p), Some(d)) = (p, &self.drag)
        {
            let (w, h) = (self.canvas.width() as f32, self.canvas.height() as f32);
            self.sel = Some(match d {
                SelDrag::New { start } => Bounds::from_points(*start, p),
                SelDrag::Move { grab, orig } => {
                    let dx = (p.x - grab.x).clamp(-orig.x0, w - orig.x1);
                    let dy = (p.y - grab.y).clamp(-orig.y0, h - orig.y1);
                    Bounds { x0: orig.x0 + dx, y0: orig.y0 + dy, x1: orig.x1 + dx, y1: orig.y1 + dy }
                }
                SelDrag::Resize { handle } => handle.apply(self.sel.unwrap_or_default(), p),
            });
        }
        if resp.drag_stopped_by(egui::PointerButton::Primary) {
            self.drag = None;
            // A tiny drag is a click: take the window under the pointer.
            if self.sel.is_some_and(|s| s.width() * view.zoom < 4.0 || s.height() * view.zoom < 4.0) {
                self.sel = Some(self.snap_target());
            }
            if self.mode == Mode::Area && self.sel.is_some() && self.script.is_none() {
                // Nothing to annotate: the selection is the answer once the button is released.
                self.confirm(ui.ctx(), Action::Default);
            }
        }
        if resp.clicked_by(egui::PointerButton::Primary) && self.sel.is_none() {
            self.sel = Some(self.snap_target());
            if self.mode == Mode::Area {
                self.confirm(ui.ctx(), Action::Default);
            }
        }
        if resp.double_clicked() && self.sel.is_some() {
            self.confirm(ui.ctx(), Action::Default);
        }

        // Which window is under the pointer, while choosing.
        self.hover_window = None;
        if self.sel.is_none()
            && self.drag.is_none()
            && let Some(p) = p
        {
            self.hover_window = self.windows.iter().position(|w| w.contains(p.x, p.y));
        }

        // Cursor shape.
        if let (Some(sel), Some(s)) = (self.sel, pos) {
            let r = view.bounds_to_screen(sel);
            let icon = match &self.drag {
                Some(SelDrag::Move { .. }) => CursorIcon::Grabbing,
                Some(SelDrag::Resize { handle }) => handle.cursor(),
                Some(SelDrag::New { .. }) => CursorIcon::Crosshair,
                None => match self.handle_at(r, s) {
                    Some(h) => h.cursor(),
                    None if r.contains(s) => CursorIcon::Move,
                    None => CursorIcon::Crosshair,
                },
            };
            ui.ctx().set_cursor_icon(icon);
        } else {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
    }

    fn handle_at(&self, r: Rect, s: Pos2) -> Option<Handle> {
        if self.canvas.tool.is_some() {
            return None;
        }
        Handle::ALL.into_iter().find(|h| h.point(r).distance(s) <= 8.0)
    }

    fn paint_dim(&self, ui: &egui::Ui, screen: Rect, view: View) {
        let dim = Color32::from_black_alpha(110);
        let painter = ui.painter();
        match self.sel {
            None => {
                painter.rect_filled(screen, 0.0, Color32::from_black_alpha(55));
            }
            Some(sel) => {
                let r = view.bounds_to_screen(sel);
                painter.rect_filled(Rect::from_min_max(screen.min, pos2(screen.max.x, r.min.y)), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(screen.min.x, r.max.y), screen.max), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(screen.min.x, r.min.y), pos2(r.min.x, r.max.y)), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(screen.max.x, r.max.y)), 0.0, dim);
            }
        }
    }

    fn paint_selection(&self, ui: &egui::Ui, view: View) {
        let painter = ui.painter();
        if let Some(sel) = self.sel {
            let r = view.bounds_to_screen(sel);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, ui::ACCENT), StrokeKind::Outside);
            if self.canvas.tool.is_none() && self.drag.is_none() {
                for h in Handle::ALL {
                    let c = h.point(r);
                    painter.rect_filled(Rect::from_center_size(c, vec2(7.0, 7.0)), 1.0, Color32::WHITE);
                    painter.rect_stroke(
                        Rect::from_center_size(c, vec2(7.0, 7.0)),
                        1.0,
                        Stroke::new(1.0, ui::ACCENT),
                        StrokeKind::Inside,
                    );
                }
            }
            if let Some((x, y, w, h)) = sel.to_pixels(self.canvas.width(), self.canvas.height()) {
                let label = format!("{w} x {h}");
                let anchor = if r.min.y > 26.0 { r.left_top() - vec2(0.0, 6.0) } else { r.left_top() + vec2(6.0, 6.0) };
                let align = if r.min.y > 26.0 { egui::Align2::LEFT_BOTTOM } else { egui::Align2::LEFT_TOP };
                tag(painter, anchor, align, &label);
                let _ = (x, y);
            }
        } else if let Some(w) = self.hover_window.and_then(|i| self.windows.get(i)) {
            let r = view.bounds_to_screen(Bounds { x0: w.x, y0: w.y, x1: w.x + w.w, y1: w.y + w.h });
            painter.rect_filled(r, 0.0, ui::ACCENT.gamma_multiply(0.12));
            painter.rect_stroke(r, 0.0, Stroke::new(2.0, ui::ACCENT), StrokeKind::Inside);
            if !w.title.is_empty() {
                let title: String = w.title.chars().take(60).collect();
                tag(painter, r.left_top() + vec2(6.0, 6.0), egui::Align2::LEFT_TOP, &title);
            }
        }
    }

    fn paint_crosshair_and_loupe(&mut self, ui: &egui::Ui, screen: Rect, view: View, pointer: Pos2) {
        let painter = ui.painter().clone();
        let line = Stroke::new(1.0, Color32::from_white_alpha(70));
        painter.line_segment([pos2(screen.min.x, pointer.y), pos2(screen.max.x, pointer.y)], line);
        painter.line_segment([pos2(pointer.x, screen.min.y), pos2(pointer.x, screen.max.y)], line);
        if !self.magnifier {
            return;
        }
        let p = view.to_image(pointer);
        let (px, py) = (p.x.floor(), p.y.floor());
        const CELLS: f32 = 15.0;
        const SIZE: f32 = 135.0;
        let half = (CELLS / 2.0).floor();
        let src = Bounds { x0: px - half, y0: py - half, x1: px - half + CELLS, y1: py - half + CELLS };
        let mut origin = pointer + vec2(22.0, 22.0);
        if origin.x + SIZE > screen.max.x {
            origin.x = pointer.x - 22.0 - SIZE;
        }
        if origin.y + SIZE + 26.0 > screen.max.y {
            origin.y = pointer.y - 22.0 - SIZE - 26.0;
        }
        let dest = Rect::from_min_size(origin, vec2(SIZE, SIZE));
        painter.rect_filled(dest.expand(1.0), 2.0, Color32::BLACK);
        // Clip the source to the image, keeping cells square.
        let img = self.image_bounds();
        let cell = SIZE / CELLS;
        let cs =
            Bounds { x0: src.x0.max(img.x0), y0: src.y0.max(img.y0), x1: src.x1.min(img.x1), y1: src.y1.min(img.y1) };
        if !cs.is_empty() {
            let d = Rect::from_min_max(
                dest.min + vec2((cs.x0 - src.x0) * cell, (cs.y0 - src.y0) * cell),
                dest.min + vec2((cs.x1 - src.x0) * cell, (cs.y1 - src.y0) * cell),
            );
            self.canvas.paint_zoomed(ui, d, cs);
        }
        let grid = Stroke::new(1.0, Color32::from_black_alpha(45));
        for i in 1..CELLS as i32 {
            let o = i as f32 * cell;
            painter.line_segment([pos2(dest.min.x + o, dest.min.y), pos2(dest.min.x + o, dest.max.y)], grid);
            painter.line_segment([pos2(dest.min.x, dest.min.y + o), pos2(dest.max.x, dest.min.y + o)], grid);
        }
        let center = Rect::from_min_size(dest.min + vec2(half * cell, half * cell), vec2(cell, cell));
        painter.rect_stroke(center, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Middle);
        painter.rect_stroke(dest, 2.0, Stroke::new(1.0, Color32::from_white_alpha(160)), StrokeKind::Outside);

        let (x, y) = (px as i64, py as i64);
        let color = if x >= 0 && y >= 0 && (x as u32) < self.canvas.width() && (y as u32) < self.canvas.height() {
            let c = self.canvas.base.get_pixel(x as u32, y as u32).0;
            format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
        } else {
            String::new()
        };
        let now = ui.input(|i| i.time);
        let info = match &self.copied_color {
            Some((hex, t)) if now - t < 1.5 => format!("Copied {hex}"),
            _ => format!("{x}, {y}   {color}"),
        };
        tag(&painter, pos2(dest.min.x, dest.max.y + 6.0), egui::Align2::LEFT_TOP, &info);
    }

    fn toolbar(&mut self, ctx: &egui::Context, screen: Rect, view: View) {
        let Some(sel) = self.sel else { return };
        if self.drag.is_some() {
            return;
        }
        let r = view.bounds_to_screen(sel);
        let id = egui::Id::unique("overlay-toolbar");
        let size = ctx.memory(|m| m.area_rect(id)).map(|r| r.size()).unwrap_or(vec2(900.0, 44.0));
        let gap = 8.0;
        let mut pos = pos2(r.center().x - size.x / 2.0, r.max.y + gap);
        if pos.y + size.y > screen.max.y - 4.0 {
            pos.y = r.min.y - gap - size.y;
            if pos.y < screen.min.y + 4.0 {
                pos.y = r.max.y - size.y - gap;
            }
        }
        pos.x = pos.x.clamp(screen.min.x + 4.0, (screen.max.x - size.x - 4.0).max(screen.min.x + 4.0));
        let mut action = None;
        let mut cancel = false;
        egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::new()
                .fill(ui::PANEL)
                .corner_radius(8)
                .stroke(Stroke::new(1.0, Color32::from_white_alpha(25)))
                .inner_margin(egui::Margin::symmetric(6, 4))
                .shadow(egui::Shadow { offset: [0, 3], blur: 12, spread: 0, color: Color32::from_black_alpha(90) })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if self.mode == Mode::Capture {
                            toolbar::annotation_controls(ui, &mut self.canvas, "Adjust selection (V)");
                            toolbar::separator(ui);
                            let buttons = [
                                (Icon::Copy, Action::Copy, "Copy (Ctrl+C)"),
                                (Icon::Save, Action::Save, "Save (Ctrl+S)"),
                                (Icon::SaveAs, Action::SaveAs, "Save as... (Ctrl+Shift+S)"),
                                (Icon::Upload, Action::Upload, "Upload (Ctrl+U)"),
                                (Icon::Ocr, Action::Ocr, "Copy text from image (Ctrl+T)"),
                                (Icon::Pin, Action::Pin, "Pin to screen (Ctrl+P)"),
                            ];
                            for (icon, a, tip) in buttons {
                                if icons::button(ui, icon, false, true, tip).clicked() {
                                    action = Some(a);
                                }
                            }
                            toolbar::separator(ui);
                        }
                        if icons::button(ui, Icon::Cancel, false, true, "Cancel (Esc)").clicked() {
                            cancel = true;
                        }
                        if icons::button(ui, Icon::Done, false, true, "Done (Enter)").clicked() {
                            action = Some(Action::Default);
                        }
                    });
                });
        });
        if cancel {
            self.close(ctx);
        } else if let Some(a) = action {
            self.confirm(ctx, a);
        }
    }

    fn run_script(&mut self, ctx: &egui::Context) {
        let Some(script) = self.script.clone() else { return };
        let (w, h) = (self.canvas.width() as f32, self.canvas.height() as f32);
        match self.frame_no {
            4 => {
                self.sel = Some(Bounds { x0: w * 0.2, y0: h * 0.22, x1: w * 0.78, y1: h * 0.74 });
                if script.annotate && self.mode == Mode::Capture {
                    // Sized for the picture, so the demo looks the same on any monitor.
                    let u = (w / 1600.0).max(self.scale);
                    let c = |i: usize| PALETTE[i];
                    let ops = [
                        Op {
                            shape: Shape::Rect { a: P::new(w * 0.27, h * 0.3), b: P::new(w * 0.47, h * 0.45) },
                            color: c(0),
                            width: 4.0 * u,
                        },
                        Op {
                            shape: Shape::Arrow { a: P::new(w * 0.62, h * 0.62), b: P::new(w * 0.49, h * 0.47) },
                            color: c(0),
                            width: 4.0 * u,
                        },
                        Op {
                            shape: Shape::Step { at: P::new(w * 0.27, h * 0.3), number: 1, radius: 14.0 * u },
                            color: c(4),
                            width: 4.0 * u,
                        },
                        Op {
                            shape: Shape::Highlight { a: P::new(w * 0.52, h * 0.30), b: P::new(w * 0.72, h * 0.34) },
                            color: c(2),
                            width: 4.0 * u,
                        },
                        Op {
                            shape: Shape::Pixelate {
                                a: P::new(w * 0.52, h * 0.38),
                                b: P::new(w * 0.72, h * 0.44),
                                block: 12.0 * u,
                            },
                            color: c(0),
                            width: 4.0 * u,
                        },
                        Op {
                            shape: Shape::Text {
                                at: P::new(w * 0.56, h * 0.64),
                                text: "Click here".into(),
                                font_px: 26.0 * u,
                            },
                            color: c(0),
                            width: 4.0 * u,
                        },
                    ];
                    for op in ops {
                        self.canvas.push(op);
                    }
                    self.canvas.set_tool(Some(snapsmith::editor::Tool::Arrow));
                }
            }
            12 => {
                if let Some(path) = script.screenshot {
                    self.screenshot_pending = true;
                    let ctx2 = ctx.clone();
                    ctx.request_screenshot(move |img| {
                        if let Err(e) = crate::app::save_color_image(&img, &path) {
                            eprintln!("could not save screenshot: {e}");
                        }
                        ctx2.data_mut(|d| d.insert_temp(egui::Id::unique("shot-saved"), true));
                        ctx2.request_repaint();
                    });
                }
            }
            n if n >= 14 => {
                let saved = ctx.data(|d| d.get_temp::<bool>(egui::Id::unique("shot-saved")).unwrap_or(false));
                if !self.screenshot_pending || saved || n > 300 {
                    self.confirm(ctx, Action::Default);
                }
            }
            _ => {}
        }
        ctx.request_repaint();
    }
}

/// A small dark label.
fn tag(painter: &egui::Painter, pos: Pos2, align: egui::Align2, text: &str) {
    let galley = painter.layout_no_wrap(text.to_string(), egui::FontId::monospace(12.0), Color32::WHITE);
    let rect = align.anchor_size(pos, galley.size() + vec2(10.0, 6.0));
    painter.rect_filled(rect, 4.0, Color32::from_black_alpha(190));
    painter.galley(rect.min + vec2(5.0, 3.0), galley, Color32::WHITE);
}

impl eframe::App for Overlay {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.frame_no < 3 {
            crate::log::timing(&format!("overlay frame {}", self.frame_no));
        }
        if self.frame_no == 4 && std::env::var_os("SNAPSMITH_TIMING_EXIT").is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.plain {
            let screen = ctx.viewport_rect();
            let view = View { origin: screen.min, zoom: screen.width() / self.canvas.width().max(1) as f32 };
            self.canvas.paint_image(ui, view);
            return;
        }
        if self.frame_no == 0
            && self.self_place
            && let Some(w) = frame.winit_window()
        {
            platform::cover_monitor(w, self.rect);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if self.frame_no == 2 {
            // Some window managers only honour focus once the window is mapped.
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.frame_no += 1;
        if self.closing {
            return;
        }

        let screen = ctx.viewport_rect();
        let view = View { origin: screen.min, zoom: screen.width() / self.canvas.width().max(1) as f32 };
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let pointer_img = pointer.map(|p| view.to_image(p));

        self.handle_keys(&ctx, pointer_img);
        if self.closing {
            return;
        }
        if self.script.is_some() {
            self.run_script(&ctx);
        }

        let resp = ui.interact(screen, egui::Id::unique("overlay-surface"), Sense::click_and_drag());
        let drawing = self.mode == Mode::Capture
            && self.sel.is_some()
            && self.canvas.tool.is_some()
            && self.drag.is_none()
            && self.canvas.interact(ui, &resp, view);
        if !drawing {
            self.handle_pointer(ui, &resp, view);
        } else if resp.secondary_clicked() {
            self.canvas.set_tool(None);
        }
        if self.closing {
            return;
        }

        self.canvas.paint_image(ui, view);
        self.paint_dim(ui, screen, view);
        self.paint_selection(ui, view);
        let choosing = self.sel.is_none() || matches!(self.drag, Some(SelDrag::New { .. } | SelDrag::Resize { .. }));
        if choosing && let Some(p) = pointer {
            self.paint_crosshair_and_loupe(ui, screen, view, p);
        }
        if self.mode == Mode::Capture && self.sel.is_some() {
            if self.canvas.tool.is_some() {
                ctx.set_cursor_icon(CursorIcon::Crosshair);
            }
            self.canvas.paint_live(ui, view);
        }
        if self.sel.is_none() && self.frame_no < 400 {
            let hint = match self.mode {
                Mode::Capture => {
                    "Drag to select  ·  Click a window  ·  Enter for the whole screen  ·  C copies colour  ·  Esc cancels"
                }
                Mode::Area => {
                    "Drag to select an area  ·  Click a window  ·  Enter for the whole screen  ·  Esc cancels"
                }
            };
            tag(ui.painter(), pos2(screen.center().x, screen.min.y + 18.0), egui::Align2::CENTER_TOP, hint);
        }
        self.toolbar(&ctx, screen, view);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}
