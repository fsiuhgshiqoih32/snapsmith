//! Rasterises annotations onto an image with tiny-skia (shapes) and ab_glyph (text). This is the
//! one renderer for both the on-screen preview and the exported file, so what you see is what you
//! get.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use image::RgbaImage;
use tiny_skia::{BlendMode, Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use super::{Bounds, Op, P, Rgba, Shape};

/// The annotation font (Ubuntu Light, the same face egui uses, so the text box matches).
pub fn font() -> FontRef<'static> {
    FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("embedded font is valid")
}

/// `base` with `ops` drawn on top, same size.
pub fn compose(base: &RgbaImage, ops: &[Op]) -> RgbaImage {
    if ops.is_empty() || base.width() == 0 || base.height() == 0 {
        return base.clone();
    }
    let mut pm = to_pixmap(base);
    for op in ops {
        draw(&mut pm, op);
    }
    from_pixmap(&pm)
}

/// Composes, then crops to `crop` (x, y, w, h) if given.
pub fn export(base: &RgbaImage, ops: &[Op], crop: Option<(u32, u32, u32, u32)>) -> RgbaImage {
    let full = compose(base, ops);
    match crop {
        Some((x, y, w, h)) => image::imageops::crop_imm(&full, x, y, w, h).to_image(),
        None => full,
    }
}

fn to_pixmap(img: &RgbaImage) -> Pixmap {
    let mut data = img.as_raw().clone();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 255 {
            px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
            px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
            px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
        }
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(img.width(), img.height()).expect("non-empty"))
        .expect("pixmap size matches")
}

fn from_pixmap(pm: &Pixmap) -> RgbaImage {
    let mut data = pm.data().to_vec();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 255 && a != 0 {
            px[0] = ((px[0] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[1] = ((px[1] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[2] = ((px[2] as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
    RgbaImage::from_raw(pm.width(), pm.height(), data).expect("buffer size matches")
}

fn paint(color: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(color[0], color[1], color[2], color[3]));
    p.anti_alias = true;
    p
}

fn stroke(width: f32, join: LineJoin) -> Stroke {
    Stroke { width: width.max(0.5), line_cap: LineCap::Round, line_join: join, ..Default::default() }
}

fn rect_of(a: P, b: P) -> Option<Rect> {
    let r = Bounds::from_points(a, b);
    Rect::from_ltrb(r.x0, r.y0, r.x1, r.y1)
}

/// Relative luminance, 0 (black) to 1 (white).
pub fn luminance(c: Rgba) -> f32 {
    (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0
}

/// Black or white, whichever reads better on `c`.
pub fn contrast(c: Rgba) -> Rgba {
    if luminance(c) > 0.55 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }
}

pub fn draw(pm: &mut Pixmap, op: &Op) {
    let color = op.color;
    let w = op.width;
    match &op.shape {
        Shape::Rect { a, b } => {
            if let Some(r) = rect_of(*a, *b) {
                let path = PathBuilder::from_rect(r);
                pm.stroke_path(&path, &paint(color), &stroke(w, LineJoin::Miter), Transform::identity(), None);
            }
        }
        Shape::Ellipse { a, b } => {
            if let Some(path) = rect_of(*a, *b).and_then(PathBuilder::from_oval) {
                pm.stroke_path(&path, &paint(color), &stroke(w, LineJoin::Round), Transform::identity(), None);
            }
        }
        Shape::Line { a, b } => {
            let mut pb = PathBuilder::new();
            pb.move_to(a.x, a.y);
            pb.line_to(b.x, b.y);
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, &paint(color), &stroke(w, LineJoin::Round), Transform::identity(), None);
            }
        }
        Shape::Arrow { a, b } => draw_arrow(pm, *a, *b, w, color),
        Shape::Pen { points } => draw_pen(pm, points, w, color),
        Shape::Highlight { a, b } => {
            if let Some(r) = rect_of(*a, *b) {
                let mut p = paint([color[0], color[1], color[2], 255]);
                p.blend_mode = BlendMode::Multiply;
                pm.fill_rect(r, &p, Transform::identity(), None);
            }
        }
        Shape::Text { at, text, font_px } => draw_text(pm, *at, text, *font_px, color, true),
        Shape::Step { at, number, radius } => draw_step(pm, *at, *number, *radius, color),
        Shape::Pixelate { a, b, block } => {
            if let Some(px) = Bounds::from_points(*a, *b).to_pixels(pm.width(), pm.height()) {
                pixelate(pm, px, block.round().max(2.0) as u32);
            }
        }
        Shape::Blur { a, b, radius } => {
            if let Some(px) = Bounds::from_points(*a, *b).to_pixels(pm.width(), pm.height()) {
                blur(pm, px, radius.round().max(1.0) as u32);
            }
        }
    }
}

/// Head length and half-width for an arrow of stroke `width` and length `len`.
pub fn arrow_head(width: f32, len: f32) -> (f32, f32) {
    let head = (width * 3.0 + 9.0).min(len * 0.6);
    (head, head * 0.5)
}

fn draw_arrow(pm: &mut Pixmap, a: P, b: P, width: f32, color: Rgba) {
    let len = a.dist(b);
    if len < 1.0 {
        return;
    }
    let (dx, dy) = ((b.x - a.x) / len, (b.y - a.y) / len);
    let (head, half) = arrow_head(width, len);
    let base = P::new(b.x - dx * head, b.y - dy * head);
    let shaft_end = P::new(b.x - dx * head * 0.7, b.y - dy * head * 0.7);
    let mut pb = PathBuilder::new();
    pb.move_to(a.x, a.y);
    pb.line_to(shaft_end.x, shaft_end.y);
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint(color), &stroke(width, LineJoin::Round), Transform::identity(), None);
    }
    let (px, py) = (-dy, dx);
    let mut pb = PathBuilder::new();
    pb.move_to(b.x, b.y);
    pb.line_to(base.x + px * half, base.y + py * half);
    pb.line_to(base.x - px * half, base.y - py * half);
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
    }
}

fn draw_pen(pm: &mut Pixmap, points: &[P], width: f32, color: Rgba) {
    if points.is_empty() {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].x, points[0].y);
    if points.len() == 1 {
        pb.line_to(points[0].x + 0.01, points[0].y);
    } else {
        // Quadratic curves through the midpoints smooth out mouse jitter.
        for i in 1..points.len() - 1 {
            let (p, n) = (points[i], points[i + 1]);
            pb.quad_to(p.x, p.y, (p.x + n.x) / 2.0, (p.y + n.y) / 2.0);
        }
        let last = points[points.len() - 1];
        pb.line_to(last.x, last.y);
    }
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint(color), &stroke(width, LineJoin::Round), Transform::identity(), None);
    }
}

