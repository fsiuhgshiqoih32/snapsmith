//! A warm selection window, for an instant start.
//!
//! Opening a window and its renderer from scratch takes about a quarter of a second. So the
//! background process (and the main window) keep one `snapsmith standby` process waiting with
//! its window already created and hidden. A shortcut sends it one line on stdin, such as
//! `record --gif`; it then grabs the screen, paints the selection while still hidden, shows it,
//! and carries on exactly like a freshly started `snapsmith record --gif`. A new standby is
//! started a moment later for next time.
//!
//! On by default on Windows. Elsewhere `SNAPSMITH_STANDBY=1` turns it on (`=0` turns it off);
//! never on Wayland, where shortcuts come from the desktop and windows can't hide themselves.

use std::io::{BufRead, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2};

use crate::commands::{self, AreaAction, CaptureOptions, Prepared};
use crate::log::warning;
use crate::overlay::{Overlay, Picked};
use crate::{capture, platform};

pub fn enabled() -> bool {
    if platform::is_wayland() {
        return false;
    }
    match std::env::var("SNAPSMITH_STANDBY").unwrap_or_default().to_ascii_lowercase().as_str() {
        "1" | "on" | "yes" | "true" => true,
        "0" | "off" | "no" | "false" => false,
        _ => cfg!(windows),
    }
}

/// Keeps one standby process ready and hands it work.
pub struct Pool {
    ready: Option<Child>,
    respawn_at: Option<Instant>,
}

impl Pool {
    pub fn new() -> Self {
        Pool { ready: None, respawn_at: enabled().then(Instant::now) }
    }

    /// Call every so often: starts a standby when one is due.
    pub fn tick(&mut self) {
        if let Some(c) = &mut self.ready
            && !matches!(c.try_wait(), Ok(None))
        {
            self.ready = None;
            self.respawn_at.get_or_insert(Instant::now() + Duration::from_secs(2));
        }
        if self.ready.is_none() && self.respawn_at.is_some_and(|t| Instant::now() >= t) {
            self.respawn_at = None;
            self.spawn();
        }
    }

    fn spawn(&mut self) {
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                warning!("standby unavailable: {e}");
                return;
            }
        };
        let mut cmd = Command::new(exe);
        cmd.arg("standby").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        crate::app::detach(&mut cmd);
        match cmd.spawn() {
            Ok(child) => self.ready = Some(child),
            Err(e) => {
                warning!("could not start the standby window: {e}");
                self.respawn_at = Some(Instant::now() + Duration::from_secs(30));
            }
        }
    }

    /// Hands `args` (like `["record", "--gif"]`) to the waiting standby and returns its process,
    /// now doing the work. `None` if there is no standby or it can't take this action: start a
    /// normal process then.
    pub fn trigger(&mut self, args: &[&str]) -> Option<Child> {
        AreaAction::parse(args)?;
        let mut child = self.ready.take()?;
        if enabled() {
            self.respawn_at = Some(Instant::now() + Duration::from_millis(1500));
        }
        if !matches!(child.try_wait(), Ok(None)) {
            return None;
        }
        let line = format!("{}\n", args.join(" "));
        // Writing the line and closing stdin: a standby takes exactly one action.
        let sent = child.stdin.take().is_some_and(|mut s| s.write_all(line.as_bytes()).and_then(|_| s.flush()).is_ok());
        if sent {
            Some(child)
        } else {
            let _ = child.kill();
            None
        }
    }
}

/// Full-size frames to paint underneath before raising: GPU frames reach the screen a frame or
/// two after they're drawn.
const UNCLOAK_AFTER: u32 = 3;

/// Where the waiting window sits: far outside any monitor.
const PARK: f32 = -20000.0;

/// What the stdin thread hands the window.
enum Msg {
    Quit,
    Failed(String),
    Ready { action: AreaAction, prep: Box<Prepared>, shot: crate::capture::Shot },
}

/// Runs on the stdin thread the moment the action arrives, while the window wakes up (hidden
/// windows only repaint every 100 ms): the checks, then the screen grab.
fn prepare_and_capture(words: &str) -> Msg {
    let parts: Vec<&str> = words.split_whitespace().collect();
    let Some(action) = AreaAction::parse(&parts) else {
        return Msg::Failed(format!("unknown standby action {words:?}"));
    };
    crate::log::timing("standby: action received");
    let prep = match commands::prepare(action, CaptureOptions::default()) {
        Ok(Some(p)) => p,
        Ok(None) => return Msg::Quit,
        Err(e) => return Msg::Failed(e),
    };
    match capture::shoot_monitor_under_cursor(true) {
        Ok(shot) => {
            crate::log::timing("standby: screen captured");
            Msg::Ready { action, prep: Box::new(prep), shot }
        }
        Err(e) => Msg::Failed(e),
    }
}

