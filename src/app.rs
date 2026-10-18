//! Small things every window and command needs.

use std::path::Path;
use std::process::{Command, Stdio};

use eframe::egui;
use image::RgbaImage;

use snapsmith::config::ImageFormat;

/// eframe options for one of our windows. `accessory` windows (overlay, notifications, pins)
/// don't put an icon in the macOS Dock.
pub fn native_options_ext(viewport: egui::ViewportBuilder, accessory: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: viewport.with_icon(crate::ui::app_icon()),
        centered: false,
        event_loop_builder: Some(Box::new(move |builder| {
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
                builder.with_activation_policy(if accessory {
                    ActivationPolicy::Accessory
                } else {
                    ActivationPolicy::Regular
                });
            }
            let _ = (builder, accessory);
        })),
        ..Default::default()
    }
}

pub fn native_options(viewport: egui::ViewportBuilder) -> eframe::NativeOptions {
    native_options_ext(viewport, true)
}

pub fn save_color_image(img: &egui::ColorImage, path: &Path) -> Result<(), String> {
    let [w, h] = img.size;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for c in &img.pixels {
        rgba.extend_from_slice(&c.to_srgba_unmultiplied());
    }
    let buf = RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("bad screenshot size")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    buf.save(path).map_err(|e| e.to_string())
}

/// For checking windows without a user: with `SNAPSMITH_SHOT=<folder>` set, a window saves a
/// picture of itself as `<folder>/<name>.png` after a few frames and closes.
pub fn shot_hook(ctx: &egui::Context, frame_no: u64, name: &str) {
    if frame_no != 20 {
        if frame_no < 20 && std::env::var_os("SNAPSMITH_SHOT").is_some() {
            ctx.request_repaint();
        }
        return;
    }
    let Some(path) =
        std::env::var_os("SNAPSMITH_SHOT").map(|d| std::path::PathBuf::from(d).join(format!("{name}.png")))
    else {
        return;
    };
    let c = ctx.clone();
    ctx.request_screenshot(move |img| {
        if let Err(e) = save_color_image(&img, &path) {
            eprintln!("could not save screenshot: {e}");
        }
        c.send_viewport_cmd(egui::ViewportCommand::Close);
        c.request_repaint();
    });
}

/// Encodes for saving or uploading.
pub fn encode(img: &RgbaImage, format: ImageFormat, jpeg_quality: u8) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    match format {
        ImageFormat::Png => {
            let enc = image::codecs::png::PngEncoder::new_with_quality(
                &mut out,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Adaptive,
            );
            image::ImageEncoder::write_image(
                enc,
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| e.to_string())?;
        }
        ImageFormat::Jpg => {
            let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, jpeg_quality);
            image::ImageEncoder::write_image(
                enc,
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(out.into_inner())
}

/// Starts another copy of this program, detached, e.g. `snapsmith pin file.png`.
pub fn spawn_self(args: &[&str]) -> Result<std::process::Child, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    detach(&mut cmd);
    cmd.spawn().map_err(|e| format!("could not start snapsmith {}: {e}", args.join(" ")))
}

/// Keeps a child alive when the terminal that started us closes.
pub fn detach(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

/// Opens a file, folder or link with the system's default program.
pub fn open(target: &str) {
    if let Err(e) = opener::open(target) {
        crate::log::warning!("could not open {target}: {e}");
    }
}

/// Shows a file selected in its folder.
pub fn reveal(path: &Path) {
    if let Err(e) = opener::reveal(path) {
        crate::log::warning!("could not show {}: {e}", path.display());
        if let Some(dir) = path.parent() {
            open(&dir.to_string_lossy());
        }
    }
}

/// A short temporary file path that's unique to this process.
pub fn temp_file(stem: &str, ext: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("snapsmith");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{stem}-{}-{:08x}.{ext}", std::process::id(), fastrand::u32(..)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_both_formats() {
        let img = RgbaImage::from_pixel(8, 6, image::Rgba([10, 200, 30, 255]));
        let png = encode(&img, ImageFormat::Png, 90).unwrap();
        assert_eq!(&png[..4], b"\x89PNG");
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8(), img);
        let jpg = encode(&img, ImageFormat::Jpg, 90).unwrap();
        assert_eq!(&jpg[..2], b"\xFF\xD8");
    }
}