fn draw_step(pm: &mut Pixmap, at: P, number: u32, radius: f32, color: Rgba) {
    let r = radius.max(4.0);
    if let Some(path) = PathBuilder::from_circle(at.x, at.y, r) {
        pm.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
        let ring = stroke((r * 0.12).max(1.0), LineJoin::Round);
        pm.stroke_path(&path, &paint([255, 255, 255, 230]), &ring, Transform::identity(), None);
    }
    let label = number.to_string();
    let font_px = r * 1.15;
    let (tw, th) = text_size(&label, font_px);
    draw_text(pm, P::new(at.x - tw / 2.0, at.y - th / 2.0), &label, font_px, contrast(color), false);
}

/// Width and height of `text` laid out at `font_px`, lines split on `\n`.
pub fn text_size(text: &str, font_px: f32) -> (f32, f32) {
    let font = font();
    let scaled = font.as_scaled(PxScale::from(font_px));
    let line_h = scaled.height() + scaled.line_gap();
    let mut max_w: f32 = 0.0;
    let mut lines = 0;
    for line in text.split('\n') {
        lines += 1;
        let mut x = 0.0;
        let mut prev = None;
        for c in line.chars() {
            let id = font.glyph_id(c);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            x += scaled.h_advance(id);
            prev = Some(id);
        }
        max_w = max_w.max(x);
    }
    (max_w, scaled.height() + line_h * (lines.max(1) - 1) as f32)
}

