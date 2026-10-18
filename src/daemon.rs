//! The background process: tray icon and global shortcuts. Every capture runs as a separate
//! `snapsmith ...` process, so a crash or a slow upload never takes the tray down, and no window
//! has to sit hidden waiting for a hotkey.

use std::process::Child;
use std::time::{Duration, Instant};

use global_hotkey::GlobalHotKeyEvent;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::MenuEvent;

use snapsmith::config::{self, HotkeyAction};

use crate::hotkey::Hotkeys;
use crate::log::{error, info, warning};
use crate::tray::{MenuAction, Tray};
use crate::{autostart, platform, record, single_instance};

/// Created by the main window after installing an update.
pub const RESTART_FILE: &str = "restart-daemon";

enum UserEvent {
    Hotkey(GlobalHotKeyEvent),
    Menu(MenuEvent),
}

pub fn run() -> Result<(), String> {
    let lock = match single_instance::acquire_patiently(single_instance::DAEMON)? {
        Some(l) => l,
        None => {
            println!("Snapsmith is already running.");
            return Ok(());
        }
    };
    let config_path = config::default_path();
    let (mut settings, warn) = config::load_or_create(&config_path);
    if let Some(w) = warn {
        warning!("{w}");
    }
    info!("background process started (version {})", env!("CARGO_PKG_VERSION"));

    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    GlobalHotKeyEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Hotkey(e));
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    let mut hotkeys = Hotkeys::new();
    hotkeys.apply(&settings.hotkeys);
    let launcher = autostart::launcher();
    let mut tray: Option<Tray> = None;
    let mut mtime = config::mtime(&config_path);
    let mut children: Vec<(HotkeyAction, Child)> = Vec::new();
    let mut last_tick = Instant::now();
    let _lock = lock;

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));
        match event {
            Event::NewEvents(StartCause::Init) => {
                let auto = launcher.as_ref().map(autostart::is_enabled);
                match Tray::new(&hotkeys, auto) {
                    Ok(t) => tray = Some(t),
                    Err(e) => error!("tray icon unavailable: {e}"),
                }
            }
            Event::UserEvent(UserEvent::Hotkey(e)) => {
                if let Some(action) = hotkeys.action_for(e.id, e.state) {
                    trigger(action, &mut children);
                }
            }
            Event::UserEvent(UserEvent::Menu(e)) => {
                let Some(action) = tray.as_ref().and_then(|t| t.action_for(e.id())) else { return };
                match action {
                    MenuAction::Run(a) => trigger(a, &mut children),
                    MenuAction::StopRecording => record::request_stop(),
                    MenuAction::OpenWindow => spawn(&["window"]),
                    MenuAction::OpenFolder => {
                        let dir = settings.capture.save_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        crate::app::open(&dir.to_string_lossy());
                    }
                    MenuAction::ToggleAutostart => {
                        if let Some(l) = &launcher {
                            autostart::set_enabled(l, !autostart::is_enabled(l));
                        }
                    }
                    MenuAction::Quit => {
                        info!("quit from the tray");
                        *control_flow = ControlFlow::Exit;
                        return;
                    }
                }
                sync(&mut tray, &hotkeys, &launcher);
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) | Event::MainEventsCleared => {
                if last_tick.elapsed() < Duration::from_millis(400) {
                    return;
                }
                last_tick = Instant::now();
                // Settings edited in the window (or by hand): pick them up.
                let m = config::mtime(&config_path);
                if m != mtime {
                    mtime = m;
                    match config::load(&config_path) {
                        Ok(s) => {
                            if s.hotkeys != settings.hotkeys {
                                hotkeys.apply(&s.hotkeys);
                            }
                            settings = s;
                        }
                        Err(e) => warning!("{e}"),
                    }
                }
                children.retain_mut(|(_, c)| matches!(c.try_wait(), Ok(None)));
                // After an update the main window asks for a restart on the new program file.
                let restart = config::config_dir().join(RESTART_FILE);
                if restart.exists() {
                    let _ = std::fs::remove_file(&restart);
                    info!("restarting after an update");
                    spawn(&["daemon"]);
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                sync(&mut tray, &hotkeys, &launcher);
            }
            _ => {}
        }
    })
}

fn sync(tray: &mut Option<Tray>, hotkeys: &Hotkeys, launcher: &Option<auto_launch::AutoLaunch>) {
    if let Some(t) = tray {
        let auto = launcher.as_ref().map(autostart::is_enabled);
        t.sync(hotkeys, auto, single_instance::is_held(single_instance::RECORDING));
    }
}

fn trigger(action: HotkeyAction, children: &mut Vec<(HotkeyAction, Child)>) {
    // A recording shortcut while recording stops it.
    if matches!(action, HotkeyAction::RecordGif | HotkeyAction::RecordVideo)
        && single_instance::is_held(single_instance::RECORDING)
    {
        record::request_stop();
        return;
    }
    // One selection overlay at a time.
    if matches!(action, HotkeyAction::Region | HotkeyAction::Ocr | HotkeyAction::RecordGif | HotkeyAction::RecordVideo)
        && single_instance::is_held(single_instance::OVERLAY)
    {
        return;
    }
    platform::allow_child_foreground();
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            error!("{e}");
            return;
        }
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(action.args()).stdin(std::process::Stdio::null());
    crate::app::detach(&mut cmd);
    match cmd.spawn() {
        Ok(child) => children.push((action, child)),
        Err(e) => error!("could not start {}: {e}", action.label()),
    }
}

fn spawn(args: &[&str]) {
    if let Err(e) = crate::app::spawn_self(args) {
        error!("{e}");
    }
}

/// Starts the background process unless it's already running.
pub fn ensure_running() {
    if !single_instance::is_held(single_instance::DAEMON) {
        spawn(&["daemon"]);
    }
}
