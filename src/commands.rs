//! The flows behind each command: capture, then the overlay or editor, then the tasks.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use image::RgbaImage;

use snapsmith::config::{self, Settings, Task};

use crate::capture;
use crate::log::{info, warning};
use crate::overlay::{self, Action, Mode};
use crate::platform::{self, DesktopRect};
use crate::record;
use crate::single_instance;
use crate::tasks::{self, Job, Progress};
use crate::toast::{self, Toast};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum CaptureKind {
    /// Select an area (or click a window) on a frozen screen.
    Region,
    /// Every monitor.
    Screen,
    /// The monitor under the pointer.
    Monitor,
    /// The focused window.
    Window,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureOptions {
    pub delay: Option<f32>,
    /// Replaces the after-capture tasks from the settings.
    pub tasks: Option<Vec<Task>>,
    /// Save exactly here (and print the path).
    pub output: Option<PathBuf>,
}

pub fn load_settings() -> Settings {
    let (s, warning) = config::load_or_create(&config::default_path());
    if let Some(w) = warning {
        warning!("{w}");
    }
    s
}

/// On macOS, captures need the Screen Recording permission. Asks for it if missing.
fn ensure_permission() -> Result<(), String> {
    if platform::screen_capture_allowed() {
        return Ok(());
    }
    platform::request_screen_capture();
    if platform::screen_capture_allowed() {
        return Ok(());
    }
    Err("Snapsmith needs permission to record the screen. Allow it in System Settings > Privacy & Security > \
         Screen Recording, then start Snapsmith again."
        .into())
}

/// The commands that start with picking an area on a frozen screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaAction {
    Region,
    Ocr,
    Record(record::Format),
}

impl AreaAction {
    /// From command-line words: `capture region`, `ocr`, `record --gif`, `record --mp4`.
    pub fn parse(words: &[&str]) -> Option<Self> {
        match words {
            ["capture"] | ["capture", "region"] => Some(AreaAction::Region),
            ["ocr"] => Some(AreaAction::Ocr),
            ["record"] | ["record", "--gif"] => Some(AreaAction::Record(record::Format::Gif)),
            ["record", "--mp4"] => Some(AreaAction::Record(record::Format::Mp4)),
            _ => None,
        }
    }

    pub fn mode(self) -> Mode {
        match self {
            AreaAction::Region => Mode::Capture,
            AreaAction::Ocr | AreaAction::Record(_) => Mode::Area,
        }
    }
}

/// What was checked and claimed before the selection appeared.
pub struct Prepared {
    pub action: AreaAction,
    pub settings: Settings,
    opts: CaptureOptions,
    ffmpeg: Option<PathBuf>,
    overlay_lock: single_instance::InstanceLock,
    recording_lock: Option<single_instance::InstanceLock>,
}

/// Everything that can fail before the selection appears, so it fails before, not after.
/// `Ok(None)` means there's nothing to do (a selection is already open, or this stopped a
/// recording).
pub fn prepare(action: AreaAction, opts: CaptureOptions) -> Result<Option<Prepared>, String> {
    let recording_lock = match action {
        AreaAction::Record(_) => {
            // Starting a recording while one runs stops it instead.
            match single_instance::acquire(single_instance::RECORDING)? {
                Some(l) => {
                    let _ = std::fs::remove_file(record::stop_file());
                    Some(l)
                }
                None => {
                    record::request_stop();
                    return Ok(None);
                }
            }
        }
        _ => None,
    };
    let settings = load_settings();
    crate::log::timing("settings loaded");
    if matches!(action, AreaAction::Record(_)) && platform::is_wayland() {
        return Err("Screen recording isn't available on Wayland yet: the desktop asks for permission on every \
                    frame. It works in an X11 session."
            .into());
    }
    ensure_permission()?;
    let ffmpeg = match action {
        AreaAction::Record(record::Format::Mp4) => Some(record::find_ffmpeg(&settings)?),
        _ => None,
    };
    let Some(overlay_lock) = single_instance::acquire(single_instance::OVERLAY)? else {
        info!("a selection is already open");
        return Ok(None);
    };
    Ok(Some(Prepared { action, settings, opts, ffmpeg, overlay_lock, recording_lock }))
}