/// Draws text with its top-left at `at`. Ubuntu Light is thin, so the glyphs are thickened a
/// little; `outline` adds a contrasting halo so text stays readable on busy screenshots.
fn draw_text(pm: &mut Pixmap, at: P, text: &str, font_px: f32, color: Rgba, outline: bool) {
    if text.is_empty() || font_px < 2.0 {
        return;
    }
    let font = font();
    let scaled = font.as_scaled(PxScale::from(font_px));
    let line_h = scaled.height() + scaled.line_gap();
    let bold = (font_px / 22.0).max(0.4);
    let halo = if outline { (font_px / 15.0).max(1.2) } else { 0.0 };
    let pad = (bold + halo).ceil() as i32 + 2;

    let (tw, th) = text_size(text, font_px);
    let x0 = at.x.floor() as i32 - pad;
    let y0 = at.y.floor() as i32 - pad;
    let mw = tw.ceil() as i32 + 2 * pad + 2;
    let mh = th.ceil() as i32 + 2 * pad + 2;
    if mw <= 0 || mh <= 0 || mw as i64 * mh as i64 > 64_000_000 {
        return;
    }
    let (mw, mh) = (mw as usize, mh as usize);
    let mut cov = vec![0f32; mw * mh];
    let ox = at.x - x0 as f32;
    let oy = at.y - y0 as f32;
    for (li, line) in text.split('\n').enumerate() {
        let baseline = oy + scaled.ascent() + line_h * li as f32;
        let mut x = ox;
        let mut prev = None;
        for c in line.chars() {
            let id = font.glyph_id(c);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            let glyph = id.with_scale_and_position(PxScale::from(font_px), point(x, baseline));
            x += scaled.h_advance(id);
            prev = Some(id);
            if let Some(outlined) = font.outline_glyph(glyph) {
                let b = outlined.px_bounds();
                outlined.draw(|gx, gy, c| {
                    let px = b.min.x as i32 + gx as i32;
                    let py = b.min.y as i32 + gy as i32;
                    if px >= 0 && py >= 0 && (px as usize) < mw && (py as usize) < mh {
                        let i = py as usize * mw + px as usize;
                        cov[i] = (cov[i] + c).min(1.0);
                    }
                });
            }
        }
    }
    let body = dilate(&cov, mw, mh, bold);
    if outline {
        let ring = dilate(&body, mw, mh, halo);
        let oc = contrast(color);
        blend_mask(pm, &ring, mw, mh, x0, y0, [oc[0], oc[1], oc[2], 180]);
    }
    blend_mask(pm, &body, mw, mh, x0, y0, color);
}

/// Grows a coverage mask by `r` pixels (a soft max over a disc).
fn dilate(mask: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    if r < 0.25 {
        return mask.to_vec();
    }
    let ri = r.ceil() as i32;
    let offsets: Vec<(i32, i32, f32)> = (-ri..=ri)
        .flat_map(|dy| (-ri..=ri).map(move |dx| (dx, dy)))
        .filter_map(|(dx, dy)| {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            let weight = (r + 0.5 - d).clamp(0.0, 1.0);
            (weight > 0.0).then_some((dx, dy, weight))
        })
        .collect();
    let mut out = vec![0f32; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut m: f32 = 0.0;
            for &(dx, dy, wgt) in &offsets {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                    m = m.max(mask[sy as usize * w + sx as usize] * wgt);
                }
            }
            out[y as usize * w + x as usize] = m;
        }
    }
    out
}

