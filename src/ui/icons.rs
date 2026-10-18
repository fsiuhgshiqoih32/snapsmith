//! Toolbar icons, painted with egui shapes so they stay sharp at any scale and need no image
//! files or icon fonts.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

use snapsmith::editor::Tool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Select,
    Tool(Tool),
    Undo,
    Redo,
    Copy,
    Save,
    SaveAs,
    Upload,
    Ocr,
    Pin,
    Done,
    Cancel,
    Crop,
    Folder,
    Link,
    Open,
}

pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 18.0;
    let p = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    let st = Stroke::new(1.6 * s.max(0.8), color);
    let thin = Stroke::new(1.2 * s.max(0.8), color);
    match icon {
        Icon::Select => {
            let pts =
                vec![p(-5.0, -7.5), p(-5.0, 6.0), p(-1.5, 2.8), p(1.2, 8.0), p(3.4, 7.0), p(0.8, 1.8), p(5.5, 1.8)];
            painter.add(Shape::closed_line(pts, thin));
        }
        Icon::Tool(Tool::Rect) => {
            painter.rect_stroke(Rect::from_min_max(p(-7.0, -5.5), p(7.0, 5.5)), 0.0, st, StrokeKind::Middle);
        }
        Icon::Tool(Tool::Ellipse) => {
            painter.add(Shape::ellipse_stroke(c, vec2(7.5 * s, 5.8 * s), st));
        }
        Icon::Tool(Tool::Arrow) => arrow(painter, p(-6.5, 6.5), p(6.5, -6.5), st, s),
        Icon::Tool(Tool::Line) => {
            painter.line_segment([p(-7.0, 6.0), p(7.0, -6.0)], st);
        }
        Icon::Tool(Tool::Pen) => {
            painter.add(Shape::closed_line(
                vec![p(-7.5, 7.5), p(-2.6, 5.7), p(7.6, -4.4), p(4.4, -7.6), p(-5.7, 2.6)],
                thin,
            ));
            painter.line_segment([p(-2.6, 5.7), p(-5.7, 2.6)], thin);
        }
        Icon::Tool(Tool::Highlight) => {
            painter.rect_filled(Rect::from_min_max(p(-8.5, 4.0), p(4.0, 8.0)), 1.0, color.gamma_multiply(0.45));
            painter.add(Shape::closed_line(
                vec![p(-4.5, 3.5), p(-1.0, 0.0), p(5.0, -6.0), p(8.0, -3.0), p(2.0, 3.0), p(-1.5, 6.5)],
                thin,
            ));
        }
        Icon::Tool(Tool::Text) => {
            painter.line_segment([p(-6.0, -6.5), p(6.0, -6.5)], st);
            painter.line_segment([p(0.0, -6.5), p(0.0, 7.0)], st);
        }
        Icon::Tool(Tool::Step) => {
            painter.circle_stroke(c, 7.5 * s, st);
            painter.text(c, egui::Align2::CENTER_CENTER, "1", egui::FontId::proportional(10.5 * s), color);
        }
        Icon::Tool(Tool::Pixelate) => {
            let cell = 4.6 * s;
            for (i, j) in [(0, 0), (2, 0), (1, 1), (0, 2), (2, 2)] {
                let min = pos2(c.x - 1.5 * cell + i as f32 * cell, c.y - 1.5 * cell + j as f32 * cell);
                painter.rect_filled(Rect::from_min_size(min, Vec2::splat(cell)), 0.0, color);
            }
            painter.rect_stroke(
                Rect::from_center_size(c, Vec2::splat(3.0 * cell)),
                0.0,
                Stroke::new(1.0, color.gamma_multiply(0.6)),
                StrokeKind::Middle,
            );
        }
        Icon::Tool(Tool::Blur) => {
            for (r, a) in [(8.0, 0.18), (6.0, 0.3), (4.0, 0.55), (2.2, 1.0)] {
                painter.circle_filled(c, r * s, color.gamma_multiply(a));
            }
        }
        Icon::Undo | Icon::Redo => {
            let dir = if icon == Icon::Undo { 1.0 } else { -1.0 };
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI * (0.15 + 0.85 * i as f32 / 16.0);
                    p(dir * a.cos() * 6.0, -a.sin() * 5.5 + 3.0)
                })
                .collect();
            let tip = *pts.last().unwrap();
            painter.add(Shape::line(pts, st));
            painter.add(Shape::convex_polygon(
                vec![
                    tip + vec2(dir * -3.2 * s, -3.0 * s),
                    tip + vec2(dir * 3.2 * s, -0.5 * s),
                    tip + vec2(-dir * s, 3.4 * s),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Copy => {
            painter.rect_stroke(Rect::from_min_max(p(-6.5, -3.5), p(3.0, 7.5)), 1.5, st, StrokeKind::Middle);
            painter.line(vec![p(-3.5, -6.0), p(-3.5, -7.5), p(6.5, -7.5), p(6.5, 3.5), p(5.0, 3.5)], thin);
        }
        Icon::Save | Icon::SaveAs => {
            painter.line_segment([p(0.0, -7.5), p(0.0, 3.0)], st);
            painter.line(vec![p(-4.0, -1.0), p(0.0, 3.0), p(4.0, -1.0)], st);
            painter.line(vec![p(-7.0, 2.5), p(-7.0, 7.0), p(7.0, 7.0), p(7.0, 2.5)], st);
            if icon == Icon::SaveAs {
                for x in [3.5, 5.7, 7.9] {
                    painter.circle_filled(p(x, -6.5), 1.0 * s, color);
                }
            }
        }
        Icon::Upload => {
            painter.line_segment([p(0.0, 4.0), p(0.0, -7.0)], st);
            painter.line(vec![p(-4.0, -3.0), p(0.0, -7.0), p(4.0, -3.0)], st);
            painter.line(vec![p(-7.0, 2.5), p(-7.0, 7.0), p(7.0, 7.0), p(7.0, 2.5)], st);
        }
        Icon::Ocr => {
            for (x, y, dx, dy) in
                [(-7.5, -7.5, 1.0, 1.0), (7.5, -7.5, -1.0, 1.0), (-7.5, 7.5, 1.0, -1.0), (7.5, 7.5, -1.0, -1.0)]
            {
                painter.line(vec![p(x, y + dy * 3.0), p(x, y), p(x + dx * 3.0, y)], thin);
            }
            painter.text(c, egui::Align2::CENTER_CENTER, "T", egui::FontId::proportional(11.0 * s), color);
        }
        Icon::Pin => {
            painter.add(Shape::closed_line(
                vec![p(-3.5, -8.0), p(3.5, -8.0), p(2.5, -5.0), p(4.5, 1.0), p(-4.5, 1.0), p(-2.5, -5.0)],
                thin,
            ));
            painter.line_segment([p(0.0, 1.0), p(0.0, 8.5)], st);
        }
        Icon::Done => {
            painter.line(vec![p(-6.5, 0.5), p(-2.0, 5.0), p(7.0, -5.0)], Stroke::new(2.0 * s.max(0.8), color));
        }
        Icon::Cancel => {
            painter.line_segment([p(-5.5, -5.5), p(5.5, 5.5)], st);
            painter.line_segment([p(5.5, -5.5), p(-5.5, 5.5)], st);
        }
        Icon::Crop => {
            painter.line(vec![p(-4.0, -8.0), p(-4.0, 4.0), p(8.0, 4.0)], st);
            painter.line(vec![p(-8.0, -4.0), p(4.0, -4.0), p(4.0, 8.0)], st);
        }
        Icon::Folder => {
            painter.add(Shape::closed_line(
                vec![p(-8.0, -5.5), p(-3.0, -5.5), p(-1.5, -3.5), p(8.0, -3.5), p(8.0, 6.5), p(-8.0, 6.5)],
                st,
            ));
        }
        Icon::Link => {
            painter.rect_stroke(Rect::from_min_max(p(-8.0, -3.0), p(1.5, 3.0)), 3.0 * s, st, StrokeKind::Middle);
            painter.rect_stroke(Rect::from_min_max(p(-1.5, -3.0), p(8.0, 3.0)), 3.0 * s, st, StrokeKind::Middle);
        }
        Icon::Open => {
            painter.rect_stroke(Rect::from_min_max(p(-7.0, -5.0), p(5.0, 7.0)), 1.5, thin, StrokeKind::Middle);
            painter.line_segment([p(-1.0, 1.0), p(7.5, -7.5)], st);
            painter.line(vec![p(2.0, -7.5), p(7.5, -7.5), p(7.5, -2.0)], st);
        }
    }
}

fn arrow(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke, s: f32) {
    let dir = (b - a).normalized();
    let perp = vec2(-dir.y, dir.x);
    let head = 6.0 * s;
    let base = b - dir * head;
    painter.line_segment([a, b - dir * head * 0.6], stroke);
    painter.add(Shape::convex_polygon(
        vec![b, base + perp * head * 0.55, base - perp * head * 0.55],
        stroke.color,
        Stroke::NONE,
    ));
}

/// A square icon button. `selected` draws it highlighted (the active tool).
pub fn button(ui: &mut egui::Ui, icon: Icon, selected: bool, enabled: bool, tooltip: &str) -> egui::Response {
    let size = Vec2::splat(30.0);
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 5.0, super::ACCENT.gamma_multiply(0.85));
    } else if enabled && response.hovered() {
        painter.rect_filled(rect, 5.0, Color32::from_white_alpha(28));
    }
    let color = if !enabled {
        Color32::from_white_alpha(70)
    } else if selected {
        Color32::WHITE
    } else {
        super::TEXT
    };
    paint(painter, rect.shrink(6.0), icon, color);
    if tooltip.is_empty() { response } else { response.on_hover_text(tooltip) }
}