/// Everything after the selection closes.
pub fn after_pick(prep: Prepared, picked: Option<overlay::Picked>) -> Result<(), String> {
    let Prepared { action, settings, opts, ffmpeg, overlay_lock, recording_lock } = prep;
    drop(overlay_lock);
    let Some(p) = picked else { return Ok(()) };
    match action {
        AreaAction::Region => finish_action(p.image, p.action, &settings, Some(p.desktop), String::new(), &opts),
        AreaAction::Ocr => crate::ocr_window::run(p.image),
        AreaAction::Record(format) => {
            let _recording = recording_lock;
            record::start(&settings, format, ffmpeg, p.desktop, p.image.dimensions(), p.monitor)
        }
    }
}

/// Prepares, opens a new selection window, and carries on.
pub fn pick_and_continue(action: AreaAction, opts: CaptureOptions) -> Result<(), String> {
    let Some(prep) = prepare(action, opts)? else { return Ok(()) };
    let monitor = platform::monitor_under_cursor().ok_or("no monitor found")?;
    let rect = capture::monitor_rect(&monitor)?;
    let pending = overlay::Source::Capturing(capture::shoot_in_background(rect, true));
    let picked = overlay::run_source(rect, pending, action.mode(), prep.settings.capture.magnifier, None)?;
    after_pick(prep, picked)
}

pub fn capture(kind: CaptureKind, opts: CaptureOptions) -> Result<(), String> {
    if kind == CaptureKind::Region {
        return pick_and_continue(AreaAction::Region, opts);
    }
    let settings = load_settings();
    ensure_permission()?;
    let delay = opts.delay.unwrap_or(if kind == CaptureKind::Region { 0.0 } else { settings.capture.delay });
    if delay > 0.0 {
        std::thread::sleep(Duration::from_secs_f32(delay));
    }
    let anchor = platform::cursor_pos();
    let (image, title) = match kind {
        CaptureKind::Region => unreachable!("handled above"),
        CaptureKind::Screen => (capture::shoot_all()?, String::new()),
        CaptureKind::Monitor => (capture::shoot_monitor_under_cursor(false)?.image, String::new()),
        CaptureKind::Window => capture::shoot_active_window()?,
    };
    let at = anchor.map(|(x, y)| DesktopRect { x, y, w: 1, h: 1 });
    finish_action(image, Action::Default, &settings, at, title, &opts)
}

/// What a toolbar button or Enter means, as a task list.
fn tasks_for(action: Action, settings: &Settings, opts: &CaptureOptions) -> Vec<Task> {
    let with_toast = |mut v: Vec<Task>| {
        if settings.capture.after_capture.contains(&Task::Toast) {
            v.push(Task::Toast);
        }
        v
    };
    match action {
        Action::Default => opts.tasks.clone().unwrap_or_else(|| settings.capture.after_capture.clone()),
        Action::Copy => with_toast(vec![Task::Copy]),
        Action::Save | Action::SaveAs => with_toast(vec![Task::Save]),
        Action::Upload => vec![Task::Upload, Task::Toast],
        Action::Ocr => vec![Task::Ocr],
        Action::Pin => vec![Task::Pin],
    }
}

pub fn finish_action(
    image: RgbaImage,
    action: Action,
    settings: &Settings,
    at: Option<DesktopRect>,
    title: String,
    opts: &CaptureOptions,
) -> Result<(), String> {
    let mut tasks = tasks_for(action, settings, opts);
    let mut save_to = opts.output.clone();
    if action == Action::SaveAs {
        let suggested = tasks::planned_path(settings, image.width(), image.height(), &title);
        let mut dialog = rfd::FileDialog::new().add_filter("PNG", &["png"]).add_filter("JPEG", &["jpg", "jpeg"]);
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        dialog = dialog.set_directory(settings.capture.save_dir());
        match dialog.save_file() {
            Some(p) => save_to = Some(p),
            None => return Ok(()),
        }
    }
    if save_to.is_some() {
        tasks.retain(|t| *t != Task::Save);
    }
    run_tasks(image, tasks, settings, at, title, save_to)
}