/// `snapsmith standby`: wait hidden for one action on stdin, then run it.
pub fn run() -> Result<(), String> {
    let ctx_slot: Arc<Mutex<Option<egui::Context>>> = Arc::default();
    let (tx, rx) = mpsc::channel::<Msg>();
    {
        let slot = ctx_slot.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            let msg = match std::io::stdin().lock().read_line(&mut line) {
                Ok(n) if n > 0 => {
                    let words = line.trim().trim_start_matches('\u{feff}').trim().to_string();
                    if words.is_empty() { Msg::Quit } else { prepare_and_capture(&words) }
                }
                _ => Msg::Quit,
            };
            // Wait until the window exists, so the wake-up can't be missed.
            loop {
                if let Some(ctx) = slot.lock().unwrap().clone() {
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
    }

    let viewport = egui::ViewportBuilder::default()
        .with_title("Snapsmith")
        .with_app_id("snapsmith")
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_has_shadow(false)
        .with_active(false)
        // Parked off screen, and visible: eframe only repaints hidden windows every 100 ms.
        .with_position(pos2(PARK, PARK))
        .with_inner_size(vec2(32.0, 32.0));
    let outcome: Arc<Mutex<Outcome>> = Arc::default();
    let out = outcome.clone();
    let mut rx = Some(rx);
    crate::app::run_window("Snapsmith", viewport, true, |cc| {
        *ctx_slot.lock().unwrap() = Some(cc.egui_ctx.clone());
        Standby { rx: rx.take().expect("standby built once"), state: State::Waiting, outcome: out.clone(), frames: 0 }
    })?;
    let outcome = std::mem::take(&mut *outcome.lock().unwrap());
    match outcome {
        Outcome::Nothing => Ok(()),
        Outcome::Failed(e) => Err(e),
        Outcome::Active { prep, picked } => {
            let picked = picked.lock().unwrap().take();
            commands::after_pick(*prep, picked)
        }
    }
}

#[derive(Default)]
enum Outcome {
    #[default]
    Nothing,
    Failed(String),
    Active {
        prep: Box<Prepared>,
        picked: Arc<Mutex<Option<Picked>>>,
    },
}

enum State {
    Waiting,
    Done,
    /// `sized`: frames painted at the monitor's size so far, while underneath. `shown_at`: when it
    /// was raised.
    Active {
        overlay: Box<Overlay>,
        sized: u32,
        shown_at: Option<Instant>,
    },
}

struct Standby {
    rx: Receiver<Msg>,
    state: State,
    outcome: Arc<Mutex<Outcome>>,
    frames: u64,
}

impl Standby {
    fn quit(&mut self, ctx: &egui::Context, outcome: Outcome) {
        crate::log::timing(&format!("standby: quitting (frame {})", self.frames));
        *self.outcome.lock().unwrap() = outcome;
        self.state = State::Done;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn activate(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        action: AreaAction,
        prep: Box<Prepared>,
        shot: crate::capture::Shot,
    ) {
        let picked: Arc<Mutex<Option<Picked>>> = Arc::default();
        let mut overlay = Overlay::new(shot, action.mode(), prep.settings.capture.magnifier, None, picked.clone());
        overlay.self_place = false;
        // Into place underneath every window, showing the frozen picture undimmed, which looks
        // exactly like the screen does anyway. Once real frames are on screen it's raised and
        // the selection starts. (A window only has valid pixels once it's on screen, and fading
        // or cloaking it in stalls the compositor; this way neither is needed.) Usually only a
        // move: the window already has the monitor's size.
        overlay.plain = true;
        let rect = overlay.monitor_rect();
        if let Some(w) = frame.winit_window() {
            platform::place_behind(w, rect);
        }
        *self.outcome.lock().unwrap() = Outcome::Active { prep, picked };
        self.state = State::Active { overlay: Box::new(overlay), sized: 0, shown_at: None };
        ctx.request_repaint();
    }
}

impl eframe::App for Standby {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.frames += 1;
        if !matches!(self.state, State::Waiting) {
            return;
        }
        if self.frames == 1
            && let Some(w) = frame.winit_window()
        {
            platform::tool_window(w);
            platform::prepare_overlay(w);
            // Wait at the size of the monitor under the pointer: resizing a GPU window to full
            // screen at the last moment stalls the screen for a fraction of a second.
            if let Some(m) = platform::monitor_under_cursor()
                && let Ok(r) = capture::monitor_rect(&m)
            {
                platform::place(w, crate::platform::DesktopRect { x: PARK as i32, y: PARK as i32, w: r.w, h: r.h });
            }
        }
        match self.rx.try_recv() {
            Ok(Msg::Ready { action, prep, shot }) => self.activate(ctx, frame, action, prep, shot),
            Ok(Msg::Failed(e)) => self.quit(ctx, Outcome::Failed(e)),
            // Stdin closed without an action: whoever started us is gone.
            Ok(Msg::Quit) | Err(TryRecvError::Disconnected) => self.quit(ctx, Outcome::Nothing),
            Err(TryRecvError::Empty) => {}
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let State::Active { overlay, sized, shown_at } = &mut self.state else { return };
        let rect = overlay.monitor_rect();
        // Raise only after full-size frames were painted on screen (the first may still go to
        // the old surface while a resize is processed), so it never shows a stale or blank one.
        if shown_at.is_none()
            && *sized >= UNCLOAK_AFTER
            && let Some(w) = frame.winit_window()
        {
            *shown_at = Some(Instant::now());
            overlay.plain = false;
            platform::bring_to_front(w);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
            crate::log::timing("standby: overlay shown");
        }
        eframe::App::ui(overlay.as_mut(), ui, frame);
        if shown_at.is_none()
            && let Some(w) = frame.winit_window()
        {
            let size = w.inner_size();
            let want = if cfg!(target_os = "macos") {
                let s = w.scale_factor();
                ((rect.w as f64 * s).round() as u32, (rect.h as f64 * s).round() as u32)
            } else {
                (rect.w, rect.h)
            };
            if size.width == want.0 && size.height == want.1 {
                *sized += 1;
            }
        }
        // Keep painting briefly after raising, so a freshly presented frame is shown.
        if shown_at.is_none_or(|t| t.elapsed() < Duration::from_millis(150)) {
            ui.ctx().request_repaint();
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}
