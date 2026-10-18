//! Does Windows still compose a borderless window that covers the primary monitor, or does the
//! driver flip it straight to the screen? Direct flips can switch an HDR or VRR display's mode,
//! which blanks some monitors for a second or two.
//!
//! Opens such a window for two seconds, repainting every frame, and compares its frame count with
//! DWM's composed frames. Composed: DWM counts about as many frames as the app presents. Flipped
//! directly: DWM's count stalls while the app keeps presenting.
//!
//! `cargo run --example flip_probe -- <glow|wgpu> <exact|fullscreen>`
//! - exact: the window's rectangle is the monitor's (what the standby window does).
//! - fullscreen: winit's borderless full screen (what a cold-started overlay does).
//!
//! Measured on an RTX 3060 Ti with an HDR monitor at 240 Hz: OpenGL presented 240 fps while
//! DWM composed 1 (flipped directly, in both fits); wgpu/DX12 241 of 240 (composed).

#[cfg(not(windows))]
fn main() {
    eprintln!("flip_probe is for Windows");
}

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};

    use eframe::egui;
    use windows_sys::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    fn dwm() -> (u64, u64) {
        let mut t: DWM_TIMING_INFO = unsafe { std::mem::zeroed() };
        t.cbSize = size_of::<DWM_TIMING_INFO>() as u32;
        unsafe { DwmGetCompositionTimingInfo(std::ptr::null_mut(), &mut t) };
        (t.cFrame, t.cRefresh)
    }

    struct Probe {
        fit: String,
        start: Instant,
        frames: u64,
        mark: Option<(u64, u64, u64, Instant)>,
        done: bool,
    }

    impl eframe::App for Probe {
        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            let ctx = ui.ctx().clone();
            self.frames += 1;
            if self.frames == 2 {
                if self.fit == "fullscreen"
                    && let Some(w) = frame.winit_window()
                {
                    w.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            let rect = ctx.viewport_rect();
            let p = ui.painter();
            p.rect_filled(rect, 0.0, egui::Color32::from_gray(24));
            // Something changes every frame, so every frame is presented.
            let x = (self.frames % 2) as f32;
            p.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(1.0, 1.0)),
                0.0,
                egui::Color32::from_gray(40),
            );
            let elapsed = self.start.elapsed();
            if elapsed > Duration::from_millis(700) && self.mark.is_none() && !self.done {
                let (f, r) = dwm();
                self.mark = Some((self.frames, f, r, Instant::now()));
            }
            if elapsed > Duration::from_millis(2200)
                && let Some((af, df, dr, t)) = self.mark.take()
            {
                self.done = true;
                let (f, r) = dwm();
                let secs = t.elapsed().as_secs_f64();
                let inner = frame.winit_window().map(|w| w.inner_size()).map(|s| (s.width, s.height));
                println!(
                    " {}: inner {:?}; app {:.0} fps, DWM composed {:.0} fps, refreshes {:.0}/s",
                    self.fit,
                    inner,
                    (self.frames - af) as f64 / secs,
                    (f - df) as f64 / secs,
                    (r - dr) as f64 / secs,
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let renderer = args.first().map(String::as_str).unwrap_or("glow");
    let fit = args.get(1).cloned().unwrap_or_else(|| "exact".into());
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN) as f32, GetSystemMetrics(SM_CYSCREEN) as f32) };
    let viewport = egui::ViewportBuilder::default()
        .with_decorations(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_position(egui::pos2(0.0, 0.0))
        .with_inner_size(egui::vec2(w, h));
    let options = eframe::NativeOptions {
        viewport,
        renderer: if renderer == "wgpu" { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow },
        ..Default::default()
    };
    print!("{renderer}");
    eframe::run_native(
        "flip probe",
        options,
        Box::new(move |_| Ok(Box::new(Probe { fit, start: Instant::now(), frames: 0, mark: None, done: false }))),
    )
    .unwrap();
}