/// Source-over of `color` through a coverage mask placed at (x0, y0).
fn blend_mask(pm: &mut Pixmap, mask: &[f32], mw: usize, mh: usize, x0: i32, y0: i32, color: Rgba) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for my in 0..mh as i32 {
        let py = y0 + my;
        if py < 0 || py >= ph {
            continue;
        }
        for mx in 0..mw as i32 {
            let px = x0 + mx;
            if px < 0 || px >= pw {
                continue;
            }
            let c = mask[my as usize * mw + mx as usize];
            if c <= 0.0 {
                continue;
            }
            let sa = c * color[3] as f32 / 255.0;
            let i = (py as usize * pw as usize + px as usize) * 4;
            for k in 0..3 {
                let s = color[k] as f32 * sa;
                data[i + k] = (s + data[i + k] as f32 * (1.0 - sa)).round().clamp(0.0, 255.0) as u8;
            }
            data[i + 3] = (sa * 255.0 + data[i + 3] as f32 * (1.0 - sa)).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn pixelate(pm: &mut Pixmap, (x, y, w, h): (u32, u32, u32, u32), block: u32) {
    let stride = pm.width() as usize * 4;
    let data = pm.data_mut();
    let mut by = y;
    while by < y + h {
        let bh = block.min(y + h - by);
        let mut bx = x;
        while bx < x + w {
            let bw = block.min(x + w - bx);
            let mut sum = [0u64; 4];
            for yy in by..by + bh {
                for xx in bx..bx + bw {
                    let i = yy as usize * stride + xx as usize * 4;
                    for k in 0..4 {
                        sum[k] += data[i + k] as u64;
                    }
                }
            }
            let n = (bw * bh) as u64;
            let avg = sum.map(|s| ((s + n / 2) / n) as u8);
            for yy in by..by + bh {
                for xx in bx..bx + bw {
                    let i = yy as usize * stride + xx as usize * 4;
                    data[i..i + 4].copy_from_slice(&avg);
                }
            }
            bx += bw;
        }
        by += bh;
    }
}

/// Three box-blur passes (close to a Gaussian) inside the rectangle.
fn blur(pm: &mut Pixmap, (x, y, w, h): (u32, u32, u32, u32), radius: u32) {
    let stride = pm.width() as usize;
    let (w, h) = (w as usize, h as usize);
    let mut buf: Vec<[f32; 4]> = Vec::with_capacity(w * h);
    {
        let data = pm.data();
        for yy in 0..h {
            for xx in 0..w {
                let i = ((y as usize + yy) * stride + x as usize + xx) * 4;
                buf.push([data[i] as f32, data[i + 1] as f32, data[i + 2] as f32, data[i + 3] as f32]);
            }
        }
    }
    let r = radius as usize;
    for _ in 0..3 {
        box_pass(&mut buf, w, h, r, true);
        box_pass(&mut buf, w, h, r, false);
    }
    let data = pm.data_mut();
    for yy in 0..h {
        for xx in 0..w {
            let i = ((y as usize + yy) * stride + x as usize + xx) * 4;
            let v = buf[yy * w + xx];
            for k in 0..4 {
                data[i + k] = v[k].round().clamp(0.0, 255.0) as u8;
            }
            // Keep premultiplied colour channels within alpha.
            for k in 0..3 {
                data[i + k] = data[i + k].min(data[i + 3]);
            }
        }
    }
}

fn box_pass(buf: &mut [[f32; 4]], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, pos: usize| if horizontal { line * w + pos } else { pos * w + line };
    let mut tmp = vec![[0f32; 4]; len];
    for line in 0..lines {
        // Running sum over a window clamped at the edges.
        let mut acc = [0f32; 4];
        let at = |p: isize| buf[idx(line, p.clamp(0, len as isize - 1) as usize)];
        for p in -(r as isize)..=(r as isize) {
            let v = at(p);
            for k in 0..4 {
                acc[k] += v[k];
            }
        }
        let n = (2 * r + 1) as f32;
        for (pos, out) in tmp.iter_mut().enumerate() {
            *out = acc.map(|a| a / n);
            let add = at(pos as isize + r as isize + 1);
            let sub = at(pos as isize - r as isize);
            for k in 0..4 {
                acc[k] += add[k] - sub[k];
            }
        }
        for (pos, v) in tmp.iter().enumerate() {
            buf[idx(line, pos)] = *v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Op, PALETTE};

    fn white(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
    }

    fn op(shape: Shape) -> Op {
        Op { shape, color: [255, 0, 0, 255], width: 4.0 }
    }

    #[test]
    fn no_ops_is_identity() {
        let img = white(8, 8);
        assert_eq!(compose(&img, &[]), img);
    }

    #[test]
    fn rectangle_lands_where_drawn() {
        let img = white(100, 100);
        let out = compose(&img, &[op(Shape::Rect { a: P::new(20.0, 20.0), b: P::new(80.0, 80.0) })]);
        assert_eq!(out.get_pixel(20, 50).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(50, 20).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(50, 50).0, [255, 255, 255, 255], "stroke only, not filled");
        assert_eq!(out.get_pixel(5, 5).0, [255, 255, 255, 255]);
    }

    #[test]
    fn arrow_has_a_head() {
        let img = white(200, 60);
        let out = compose(&img, &[op(Shape::Arrow { a: P::new(10.0, 30.0), b: P::new(190.0, 30.0) })]);
        // Next to the tip the head is wider than the shaft.
        assert_eq!(out.get_pixel(175, 34).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(60, 36).0, [255, 255, 255, 255]);
        assert_eq!(out.get_pixel(60, 30).0, [255, 0, 0, 255]);
    }

    #[test]
    fn highlighter_multiplies() {
        let mut img = white(20, 20);
        img.put_pixel(5, 5, image::Rgba([0, 0, 0, 255]));
        let mut o = op(Shape::Highlight { a: P::new(0.0, 0.0), b: P::new(20.0, 20.0) });
        o.color = [255, 255, 0, 255];
        let out = compose(&img, &[o]);
        assert_eq!(out.get_pixel(10, 10).0, [255, 255, 0, 255]);
        assert_eq!(out.get_pixel(5, 5).0, [0, 0, 0, 255], "dark text stays dark");
    }

    #[test]
    fn pixelate_flattens_blocks() {
        let mut img = white(20, 20);
        for y in 0..20 {
            for x in 0..20 {
                if (x + y) % 2 == 0 {
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }
        let out = compose(&img, &[op(Shape::Pixelate { a: P::new(0.0, 0.0), b: P::new(10.0, 10.0), block: 5.0 })]);
        let p = out.get_pixel(1, 1).0;
        assert!(p[0] > 100 && p[0] < 155, "averaged to grey, got {p:?}");
        assert_eq!(out.get_pixel(1, 1), out.get_pixel(3, 4));
        assert_eq!(out.get_pixel(15, 15), img.get_pixel(15, 15), "outside untouched");
    }

    #[test]
    fn blur_softens_an_edge() {
        let mut img = white(40, 10);
        for y in 0..10 {
            for x in 20..40 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let out = compose(&img, &[op(Shape::Blur { a: P::new(0.0, 0.0), b: P::new(40.0, 10.0), radius: 3.0 })]);
        let edge = out.get_pixel(20, 5).0[0];
        assert!(edge > 40 && edge < 215, "edge pixel {edge}");
        assert_eq!(out.get_pixel(20, 5).0[3], 255);
    }

    #[test]
    fn text_and_steps_draw_something() {
        let img = white(200, 80);
        let ops = [
            Op {
                shape: Shape::Text { at: P::new(10.0, 10.0), text: "Hi\nthere".into(), font_px: 24.0 },
                color: [0, 0, 0, 255],
                width: 2.0,
            },
            Op {
                shape: Shape::Step { at: P::new(160.0, 40.0), number: 3, radius: 16.0 },
                color: PALETTE[4],
                width: 2.0,
            },
        ];
        let out = compose(&img, &ops);
        let dark_in = |x0: u32, y0: u32, x1: u32, y1: u32| {
            (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| out.get_pixel(x, y).0[0] < 80).count()
        };
        assert!(dark_in(10, 10, 60, 70) > 20, "text pixels drawn");
        assert_eq!(out.get_pixel(160 - 12, 40).0, [PALETTE[4][0], PALETTE[4][1], PALETTE[4][2], 255]);
        let (w, h) = text_size("Hi\nthere", 24.0);
        assert!(w > 30.0 && w < 90.0 && h > 40.0 && h < 70.0, "{w} x {h}");
    }

    #[test]
    fn export_crops() {
        let img = white(50, 40);
        let out = export(&img, &[], Some((10, 5, 20, 15)));
        assert_eq!(out.dimensions(), (20, 15));
    }

    #[test]
    fn translucent_images_survive() {
        let img = RgbaImage::from_pixel(10, 10, image::Rgba([200, 100, 50, 128]));
        let out = compose(&img, &[op(Shape::Line { a: P::new(0.0, 0.0), b: P::new(1.0, 0.0) })]);
        let p = out.get_pixel(8, 8).0;
        assert!((p[0] as i32 - 200).abs() <= 2 && (p[1] as i32 - 100).abs() <= 2 && p[3] == 128, "{p:?}");
    }
}
