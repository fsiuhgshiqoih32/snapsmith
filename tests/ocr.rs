//! Runs real OCR on text drawn by the annotation renderer. Needs the model files, so it only runs
//! when SNAPSMITH_OCR_DIR points at a folder holding them (the app's own copy works:
//! `<data dir>/snapsmith/ocr`).

use image::RgbaImage;
use snapsmith::editor::{Op, P, Shape, render};
use snapsmith::ocr::{self, Ocr};

#[test]
fn reads_rendered_text() {
    let Some(dir) = std::env::var_os("SNAPSMITH_OCR_DIR") else {
        eprintln!("SNAPSMITH_OCR_DIR not set; skipping");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    assert!(ocr::models_present(&dir), "models missing in {}", dir.display());
    let engine = Ocr::load(&dir).unwrap();

    let base = RgbaImage::from_pixel(520, 120, image::Rgba([255, 255, 255, 255]));
    let op = Op {
        shape: Shape::Text {
            at: P::new(16.0, 20.0),
            text: "Hello from Snapsmith\nInvoice 2026-10".into(),
            font_px: 30.0,
        },
        color: [20, 20, 20, 255],
        width: 2.0,
    };
    let img = render::compose(&base, &[op]);
    let text = engine.recognize(&img).unwrap();
    eprintln!("recognised: {text:?}");
    let lower = text.to_lowercase();
    assert!(lower.contains("hello"), "{text:?}");
    assert!(lower.contains("snapsmith"), "{text:?}");
    assert!(text.contains("2026"), "{text:?}");

    // Light text on a dark background (a terminal, say) is inverted before recognition.
    let dark = RgbaImage::from_pixel(420, 70, image::Rgba([30, 30, 34, 255]));
    let op = Op {
        shape: Shape::Text { at: P::new(12.0, 14.0), text: "cargo build --release".into(), font_px: 26.0 },
        color: [230, 230, 230, 255],
        width: 2.0,
    };
    let img = render::compose(&dark, &[op]);
    let text = engine.recognize(&img).unwrap();
    eprintln!("recognised: {text:?}");
    assert!(text.to_lowercase().contains("cargo"), "{text:?}");
}
