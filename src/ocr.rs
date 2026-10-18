//! Text recognition with ocrs. The two models (about 12 MB) aren't shipped with the app; they're
//! downloaded on first use, after the user agrees, and checked against pinned SHA-256 hashes.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use image::RgbaImage;
use sha2::{Digest, Sha256};

pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

pub const MODELS: [ModelFile; 2] = [
    ModelFile {
        name: "text-detection.rten",
        url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten",
        sha256: "f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca",
        size: 2_510_284,
    },
    ModelFile {
        name: "text-recognition.rten",
        url: "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten",
        sha256: "e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e",
        size: 9_716_568,
    },
];

pub fn total_download_size() -> u64 {
    MODELS.iter().map(|m| m.size).sum()
}

pub fn models_dir() -> PathBuf {
    crate::config::data_dir().join("ocr")
}

/// True if both model files are in place (sizes are checked; hashes were checked on download).
pub fn models_present(dir: &Path) -> bool {
    MODELS.iter().all(|m| std::fs::metadata(dir.join(m.name)).is_ok_and(|md| md.len() == m.size))
}

/// Downloads whichever models are missing. `progress(done, total)` is called as bytes arrive.
pub fn download_models(dir: &Path, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .user_agent(concat!("Snapsmith/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let total = total_download_size();
    let mut done = 0u64;
    for m in &MODELS {
        let dest = dir.join(m.name);
        if std::fs::metadata(&dest).is_ok_and(|md| md.len() == m.size) {
            done += m.size;
            progress(done, total);
            continue;
        }
        let mut resp = agent.get(m.url).call().map_err(|e| format!("could not download {}: {e}", m.name))?;
        let mut reader = resp.body_mut().with_config().limit(m.size + 1024).reader();
        let part = dir.join(format!("{}.part", m.name));
        let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("download of {} failed: {e}", m.name))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            progress(done, total);
        }
        drop(file);
        let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != m.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{} didn't match its expected checksum; not using it", m.name));
        }
        std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub struct Ocr {
    engine: ocrs::OcrEngine,
}

impl Ocr {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let load = |name: &str| {
            rten::Model::load_file(dir.join(name)).map_err(|e| format!("could not load OCR model {name}: {e}"))
        };
        let engine = ocrs::OcrEngine::new(ocrs::OcrEngineParams {
            detection_model: Some(load(MODELS[0].name)?),
            recognition_model: Some(load(MODELS[1].name)?),
            ..Default::default()
        })
        .map_err(|e| format!("could not start OCR: {e}"))?;
        Ok(Ocr { engine })
    }

    /// The text in `img`, one line per text line found.
    pub fn recognize(&self, img: &RgbaImage) -> Result<String, String> {
        let rgb = prepare(img);
        let source = ocrs::ImageSource::from_bytes(rgb.as_raw(), rgb.dimensions()).map_err(|e| e.to_string())?;
        let input = self.engine.prepare_input(source).map_err(|e| e.to_string())?;
        let text = self.engine.get_text(&input).map_err(|e| e.to_string())?;
        Ok(text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string())
    }
}

/// Screen text is small and often light-on-dark; the models do best on larger dark-on-light text.
fn prepare(img: &RgbaImage) -> image::RgbImage {
    let (w, h) = img.dimensions();
    let mut rgb = image::RgbImage::new(w, h);
    let mut luma_sum = 0u64;
    for (x, y, p) in img.enumerate_pixels() {
        // Flatten transparency onto white.
        let a = p[3] as u32;
        let c = |v: u8| ((v as u32 * a + 255 * (255 - a)) / 255) as u8;
        let px = [c(p[0]), c(p[1]), c(p[2])];
        luma_sum += (px[0] as u64 * 299 + px[1] as u64 * 587 + px[2] as u64 * 114) / 1000;
        rgb.put_pixel(x, y, image::Rgb(px));
    }
    let mean = luma_sum / (w as u64 * h as u64).max(1);
    if mean < 110 {
        image::imageops::invert(&mut rgb);
    }
    let scale = if h < 400 || w < 400 { 2 } else { 1 };
    if scale > 1 && (w as u64 * h as u64) < 8_000_000 {
        image::imageops::resize(&rgb, w * scale, h * scale, image::imageops::FilterType::CatmullRom)
    } else {
        rgb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_images_are_inverted_and_small_ones_enlarged() {
        let img = RgbaImage::from_pixel(100, 50, image::Rgba([10, 10, 10, 255]));
        let out = prepare(&img);
        assert_eq!(out.dimensions(), (200, 100));
        assert!(out.get_pixel(0, 0)[0] > 200);
    }

    #[test]
    fn missing_models_are_detected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!models_present(dir.path()));
    }
}
