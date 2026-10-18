//! Annotations: the shapes a user draws on a capture, kept as a list so they can be undone, and
//! rendered onto the pixels only on export (see [`render`]).

pub mod render;

pub type Rgba = [u8; 4];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct P {
    pub x: f32,
    pub y: f32,
}

impl P {
    pub const fn new(x: f32, y: f32) -> Self {
        P { x, y }
    }

    pub fn dist(self, o: P) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// An axis-aligned rectangle in image pixels, `x0 <= x1`, `y0 <= y1`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Bounds {
    pub fn from_points(a: P, b: P) -> Self {
        Bounds { x0: a.x.min(b.x), y0: a.y.min(b.y), x1: a.x.max(b.x), y1: a.y.max(b.y) }
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn is_empty(&self) -> bool {
        self.width() < 1.0 || self.height() < 1.0
    }

    /// Whole pixels, clamped to a `w` x `h` image: (x, y, width, height).
    pub fn to_pixels(&self, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        let x0 = self.x0.round().clamp(0.0, w as f32) as u32;
        let y0 = self.y0.round().clamp(0.0, h as f32) as u32;
        let x1 = self.x1.round().clamp(0.0, w as f32) as u32;
        let y1 = self.y1.round().clamp(0.0, h as f32) as u32;
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Rect,
    Ellipse,
    Arrow,
    Line,
    Pen,
    Highlight,
    Text,
    Step,
    Pixelate,
    Blur,
}

impl Tool {
    pub const ALL: [Tool; 10] = [
        Tool::Rect,
        Tool::Ellipse,
        Tool::Arrow,
        Tool::Line,
        Tool::Pen,
        Tool::Highlight,
        Tool::Text,
        Tool::Step,
        Tool::Pixelate,
        Tool::Blur,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Arrow => "Arrow",
            Tool::Line => "Line",
            Tool::Pen => "Pen",
            Tool::Highlight => "Highlighter",
            Tool::Text => "Text",
            Tool::Step => "Step number",
            Tool::Pixelate => "Pixelate",
            Tool::Blur => "Blur",
        }
    }

    /// Single-key shortcut, as in ShareX's editor where they overlap.
    pub fn key(self) -> char {
        match self {
            Tool::Rect => 'R',
            Tool::Ellipse => 'E',
            Tool::Arrow => 'A',
            Tool::Line => 'L',
            Tool::Pen => 'P',
            Tool::Highlight => 'H',
            Tool::Text => 'T',
            Tool::Step => 'N',
            Tool::Pixelate => 'X',
            Tool::Blur => 'B',
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect {
        a: P,
        b: P,
    },
    Ellipse {
        a: P,
        b: P,
    },
    Line {
        a: P,
        b: P,
    },
    Arrow {
        a: P,
        b: P,
    },
    Pen {
        points: Vec<P>,
    },
    Highlight {
        a: P,
        b: P,
    },
    /// `at` is the top-left of the first line.
    Text {
        at: P,
        text: String,
        font_px: f32,
    },
    /// `at` is the centre.
    Step {
        at: P,
        number: u32,
        radius: f32,
    },
    Pixelate {
        a: P,
        b: P,
        block: f32,
    },
    Blur {
        a: P,
        b: P,
        radius: f32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub shape: Shape,
    pub color: Rgba,
    /// Stroke width in image pixels.
    pub width: f32,
}

impl Op {
    /// Too small to matter (a click with a drag tool).
    pub fn is_degenerate(&self) -> bool {
        match &self.shape {
            Shape::Rect { a, b }
            | Shape::Ellipse { a, b }
            | Shape::Highlight { a, b }
            | Shape::Pixelate { a, b, .. }
            | Shape::Blur { a, b, .. } => Bounds::from_points(*a, *b).is_empty(),
            Shape::Line { a, b } | Shape::Arrow { a, b } => a.dist(*b) < 2.0,
            Shape::Pen { points } => points.len() < 2,
            Shape::Text { text, .. } => text.trim().is_empty(),
            Shape::Step { .. } => false,
        }
    }
}

/// The annotations on one image, with undo and redo.
#[derive(Clone, Debug, Default)]
pub struct Document {
    ops: Vec<Op>,
    undo: Vec<Vec<Op>>,
    redo: Vec<Vec<Op>>,
}

impl Document {
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn push(&mut self, op: Op) {
        if op.is_degenerate() {
            return;
        }
        self.undo.push(self.ops.clone());
        self.redo.clear();
        self.ops.push(op);
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.ops, prev));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(&mut self.ops, next));
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The number the next step badge gets.
    pub fn next_step(&self) -> u32 {
        self.ops
            .iter()
            .filter_map(|o| if let Shape::Step { number, .. } = o.shape { Some(number) } else { None })
            .max()
            .unwrap_or(0)
            + 1
    }
}

/// The colour palette offered in the toolbar.
pub const PALETTE: [Rgba; 8] = [
    [0xE5, 0x39, 0x35, 0xFF],
    [0xFB, 0x8C, 0x00, 0xFF],
    [0xFD, 0xD8, 0x35, 0xFF],
    [0x43, 0xA0, 0x47, 0xFF],
    [0x1E, 0x88, 0xE5, 0xFF],
    [0x8E, 0x24, 0xAA, 0xFF],
    [0xFF, 0xFF, 0xFF, 0xFF],
    [0x21, 0x21, 0x21, 0xFF],
];

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32) -> Op {
        Op { shape: Shape::Rect { a: P::new(0.0, 0.0), b: P::new(x, x) }, color: PALETTE[0], width: 2.0 }
    }

    #[test]
    fn undo_redo() {
        let mut d = Document::default();
        d.push(rect(10.0));
        d.push(rect(20.0));
        assert_eq!(d.ops().len(), 2);
        assert!(d.undo());
        assert_eq!(d.ops(), &[rect(10.0)]);
        assert!(d.redo());
        assert_eq!(d.ops().len(), 2);
        assert!(!d.redo());
        d.undo();
        d.push(rect(30.0));
        assert!(!d.can_redo());
        assert_eq!(d.ops(), &[rect(10.0), rect(30.0)]);
    }

    #[test]
    fn degenerate_ops_are_dropped() {
        let mut d = Document::default();
        d.push(rect(0.2));
        d.push(Op {
            shape: Shape::Text { at: P::default(), text: "  ".into(), font_px: 20.0 },
            color: PALETTE[0],
            width: 1.0,
        });
        assert!(d.ops().is_empty());
        assert!(!d.can_undo());
    }

    #[test]
    fn step_numbers_count_up() {
        let mut d = Document::default();
        assert_eq!(d.next_step(), 1);
        d.push(Op { shape: Shape::Step { at: P::default(), number: 1, radius: 12.0 }, color: PALETTE[0], width: 1.0 });
        d.push(Op { shape: Shape::Step { at: P::default(), number: 2, radius: 12.0 }, color: PALETTE[0], width: 1.0 });
        assert_eq!(d.next_step(), 3);
    }

    #[test]
    fn bounds_to_pixels_clamps() {
        let b = Bounds::from_points(P::new(-5.0, 10.4), P::new(50.6, 2.0));
        assert_eq!(b.to_pixels(40, 40), Some((0, 2, 40, 8)));
        assert_eq!(Bounds::from_points(P::new(50.0, 50.0), P::new(60.0, 60.0)).to_pixels(40, 40), None);
    }
}
