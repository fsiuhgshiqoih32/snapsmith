//! The annotation controls shared by the overlay and the editor: tools, colours, sizes, undo.

use eframe::egui::{self, Color32, Sense, Stroke, StrokeKind, Vec2};

use snapsmith::editor::{PALETTE, Tool};

use super::canvas::{Canvas, SIZE_NAMES};
use super::color32;
use super::icons::{self, Icon};

pub fn separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, 26.0), Sense::hover());
    ui.painter()
        .line_segment([rect.center_top(), rect.center_bottom()], Stroke::new(1.0, Color32::from_white_alpha(40)));
}

/// Tool buttons, colours, sizes, undo and redo, laid out left to right.
pub fn annotation_controls(ui: &mut egui::Ui, canvas: &mut Canvas, select_label: &str) {
    if icons::button(ui, Icon::Select, canvas.tool.is_none(), true, select_label).clicked() {
        canvas.set_tool(None);
    }
    for tool in Tool::ALL {
        let tip = format!("{} ({})", tool.label(), tool.key());
        if icons::button(ui, Icon::Tool(tool), canvas.tool == Some(tool), true, &tip).clicked() {
            canvas.set_tool(if canvas.tool == Some(tool) { None } else { Some(tool) });
        }
    }
    separator(ui);
    let current = canvas.color();
    for c in PALETTE {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(18.0, 30.0), Sense::click());
        let sw = egui::Rect::from_center_size(rect.center(), Vec2::splat(14.0));
        ui.painter().rect_filled(sw, 3.0, color32(c));
        ui.painter().rect_stroke(sw, 3.0, Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Inside);
        if c == current {
            ui.painter().rect_stroke(sw.expand(2.5), 4.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Middle);
        }
        if resp.clicked() {
            canvas.set_color(c);
        }
    }
    separator(ui);
    for (i, name) in SIZE_NAMES.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(24.0, 30.0), Sense::click());
        if canvas.size == i {
            ui.painter().rect_filled(rect.shrink2(Vec2::new(1.0, 3.0)), 5.0, super::ACCENT.gamma_multiply(0.85));
        } else if resp.hovered() {
            ui.painter().rect_filled(rect.shrink2(Vec2::new(1.0, 3.0)), 5.0, Color32::from_white_alpha(28));
        }
        ui.painter().circle_filled(rect.center(), 2.0 + i as f32 * 2.2, super::TEXT);
        if resp.on_hover_text(*name).clicked() {
            canvas.size = i;
        }
    }
    separator(ui);
    if icons::button(ui, Icon::Undo, false, canvas.doc.can_undo(), "Undo (Ctrl+Z)").clicked() {
        canvas.undo();
    }
    if icons::button(ui, Icon::Redo, false, canvas.doc.can_redo(), "Redo (Ctrl+Y)").clicked() {
        canvas.redo();
    }
}

/// Keyboard shortcuts for the annotation tools. Call only when no text field has focus.
pub fn handle_tool_keys(ctx: &egui::Context, canvas: &mut Canvas) {
    let (command, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
    if command {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
            canvas.undo();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y))
            || ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z))
        {
            canvas.redo();
        }
        return;
    }
    if shift {
        return;
    }
    for tool in Tool::ALL {
        if let Some(key) = egui::Key::from_name(&tool.key().to_string())
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key))
        {
            canvas.set_tool(Some(tool));
        }
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::V)) {
        canvas.set_tool(None);
    }
}
