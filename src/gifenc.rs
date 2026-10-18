//! Streaming GIF encoder for screen recordings. Each frame stores only the rectangle that changed,
//! with unchanged pixels transparent, which keeps screen recordings small. Identical frames are
//! merged into a longer delay.

use std::io::Write;

use image::RgbaImage;

pub struct GifWriter<W: Write> {
    encoder: gif::Encoder<W>,
    width: u16,
    height: u16,
    /// The last frame as shown (full size), to diff against.
    shown: Option<RgbaImage>,
    /// Written once the next different frame arrives, so repeats can extend its delay.
    pending: Option<gif::Frame<'static>>,
    /// Quantiser speed, 1 (best) to 30 (fastest).
    speed: i32,
    pub frames_written: usize,
}

impl<W: Write> GifWriter<W> {
    pub fn new(out: W, width: u32, height: u32) -> Result<Self, String> {
        let (w, h) = (to_u16(width)?, to_u16(height)?);
        let mut encoder = gif::Encoder::new(out, w, h, &[]).map_err(|e| e.to_string())?;
        encoder.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
        Ok(GifWriter { encoder, width: w, height: h, shown: None, pending: None, speed: 10, frames_written: 0 })
    }

    /// Adds a frame shown for `delay_ms`. Frames must all have the writer's size.
    pub fn add(&mut self, frame: &RgbaImage, delay_ms: u32) -> Result<(), String> {
        if frame.width() != self.width as u32 || frame.height() != self.height as u32 {
            return Err("frame size changed during recording".into());
        }
        let delay_cs = ((delay_ms + 5) / 10).max(2);
        let changed = match &self.shown {
            None => Some((0, 0, self.width as u32, self.height as u32)),
            Some(prev) => changed_rect(prev, frame),
        };
        let Some((x, y, w, h)) = changed else {
            if let Some(p) = &mut self.pending {
                p.delay = p.delay.saturating_add(delay_cs as u16);
            }
            return Ok(());
        };
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for yy in y..y + h {
            for xx in x..x + w {
                let p = frame.get_pixel(xx, yy).0;
                let same = self.shown.as_ref().is_some_and(|prev| prev.get_pixel(xx, yy).0 == p);
                if same {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
        }
        let mut f = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut rgba, self.speed);
        f.left = x as u16;
        f.top = y as u16;
        f.delay = delay_cs as u16;
        f.dispose = gif::DisposalMethod::Keep;
        self.flush_pending()?;
        self.pending = Some(f);
        self.shown = Some(frame.clone());
        Ok(())
    }

    fn flush_pending(&mut self) -> Result<(), String> {
        if let Some(f) = self.pending.take() {
            self.encoder.write_frame(&f).map_err(|e| e.to_string())?;
            self.frames_written += 1;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<W, String> {
        self.flush_pending()?;
        self.encoder.into_inner().map_err(|e| e.to_string())
    }
}

fn to_u16(v: u32) -> Result<u16, String> {
    u16::try_from(v).ok().filter(|v| *v > 0).ok_or_else(|| format!("GIF frames must be 1 to 65535 pixels, not {v}"))
}

/// The bounding box of pixels that differ, or `None` if the frames are identical.
pub fn changed_rect(a: &RgbaImage, b: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = a.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    let (ra, rb) = (a.as_raw(), b.as_raw());
    for y in 0..h {
        let row = (y * w * 4) as usize..((y + 1) * w * 4) as usize;
        if ra[row.clone()] == rb[row.clone()] {
            continue;
        }
        y0 = y0.min(y);
        y1 = y;
        let (sa, sb) = (&ra[row.clone()], &rb[row]);
        if let Some(first) = (0..w as usize).find(|&x| sa[x * 4..x * 4 + 4] != sb[x * 4..x * 4 + 4]) {
            x0 = x0.min(first as u32);
        }
        if let Some(last) = (0..w as usize).rev().find(|&x| sa[x * 4..x * 4 + 4] != sb[x * 4..x * 4 + 4]) {
            x1 = x1.max(last as u32);
        }
    }
    (y0 != u32::MAX).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_rect() {
        let a = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255]));
        let mut b = a.clone();
        assert_eq!(changed_rect(&a, &b), None);
        b.put_pixel(2, 3, image::Rgba([1, 0, 0, 255]));
        b.put_pixel(7, 5, image::Rgba([1, 0, 0, 255]));
        assert_eq!(changed_rect(&a, &b), Some((2, 3, 6, 3)));
    }

    #[test]
    fn encodes_and_decodes() {
        let mut frames = Vec::new();
        for i in 0..6u32 {
            let mut img = RgbaImage::from_pixel(32, 24, image::Rgba([240, 240, 240, 255]));
            // A square that moves on even frames; odd frames repeat the previous one.
            let pos = (i / 2) * 6;
            for y in 4..12 {
                for x in pos..pos + 8 {
                    img.put_pixel(x, y, image::Rgba([200, 30, 30, 255]));
                }
            }
            frames.push(img);
        }
        let mut w = GifWriter::new(Vec::new(), 32, 24).unwrap();
        for f in &frames {
            w.add(f, 100).unwrap();
        }
        let bytes = w.finish().unwrap();

        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let mut dec = opts.read_info(std::io::Cursor::new(bytes)).unwrap();
        let mut canvas = RgbaImage::new(32, 24);
        let mut count = 0;
        let mut delays = Vec::new();
        while let Some(f) = dec.read_next_frame().unwrap() {
            for y in 0..f.height as u32 {
                for x in 0..f.width as u32 {
                    let i = ((y * f.width as u32 + x) * 4) as usize;
                    let px = &f.buffer[i..i + 4];
                    if px[3] != 0 {
                        canvas.put_pixel(f.left as u32 + x, f.top as u32 + y, image::Rgba([px[0], px[1], px[2], 255]));
                    }
                }
            }
            count += 1;
            delays.push(f.delay);
        }
        // Six inputs, three distinct pictures, each shown for two frames' worth of time.
        assert_eq!(count, 3);
        assert_eq!(delays, vec![20, 20, 20]);
        let red = canvas.get_pixel(14, 6).0;
        assert!(red[0] > 150 && red[1] < 90, "{red:?}");
        let bg = canvas.get_pixel(2, 20).0;
        assert!(bg[0] > 200 && bg[1] > 200, "{bg:?}");
    }

    #[test]
    fn rejects_size_changes() {
        let mut w = GifWriter::new(Vec::new(), 4, 4).unwrap();
        assert!(w.add(&RgbaImage::new(5, 4), 100).is_err());
        assert!(GifWriter::new(Vec::new(), 0, 4).is_err());
    }
}