/// Runs the tasks, showing the notification when wanted, and waits for everything to finish.
pub fn run_tasks(
    image: RgbaImage,
    tasks: Vec<Task>,
    settings: &Settings,
    at: Option<DesktopRect>,
    title: String,
    save_to: Option<PathBuf>,
) -> Result<(), String> {
    if tasks.contains(&Task::Pin) {
        pin(&image, at)?;
    }
    let print_path = save_to.is_some();
    let core: Vec<Task> =
        tasks.iter().copied().filter(|t| matches!(t, Task::Copy | Task::Save | Task::Upload)).collect();
    let show_toast = (tasks.contains(&Task::Toast) || tasks.contains(&Task::Upload)) && !tasks.contains(&Task::Ocr);
    let ocr = tasks.contains(&Task::Ocr);
    if core.is_empty() && save_to.is_none() {
        if ocr {
            return crate::ocr_window::run(image);
        }
        return Ok(());
    }

    let (tx, rx) = mpsc::channel();
    let thumb = (show_toast || ocr).then(|| image.clone());
    let ocr_image = ocr.then(|| image.clone());
    let job = Job { image, tasks: core, title, save_to };
    let s = settings.clone();
    let worker = std::thread::spawn(move || tasks::run(job, &s, &tx));
    let anchor = at.map(|r| r.center());
    if show_toast {
        let t = Toast { title: "Screenshot".into(), thumbnail: thumb, anchor, seconds: settings.general.toast_seconds };
        if let Err(e) = toast::run(t, rx) {
            warning!("{e}");
        }
    } else {
        // No notification wanted; still drain progress so the worker never blocks.
        std::thread::spawn(move || for _ in rx {});
    }
    let outcome = worker.join().map_err(|_| "the capture task crashed".to_string())?;
    if print_path && let Some(p) = &outcome.saved {
        println!("{}", p.display());
    }
    if !show_toast && !outcome.errors.is_empty() {
        show_errors("Screenshot", &outcome.errors, anchor, settings);
    }
    if let Some(img) = ocr_image {
        crate::ocr_window::run(img)?;
    }
    if outcome.errors.is_empty() { Ok(()) } else { Err(outcome.errors.join("; ")) }
}

/// A notification that only lists problems.
pub fn show_errors(title: &str, errors: &[String], anchor: Option<(i32, i32)>, settings: &Settings) {
    let (tx, rx) = mpsc::channel();
    for e in errors {
        let _ = tx.send(Progress::Failed(e.clone()));
    }
    let _ = tx.send(Progress::Done);
    let t = Toast { title: title.into(), thumbnail: None, anchor, seconds: settings.general.toast_seconds };
    if let Err(e) = toast::run(t, rx) {
        warning!("{e}");
        platform::show_error(&errors.join("\n"));
    }
}

/// Opens the image in a pinned window, in its own process so it can stay up.
pub fn pin(image: &RgbaImage, at: Option<DesktopRect>) -> Result<(), String> {
    let path = crate::app::temp_file("pin", "png");
    image.save(&path).map_err(|e| e.to_string())?;
    let p = path.to_string_lossy().into_owned();
    let mut args = vec!["pin".to_string(), p, "--delete".to_string()];
    if let Some(r) = at.filter(|r| r.w > 1) {
        args.push("--at".into());
        args.push(format!("{},{},{},{}", r.x, r.y, r.w, r.h));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::app::spawn_self(&refs).map(|_| ())
}

/// `snapsmith ocr [file]`: read text from a file, or from an area picked on screen.
pub fn ocr(file: Option<&Path>) -> Result<(), String> {
    match file {
        Some(p) => {
            let image = image::open(p).map_err(|e| format!("could not open {}: {e}", p.display()))?.to_rgba8();
            crate::ocr_window::run(image)
        }
        None => pick_and_continue(AreaAction::Ocr, CaptureOptions::default()),
    }
}

/// `snapsmith edit <file>`.
pub fn edit(path: &Path) -> Result<(), String> {
    let settings = load_settings();
    let image = image::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?.to_rgba8();
    let Some((img, action)) = crate::editor_window::run(image, Some(path.to_path_buf()), settings.clone())? else {
        return Ok(());
    };
    finish_action(img, action, &settings, None, String::new(), &CaptureOptions::default())
}

/// `snapsmith upload <file>`: prints the link.
pub fn upload(path: &Path, quiet_ui: bool) -> Result<(), String> {
    let settings = load_settings();
    let (tx, rx) = mpsc::channel();
    let s = settings.clone();
    let p = path.to_path_buf();
    let worker = std::thread::spawn(move || tasks::upload_file(&s, &p, &tx));
    if quiet_ui {
        std::thread::spawn(move || for _ in rx {});
    } else {
        let thumb = image::open(path).ok().map(|i| i.to_rgba8());
        let t =
            Toast { title: "Upload".into(), thumbnail: thumb, anchor: None, seconds: settings.general.toast_seconds };
        if let Err(e) = toast::run(t, rx) {
            warning!("{e}");
        }
    }
    let outcome = worker.join().map_err(|_| "the upload crashed".to_string())?;
    if let Some(url) = &outcome.url {
        println!("{url}");
        if let Some(d) = &outcome.deletion_url {
            println!("deletion: {d}");
        }
    }
    if outcome.errors.is_empty() { Ok(()) } else { Err(outcome.errors.join("; ")) }
}
