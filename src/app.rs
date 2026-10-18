//! Small things every window and command needs.

use std::path::Path;
use std::process::{Command, Stdio};

use eframe::egui;
use image::RgbaImage;

use snapsmith::config::ImageFormat;

/// What a window is, which decides its macOS Dock icon and its renderer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A regular window with a Dock icon (main window, editor).
    Normal,
    /// No Dock icon: notifications, pins, the text and recording windows.
    Accessory,
    /// Covers a whole monitor (the selection and its standby); no Dock icon.
    FullScreen,
}

/// eframe options for one of our windows.
fn native_options(viewport: egui::ViewportBuilder, kind: Kind) -> eframe::NativeOptions {
    let accessory = kind != Kind::Normal;
    eframe::NativeOptions {
        viewport: viewport.with_icon(crate::ui::app_icon()),
        centered: false,
        wgpu_options: fast_wgpu(),
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

/// Window start-up time is mostly GPU set-up. By default wgpu probes Vulkan, DX12 and OpenGL
/// drivers and asks for the high-performance GPU (waking a discrete card on laptops); our
/// windows are simple, so use each system's native API and the power-saving GPU.
/// `WGPU_BACKEND` and `WGPU_POWER_PREF` still override this.
fn fast_wgpu() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup};
    use eframe::wgpu;
    let mut config = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        if wgpu::Backends::from_env().is_none() {
            setup.instance_descriptor.backends = if cfg!(windows) {
                wgpu::Backends::DX12
            } else if cfg!(target_os = "macos") {
                wgpu::Backends::METAL
            } else {
                wgpu::Backends::VULKAN | wgpu::Backends::GL
            };
        }
        if wgpu::PowerPreference::from_env().is_none() {
            setup.power_preference = wgpu::PowerPreference::LowPower;
        }
    }
    config
}

/// Renderers to try, fastest start-up first. On Windows and Linux OpenGL starts in a fraction of
/// the time DX12/Vulkan takes (drivers cache compiled shaders; wgpu recompiles every launch);
/// wgpu is the fallback for machines without a usable OpenGL (VMs, remote desktops). macOS uses
/// Metal through wgpu. `SNAPSMITH_RENDERER=glow|wgpu` overrides.
///
/// Full-screen windows on Windows use DX12: GPU drivers may present an OpenGL window that covers
/// a monitor straight to the screen, bypassing the compositor (NVIDIA's does), and on an HDR or
/// variable-refresh display that switches the monitor's mode, which blanks it for a second or
/// two. DX12 windows stay composed. Its slower start is paid ahead of time by the standby window.
fn renderers(kind: Kind) -> Vec<eframe::Renderer> {
    use eframe::Renderer::{Glow, Wgpu};
    match std::env::var("SNAPSMITH_RENDERER").unwrap_or_default().to_ascii_lowercase().as_str() {
        "wgpu" => vec![Wgpu],
        "glow" => vec![Glow, Wgpu],
        _ if cfg!(target_os = "macos") => vec![Wgpu, Glow],
        _ if cfg!(windows) && kind == Kind::FullScreen => vec![Wgpu, Glow],
        _ => vec![Glow, Wgpu],
    }
}

/// Opens a window and runs it until it closes, trying each renderer in turn if one fails to
/// start. `make` builds the app once the window and renderer exist; it is called again only if
/// the first renderer failed before the app was built.
pub fn run_window<A: eframe::App + 'static>(
    title: &str,
    viewport: egui::ViewportBuilder,
    kind: Kind,
    mut make: impl FnMut(&eframe::CreationContext<'_>) -> A,
) -> Result<(), String> {
    run_window_try(title, viewport, kind, |cc| Ok(make(cc)))
}

/// [`run_window`] where building the app can fail; the error is returned.
pub fn run_window_try<A: eframe::App + 'static>(
    title: &str,
    viewport: egui::ViewportBuilder,
    kind: Kind,
    mut make: impl FnMut(&eframe::CreationContext<'_>) -> Result<A, String>,
) -> Result<(), String> {
    let mut last = String::from("no renderer available");
    for renderer in renderers(kind) {
        let mut options = native_options(viewport.clone(), kind);
        options.renderer = renderer;
        let created = std::cell::Cell::new(false);
        let result = eframe::run_native(
            title,
            options,
            Box::new(|cc| {
                created.set(true);
                crate::log::timing("window and renderer created");
                crate::ui::apply_theme(&cc.egui_ctx);
                match make(cc) {
                    Ok(app) => Ok(Box::new(app) as Box<dyn eframe::App>),
                    Err(e) => Err(e.into()),
                }
            }),
        );
        match result {
            Ok(()) => return Ok(()),
            Err(e) if !created.get() => {
                crate::log::warning!("{renderer} renderer unavailable ({e}); trying the next one");
                last = e.to_string();
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(last)
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
