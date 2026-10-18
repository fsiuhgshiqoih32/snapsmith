//! The Snapsmith window: capture buttons, history, and settings. It edits the config file; the
//! background process picks changes up within a second.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, RichText, TextureOptions, vec2};
use image::RgbaImage;

use snapsmith::config::{self, HotkeyAction, ImageFormat, Settings, Task};
use snapsmith::history::{self, Entry};
use snapsmith::naming::{self, NameContext};
use snapsmith::upload::{self, Destination};

use crate::hotkey;
use crate::log::warning;
use crate::ui;
use crate::ui::icons::{self, Icon};
use crate::{autostart, daemon, single_instance, updater};

const RAISE_FILE: &str = "raise-window";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    History,
    Capture,
    Upload,
    Hotkeys,
    Recording,
    General,
}

impl Page {
    const ALL: [Page; 6] = [Page::History, Page::Capture, Page::Upload, Page::Hotkeys, Page::Recording, Page::General];

    fn label(self) -> &'static str {
        match self {
            Page::History => "History",
            Page::Capture => "Capture",
            Page::Upload => "Upload",
            Page::Hotkeys => "Shortcuts",
            Page::Recording => "Recording",
            Page::General => "General",
        }
    }

    fn parse(s: &str) -> Option<Page> {
        Page::ALL
            .into_iter()
            .find(|p| p.label().eq_ignore_ascii_case(s.trim()) || format!("{p:?}").eq_ignore_ascii_case(s.trim()))
    }
}

pub fn open(page: Option<String>) -> Result<(), String> {
    let lock = match single_instance::acquire(single_instance::WINDOW)? {
        Some(l) => l,
        None => {
            // Already open: bring it forward.
            let _ = std::fs::write(config::config_dir().join(RAISE_FILE), page.unwrap_or_default());
            return Ok(());
        }
    };
    daemon::ensure_running();
    let page = page.as_deref().and_then(Page::parse).unwrap_or(Page::History);
    run(page, None, lock)
}

/// Renders the window with sample history into a PNG, for the README.
pub fn demo_shot(path: &Path) -> Result<(), String> {
    run(
        Page::History,
        Some(path.to_path_buf()),
        single_instance::acquire(single_instance::WINDOW)?.ok_or("window open")?,
    )
}

fn run(page: Page, demo: Option<PathBuf>, lock: single_instance::InstanceLock) -> Result<(), String> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Snapsmith")
        .with_app_id("snapsmith")
        .with_inner_size(vec2(980.0, 640.0))
        .with_min_inner_size(vec2(720.0, 460.0))
        .with_drag_and_drop(true);
    let mut lock = Some(lock);
    crate::app::run_window("Snapsmith", viewport, false, |cc| {
        MainWindow::new(cc, page, demo.clone(), lock.take().expect("window built once"))
    })
    .map_err(|e| format!("could not open the window: {e}"))
}

enum Thumb {
    Loading,
    Ready(egui::TextureHandle),
    Missing,
}

struct HotkeyEdit {
    text: String,
    error: Option<String>,
}

struct MainWindow {
    page: Page,
    config_path: PathBuf,
    settings: Settings,
    written: Settings,
    edited_at: Option<Instant>,
    file_mtime: Option<SystemTime>,
    last_poll: Instant,
    raise_mtime: Option<SystemTime>,

    history: Vec<Entry>,
    history_mtime: Option<SystemTime>,
    filter: String,
    thumbs: HashMap<PathBuf, Thumb>,
    thumb_tx: Sender<PathBuf>,
    thumb_rx: Receiver<(PathBuf, Option<RgbaImage>)>,

    destinations: Vec<Destination>,
    upload_msg: Option<(String, bool)>,
    test_upload: Option<Receiver<Result<String, String>>>,

    hotkey_edits: HashMap<HotkeyAction, HotkeyEdit>,
    listening: Option<HotkeyAction>,
    hotkey_status: Option<hotkey::Status>,

    autostart: Option<auto_launch::AutoLaunch>,
    autostart_on: bool,
    update_rx: Option<Receiver<Result<Option<updater::Update>, String>>>,
    update_status: String,
    update: Option<updater::Update>,
    ffmpeg_status: Option<Result<PathBuf, String>>,

    pending_capture: Option<(Vec<&'static str>, Instant)>,
    standby: crate::standby::Pool,
    running: Option<Child>,
    toast: Option<(String, f64)>,

    demo: Option<PathBuf>,
    frame_no: u64,
    _lock: single_instance::InstanceLock,
}

fn thumb_worker(rx: Receiver<PathBuf>, tx: Sender<(PathBuf, Option<RgbaImage>)>, ctx: egui::Context) {
    std::thread::spawn(move || {
        for path in rx {
            let img = image::open(&path).ok().map(|i| i.thumbnail(240, 150).to_rgba8());
            if tx.send((path, img)).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
}

impl MainWindow {
    fn new(
        cc: &eframe::CreationContext<'_>,
        page: Page,
        demo: Option<PathBuf>,
        lock: single_instance::InstanceLock,
    ) -> Self {
        let config_path = config::default_path();
        let (settings, warn) = config::load_or_create(&config_path);
        if let Some(w) = warn {
            warning!("{w}");
        }
        let (req_tx, req_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        thumb_worker(req_rx, res_tx, cc.egui_ctx.clone());
        let autostart = autostart::launcher();
        let autostart_on = autostart.as_ref().is_some_and(autostart::is_enabled);
        let mut w = MainWindow {
            page,
            file_mtime: config::mtime(&config_path),
            config_path,
            written: settings.clone(),
            settings,
            edited_at: None,
            last_poll: Instant::now() - Duration::from_secs(5),
            raise_mtime: config::mtime(&config::config_dir().join(RAISE_FILE)),
            history: Vec::new(),
            history_mtime: None,
            filter: String::new(),
            thumbs: HashMap::new(),
            thumb_tx: req_tx,
            thumb_rx: res_rx,
            destinations: upload::destinations(&config::uploaders_dir()),
            upload_msg: None,
            test_upload: None,
            hotkey_edits: HashMap::new(),
            listening: None,
            hotkey_status: hotkey::read_status(),
            autostart,
            autostart_on,
            update_rx: None,
            update_status: String::new(),
            update: None,
            ffmpeg_status: None,
            pending_capture: None,
            standby: crate::standby::Pool::new(),
            running: None,
            toast: None,
            demo,
            frame_no: 0,
            _lock: lock,
        };
        w.reset_hotkey_edits();
        if w.demo.is_some() {
            w.history = demo_history();
        } else {
            w.reload_history();
        }
        w
    }

    fn reset_hotkey_edits(&mut self) {
        self.hotkey_edits = self
            .settings
            .hotkeys
            .bindings()
            .iter()
            .map(|(a, s)| (*a, HotkeyEdit { text: s.to_string(), error: None }))
            .collect();
    }

    fn reload_history(&mut self) {
        let path = history::default_path();
        self.history_mtime = config::mtime(&path);
        let mut h = history::load(&path);
        h.reverse();
        self.history = h;
    }

    fn say(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        self.toast = Some((msg.into(), ctx.input(|i| i.time)));
    }

    /// Settings file, history and raise requests, checked a few times a second.
    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((path, img)) = self.thumb_rx.try_recv() {
            let t = match img {
                Some(img) => {
                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                        [img.width() as usize, img.height() as usize],
                        img.as_raw(),
                    );
                    Thumb::Ready(ctx.load_texture(path.to_string_lossy(), ci, TextureOptions::LINEAR))
                }
                None => Thumb::Missing,
            };
            self.thumbs.insert(path, t);
        }
        if self.settings != self.written && self.edited_at.is_some_and(|t| t.elapsed() > Duration::from_millis(350)) {
            self.edited_at = None;
            match config::save(&self.config_path, &self.settings) {
                Ok(()) => {
                    self.written = self.settings.clone();
                    self.file_mtime = config::mtime(&self.config_path);
                }
                Err(e) => warning!("could not save settings: {e}"),
            }
        }
        if self.last_poll.elapsed() < Duration::from_millis(700) {
            return;
        }
        self.last_poll = Instant::now();
        let m = config::mtime(&self.config_path);
        if m != self.file_mtime && self.settings == self.written {
            self.file_mtime = m;
            if let Ok(s) = config::load(&self.config_path) {
                self.settings = s.clone();
                self.written = s;
                self.reset_hotkey_edits();
            }
        }
        if self.demo.is_none() && config::mtime(&history::default_path()) != self.history_mtime {
            self.reload_history();
        }
        let raise = config::config_dir().join(RAISE_FILE);
        let r = config::mtime(&raise);
        if r != self.raise_mtime {
            self.raise_mtime = r;
            if let Some(p) = std::fs::read_to_string(&raise).ok().as_deref().and_then(Page::parse) {
                self.page = p;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.hotkey_status = hotkey::read_status();
    }

    /// Brings the window back once the capture it started has finished.
    fn check_running(&mut self, ctx: &egui::Context) {
        if let Some(c) = &mut self.running
            && !matches!(c.try_wait(), Ok(None))
        {
            self.running = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn edited(&mut self) {
        self.edited_at = Some(Instant::now());
    }

    /// Hides the window (instantly; minimising animates), then starts the capture, which freezes
    /// the screen without this window on it.
    fn start_capture(&mut self, ctx: &egui::Context, args: Vec<&'static str>) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        self.pending_capture = Some((args, Instant::now()));
        ctx.request_repaint_after(Duration::from_millis(60));
    }

    fn run_pending(&mut self) {
        if let Some((args, at)) = &self.pending_capture
            && at.elapsed() > Duration::from_millis(60)
        {
            let args = args.clone();
            self.pending_capture = None;
            crate::platform::allow_child_foreground();
            if let Some(c) = self.standby.trigger(&args) {
                self.running = Some(c);
                return;
            }
            match crate::app::spawn_self(&args) {
                Ok(c) => self.running = Some(c),
                Err(e) => warning!("{e}"),
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let buttons: [(&str, Vec<&'static str>); 6] = [
                ("Region", vec!["capture", "region"]),
                ("Window", vec!["capture", "window"]),
                ("Screen", vec!["capture", "screen"]),
                ("Record GIF", vec!["record", "--gif"]),
                ("Record video", vec!["record", "--mp4"]),
                ("Extract text", vec!["ocr"]),
            ];
            for (label, args) in buttons {
                if ui.button(label).clicked() {
                    self.start_capture(&ctx, args);
                }
            }
            ui.separator();
            if ui.button("Upload file...").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                let p = path.to_string_lossy().into_owned();
                if let Err(e) = crate::app::spawn_self(&["upload", &p]) {
                    warning!("{e}");
                }
            }
            if ui.button("Open image...").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "webp"])
                    .pick_file()
            {
                let p = path.to_string_lossy().into_owned();
                if let Err(e) = crate::app::spawn_self(&["edit", &p]) {
                    warning!("{e}");
                }
            }
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        for p in Page::ALL {
            let selected = self.page == p;
            let text = RichText::new(p.label()).size(14.0);
            let resp = ui.add_sized([ui.available_width(), 30.0], egui::Button::selectable(selected, text));
            if resp.clicked() {
                self.page = p;
                self.listening = None;
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Snapsmith {}", env!("CARGO_PKG_VERSION"))).color(ui::MUTED).size(11.5));
            let running = single_instance::is_held(single_instance::DAEMON);
            ui.label(
                RichText::new(if running { "Shortcuts active" } else { "Shortcuts off" })
                    .color(if running { ui::OK } else { ui::MUTED })
                    .size(11.5),
            );
        });
    }

    fn history_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter").desired_width(220.0));
            ui.label(RichText::new(format!("{} items", self.history.len())).color(ui::MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Screenshots folder").clicked() {
                    let dir = self.settings.capture.save_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    crate::app::open(&dir.to_string_lossy());
                }
            });
        });
        ui.add_space(6.0);
        if self.history.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Nothing here yet").size(16.0));
                ui.add_space(4.0);
                let key = if self.settings.hotkeys.region.is_empty() {
                    "the Region button".to_string()
                } else {
                    self.settings.hotkeys.region.clone()
                };
                ui::hint(ui, &format!("Press {key} to capture part of the screen."));
            });
            return;
        }
        let filter = self.filter.to_lowercase();
        let items: Vec<usize> = (0..self.history.len())
            .filter(|&i| {
                let e = &self.history[i];
                filter.is_empty()
                    || e.display_name().to_lowercase().contains(&filter)
                    || e.url.as_deref().is_some_and(|u| u.to_lowercase().contains(&filter))
            })
            .collect();
        let row_h = 84.0;
        let mut remove = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, items.len(), |ui, range| {
            for &i in &items[range] {
                let entry = self.history[i].clone();
                let frame = egui::Frame::new().fill(ui::PANEL).corner_radius(6).inner_margin(egui::Margin::same(8));
                frame.show(ui, |ui| {
                    ui.set_height(row_h - 22.0);
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(110.0, 62.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 4.0, Color32::from_black_alpha(70));
                        if let Some(path) = &entry.path {
                            match self.thumbs.get(path) {
                                Some(Thumb::Ready(t)) => {
                                    let size = t.size_vec2();
                                    let s = (rect.width() / size.x).min(rect.height() / size.y);
                                    let r = egui::Rect::from_center_size(rect.center(), size * s);
                                    ui.painter().image(
                                        t.id(),
                                        r,
                                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                        Color32::WHITE,
                                    );
                                }
                                Some(Thumb::Loading) => {}
                                Some(Thumb::Missing) => {
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        file_kind(path),
                                        egui::FontId::proportional(12.0),
                                        ui::MUTED,
                                    );
                                }
                                None => {
                                    self.thumbs.insert(path.clone(), Thumb::Loading);
                                    let _ = self.thumb_tx.send(path.clone());
                                }
                            }
                        } else {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "link",
                                egui::FontId::proportional(12.0),
                                ui::MUTED,
                            );
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.add(egui::Label::new(RichText::new(entry.display_name()).strong()).truncate());
                            let when = chrono::DateTime::from_timestamp_millis(entry.time)
                                .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                                .unwrap_or_default();
                            let mut meta = when;
                            if entry.width > 0 {
                                meta.push_str(&format!("   {} x {}", entry.width, entry.height));
                            }
                            if let Some(u) = &entry.uploader {
                                meta.push_str(&format!("   {u}"));
                            }
                            ui.label(RichText::new(meta).color(ui::MUTED).size(12.0));
                            if let Some(url) = &entry.url {
                                ui.add(egui::Label::new(RichText::new(url).color(ui::ACCENT).size(12.0)).truncate());
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if icons::button(ui, Icon::Cancel, false, true, "Remove from history (keeps the file)")
                                .clicked()
                            {
                                remove = Some(i);
                            }
                            let exists = entry.path.as_ref().is_some_and(|p| p.exists());
                            if let Some(p) = entry.path.as_ref().filter(|_| exists) {
                                if icons::button(ui, Icon::Folder, false, true, "Show in folder").clicked() {
                                    crate::app::reveal(p);
                                }
                                if entry.url.is_none()
                                    && icons::button(ui, Icon::Upload, false, true, "Upload").clicked()
                                {
                                    let _ = crate::app::spawn_self(&["upload", &p.to_string_lossy()]);
                                }
                                if is_image(p) {
                                    if icons::button(ui, Icon::Tool(snapsmith::editor::Tool::Pen), false, true, "Edit")
                                        .clicked()
                                    {
                                        let _ = crate::app::spawn_self(&["edit", &p.to_string_lossy()]);
                                    }
                                    if icons::button(ui, Icon::Copy, false, true, "Copy image").clicked() {
                                        match image::open(p).map(|i| i.to_rgba8()) {
                                            Ok(img) => match crate::clipboard::copy_image(&img) {
                                                Ok(()) => self.say(&ctx, "Image copied"),
                                                Err(e) => self.say(&ctx, e),
                                            },
                                            Err(e) => self.say(&ctx, e.to_string()),
                                        }
                                    }
                                }
                            }
                            if let Some(url) = &entry.url {
                                if icons::button(ui, Icon::Link, false, true, "Copy link").clicked() {
                                    crate::clipboard::copy_text(url);
                                    self.say(&ctx, "Link copied");
                                }
                                if icons::button(ui, Icon::Open, false, true, "Open link").clicked() {
                                    crate::app::open(url);
                                }
                            } else if let Some(p) = entry.path.as_ref().filter(|_| exists)
                                && icons::button(ui, Icon::Open, false, true, "Open").clicked()
                            {
                                crate::app::open(&p.to_string_lossy());
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
        });
        if let Some(i) = remove {
            self.history.remove(i);
            if self.demo.is_none() {
                let mut oldest_first = self.history.clone();
                oldest_first.reverse();
                if let Err(e) = history::rewrite(&history::default_path(), &oldest_first) {
                    warning!("{e}");
                }
                self.history_mtime = config::mtime(&history::default_path());
            }
        }
    }

    fn capture_page(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui::section(ui, "After a capture");
        ui::hint(ui, "Enter (or the check mark) in the selection runs these, in this order.");
        for t in Task::ALL {
            let mut on = self.settings.capture.after_capture.contains(&t);
            if ui.checkbox(&mut on, t.label()).changed() {
                if on {
                    self.settings.capture.after_capture.push(t);
                    self.settings.capture.after_capture.sort_by_key(|x| Task::ALL.iter().position(|y| y == x));
                } else {
                    self.settings.capture.after_capture.retain(|x| *x != t);
                }
                changed = true;
            }
        }

        ui::section(ui, "Saving");
        ui.horizontal(|ui| {
            ui.label("Folder");
            let mut dir = if self.settings.capture.save_dir.is_empty() {
                config::default_save_dir().to_string_lossy().into_owned()
            } else {
                self.settings.capture.save_dir.clone()
            };
            if ui.add(egui::TextEdit::singleline(&mut dir).desired_width(360.0)).changed() {
                self.settings.capture.save_dir = dir;
                changed = true;
            }
            if ui.button("Browse...").clicked()
                && let Some(p) = rfd::FileDialog::new().set_directory(self.settings.capture.save_dir()).pick_folder()
            {
                self.settings.capture.save_dir = p.to_string_lossy().into_owned();
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("File name");
            changed |= ui
                .add(egui::TextEdit::singleline(&mut self.settings.capture.file_pattern).desired_width(360.0))
                .changed();
        });
        let example = naming::expand(
            &self.settings.capture.file_pattern,
            &NameContext { width: 1280, height: 720, title: "Window".into() },
        );
        ui::hint(ui, &format!("Example: {example}.{}", self.settings.capture.format.extension()));
        ui::hint(
            ui,
            "%y %mo %d %h %mi %s %ms date and time, %rn random digit, %width %height, %t window title, / for folders",
        );
        ui.horizontal(|ui| {
            ui.label("Format");
            changed |= ui.radio_value(&mut self.settings.capture.format, ImageFormat::Png, "PNG").changed();
            changed |= ui.radio_value(&mut self.settings.capture.format, ImageFormat::Jpg, "JPEG").changed();
            if self.settings.capture.format == ImageFormat::Jpg {
                changed |= ui
                    .add(egui::Slider::new(&mut self.settings.capture.jpeg_quality, 40..=100).text("quality"))
                    .changed();
            }
        });

        ui::section(ui, "Selection");
        changed |= ui
            .checkbox(&mut self.settings.capture.magnifier, "Show the magnifier while selecting (M toggles it)")
            .changed();
        ui.horizontal(|ui| {
            ui.label("Delay before screen and window captures");
            changed |= ui
                .add(egui::Slider::new(&mut self.settings.capture.delay, 0.0..=10.0).suffix(" s").step_by(0.5))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("Notifications stay for");
            changed |= ui
                .add(egui::Slider::new(&mut self.settings.general.toast_seconds, 1.0..=15.0).suffix(" s").step_by(0.5))
                .changed();
        });
        if changed {
            self.edited();
        }
    }

    fn upload_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut changed = false;
        // .sxcu files dropped on the window are imported.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for p in dropped {
            self.import(&p);
        }

        ui::section(ui, "Destination");
        ui::hint(ui, "Uploads go here. Import ShareX custom uploaders (.sxcu) to use any host they support.");
        let mut remove = None;
        for d in self.destinations.clone() {
            ui.horizontal(|ui| {
                changed |= ui.radio_value(&mut self.settings.upload.destination, d.id.clone(), &d.name).changed();
                if !d.builtin && ui.small_button("Remove").clicked() {
                    remove = Some(d.id.clone());
                }
            });
        }
        if let Some(id) = remove {
            if let Err(e) = upload::remove(&id, &config::uploaders_dir()) {
                self.upload_msg = Some((e, true));
            }
            if self.settings.upload.destination == id {
                self.settings.upload.destination = "catbox".into();
                changed = true;
            }
            self.destinations = upload::destinations(&config::uploaders_dir());
        }
        ui.horizontal(|ui| {
            if ui.button("Import .sxcu...").clicked()
                && let Some(p) =
                    rfd::FileDialog::new().add_filter("ShareX custom uploader", &["sxcu", "json"]).pick_file()
            {
                self.import(&p);
            }
            ui::hint(ui, "or drop .sxcu files on this window");
        });
        if let Some((msg, err)) = &self.upload_msg {
            ui.label(RichText::new(msg).color(if *err { ui::DANGER } else { ui::OK }));
        }

        ui::section(ui, "Options");
        changed |= ui.checkbox(&mut self.settings.upload.copy_url, "Copy the link after uploading").changed();
        ui.horizontal(|ui| {
            ui.label("Litterbox keeps files for");
            egui::ComboBox::from_id_salt("litterbox-time").selected_text(&self.settings.upload.litterbox_time).show_ui(
                ui,
                |ui| {
                    for t in ["1h", "12h", "24h", "72h"] {
                        changed |=
                            ui.selectable_value(&mut self.settings.upload.litterbox_time, t.to_string(), t).changed();
                    }
                },
            );
        });
        ui.horizontal(|ui| {
            ui.label("Catbox user hash (optional)");
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.settings.upload.catbox_userhash)
                        .desired_width(220.0)
                        .password(true),
                )
                .changed();
        });

        ui::section(ui, "Test");
        ui.horizontal(|ui| {
            let busy = self.test_upload.is_some();
            if ui.add_enabled(!busy, egui::Button::new("Upload a test image")).clicked() {
                let (tx, rx) = mpsc::channel();
                let settings = self.settings.upload.clone();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let img = crate::selftest::synthetic_desktop(320, 200);
                    let r = crate::app::encode(&img, ImageFormat::Png, 90)
                        .and_then(|bytes| upload::upload_with_settings(&settings, "snapsmith-test.png", &bytes))
                        .map(|(_, r)| r.url);
                    let _ = tx.send(r);
                    c.request_repaint();
                });
                self.test_upload = Some(rx);
            }
            if busy {
                ui.add(egui::Spinner::new());
            }
            ui::hint(ui, "Sends a small generated picture to the selected destination.");
        });
        if let Some(rx) = &self.test_upload
            && let Ok(r) = rx.try_recv()
        {
            self.test_upload = None;
            self.upload_msg = Some(match r {
                Ok(url) => (format!("Uploaded: {url}"), false),
                Err(e) => (e, true),
            });
        }
        if changed {
            self.edited();
        }
    }

    fn import(&mut self, path: &Path) {
        match upload::import(path, &config::uploaders_dir()) {
            Ok(id) => {
                self.destinations = upload::destinations(&config::uploaders_dir());
                let name = self.destinations.iter().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or(id.clone());
                self.settings.upload.destination = id;
                self.upload_msg = Some((format!("Added {name} and made it the destination."), false));
                self.edited();
            }
            Err(e) => self.upload_msg = Some((e, true)),
        }
    }

    fn hotkeys_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui::section(ui, "Global shortcuts");
        ui::hint(ui, "They work anywhere while Snapsmith runs in the tray. Click Set, then press the keys.");
        if let Some(note) = self.hotkey_status.as_ref().and_then(|s| s.note.clone()) {
            ui.add_space(4.0);
            ui.label(RichText::new(note).color(Color32::from_rgb(0xF0, 0xB4, 0x3C)));
        }
        ui.add_space(6.0);

        // Capture a key combination while listening.
        if let Some(action) = self.listening {
            let pressed = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some((key, m)) = pressed {
                if key == egui::Key::Escape {
                    self.listening = None;
                } else {
                    let mut parts = Vec::new();
                    if m.ctrl || (m.command && !cfg!(target_os = "macos")) {
                        parts.push("Ctrl");
                    }
                    if m.mac_cmd {
                        parts.push("Cmd");
                    }
                    if m.alt {
                        parts.push("Alt");
                    }
                    if m.shift {
                        parts.push("Shift");
                    }
                    parts.push(key.name());
                    let spec = parts.join("+");
                    if let Some(edit) = self.hotkey_edits.get_mut(&action) {
                        edit.text = spec;
                    }
                    self.listening = None;
                    self.commit_hotkey(action);
                }
            }
        }

        egui::Grid::new("hotkeys").num_columns(4).spacing([12.0, 8.0]).show(ui, |ui| {
            let status = self.hotkey_status.clone().unwrap_or_default();
            for (action, _) in self.settings.hotkeys.clone().bindings() {
                ui.label(action.label());
                let listening = self.listening == Some(action);
                let mut commit = false;
                if let Some(edit) = self.hotkey_edits.get_mut(&action) {
                    let te = egui::TextEdit::singleline(&mut edit.text).desired_width(200.0).hint_text(if listening {
                        "Press keys..."
                    } else {
                        "None"
                    });
                    let r = ui.add_enabled(!listening, te);
                    if r.lost_focus() {
                        commit = true;
                    }
                }
                if commit {
                    self.commit_hotkey(action);
                }
                ui.horizontal(|ui| {
                    if ui.button(if listening { "Press keys" } else { "Set" }).clicked() {
                        self.listening = if listening { None } else { Some(action) };
                    }
                    if ui.button("Clear").clicked() {
                        if let Some(edit) = self.hotkey_edits.get_mut(&action) {
                            edit.text.clear();
                        }
                        self.commit_hotkey(action);
                    }
                });
                let edit_err = self.hotkey_edits.get(&action).and_then(|e| e.error.clone());
                let failed = status.failed.iter().find(|(a, _, _)| *a == action).map(|(_, _, e)| e.clone());
                let ok = status.registered.iter().any(|(a, _)| *a == action);
                match (edit_err, failed) {
                    (Some(e), _) | (None, Some(e)) => {
                        ui.label(RichText::new(e).color(ui::DANGER).size(12.0));
                    }
                    (None, None) if ok => {
                        ui.label(RichText::new("Active").color(ui::OK).size(12.0));
                    }
                    _ => {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });
        ui.add_space(10.0);
        ui::hint(
            ui,
            "In the selection: R E A L P H T N X B pick tools, V adjusts the selection, Ctrl+C copies, Ctrl+S saves,",
        );
        ui::hint(
            ui,
            "Ctrl+U uploads, Ctrl+T reads text, Ctrl+P pins, arrows nudge (Shift: 10 px, Alt: resize), Esc cancels.",
        );
        if crate::platform::is_wayland() {
            ui.add_space(6.0);
            ui::hint(
                ui,
                "On Wayland, bind commands such as `snapsmith capture region` in your desktop's keyboard settings.",
            );
        }
    }

    fn commit_hotkey(&mut self, action: HotkeyAction) {
        let Some(edit) = self.hotkey_edits.get_mut(&action) else { return };
        match hotkey::parse(&edit.text) {
            Ok(_) => {
                edit.error = None;
                let text = edit.text.trim().to_string();
                let clash = self
                    .settings
                    .hotkeys
                    .bindings()
                    .iter()
                    .any(|(a, s)| *a != action && !text.is_empty() && s.trim().eq_ignore_ascii_case(&text));
                if clash {
                    edit.error = Some("Already used by another action".into());
                    return;
                }
                *self.settings.hotkeys.get_mut(action) = text;
                self.edited();
            }
            Err(e) => edit.error = Some(format!("Not a valid shortcut: {e}")),
        }
    }

    fn recording_page(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui::section(ui, "GIF");
        ui.horizontal(|ui| {
            ui.label("Frames per second");
            changed |= ui.add(egui::Slider::new(&mut self.settings.record.gif_fps, 5..=30)).changed();
        });
        ui::section(ui, "Video (MP4)");
        ui.horizontal(|ui| {
            ui.label("Frames per second");
            changed |= ui.add(egui::Slider::new(&mut self.settings.record.video_fps, 10..=60)).changed();
        });
        ui.horizontal(|ui| {
            ui.label("ffmpeg");
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.settings.record.ffmpeg)
                        .hint_text("found on PATH")
                        .desired_width(300.0),
                )
                .changed();
            if ui.button("Browse...").clicked()
                && let Some(p) = rfd::FileDialog::new().pick_file()
            {
                self.settings.record.ffmpeg = p.to_string_lossy().into_owned();
                changed = true;
            }
            if ui.button("Check").clicked() {
                self.ffmpeg_status = Some(crate::record::find_ffmpeg(&self.settings));
            }
        });
        match &self.ffmpeg_status {
            Some(Ok(p)) => {
                ui.label(RichText::new(format!("Using {}", p.display())).color(ui::OK).size(12.0));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(ui::DANGER).size(12.0));
            }
            None => ui::hint(ui, "MP4 recording uses ffmpeg, which isn't bundled. GIF recording needs nothing extra."),
        }
        ui::section(ui, "Both");
        ui.horizontal(|ui| {
            ui.label("Stop automatically after");
            changed |= ui
                .add(egui::Slider::new(&mut self.settings.record.max_seconds, 5..=1800).suffix(" s").logarithmic(true))
                .changed();
        });
        changed |= ui.checkbox(&mut self.settings.record.show_cursor, "Show the mouse pointer").changed();
        ui.add_space(6.0);
        ui::hint(
            ui,
            "Recordings are saved next to your screenshots. If uploading is on in Capture, they're uploaded too.",
        );
        if crate::platform::is_wayland() {
            ui::hint(ui, "Recording isn't available on Wayland yet.");
        }
        if changed {
            self.edited();
        }
    }

    fn general_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui::section(ui, "Startup");
        if let Some(l) = &self.autostart {
            if ui.checkbox(&mut self.autostart_on, "Start Snapsmith when I log in").changed() {
                autostart::set_enabled(l, self.autostart_on);
                self.autostart_on = autostart::is_enabled(l);
            }
        } else {
            ui::hint(ui, "Start at login isn't available on this system.");
        }

        ui::section(ui, "Updates");
        ui.horizontal(|ui| {
            if ui.add_enabled(self.update_rx.is_none(), egui::Button::new("Check for updates")).clicked() {
                let (tx, rx) = mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(updater::check());
                    c.request_repaint();
                });
                self.update_rx = Some(rx);
                self.update_status = "Checking...".into();
            }
            if let Some(u) = self.update.clone()
                && ui.button(format!("Install {}", u.version)).clicked()
            {
                self.update_status = match updater::install(&u) {
                    Ok(()) => {
                        // Restart the background process on the new version.
                        let _ = std::fs::write(config::config_dir().join(daemon::RESTART_FILE), b"");
                        format!("Installed {}. Reopen this window to use it.", u.version)
                    }
                    Err(e) => e,
                };
                self.update = None;
            }
            ui.label(&self.update_status);
        });
        if let Some(rx) = &self.update_rx
            && let Ok(r) = rx.try_recv()
        {
            self.update_rx = None;
            match r {
                Ok(None) => self.update_status = "Snapsmith is up to date.".into(),
                Ok(Some(u)) => {
                    self.update_status = format!("Version {} is available.", u.version);
                    self.update = Some(u);
                }
                Err(e) => self.update_status = e,
            }
        }

        ui::section(ui, "Files");
        egui::Grid::new("paths").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            let rows = [
                ("Settings", self.config_path.clone()),
                ("Custom uploaders", config::uploaders_dir()),
                ("History and OCR models", config::data_dir()),
                ("Log", config::config_dir().join("snapsmith.log")),
            ];
            for (label, path) in rows {
                ui.label(label);
                ui.label(RichText::new(path.to_string_lossy()).color(ui::MUTED).size(12.0));
                if ui.small_button("Open").clicked() {
                    if path.is_dir() || path.extension().is_some() && path.exists() {
                        crate::app::open(&path.to_string_lossy());
                    } else {
                        let _ = std::fs::create_dir_all(&path);
                        crate::app::open(&path.to_string_lossy());
                    }
                }
                ui.end_row();
            }
        });

        ui::section(ui, "About");
        ui.label(format!("Snapsmith {}  ·  MIT license", env!("CARGO_PKG_VERSION")));
        ui.hyperlink_to("github.com/fsiuhgshiqoih32/snapsmith", "https://github.com/fsiuhgshiqoih32/snapsmith");
    }
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif"))
}

fn file_kind(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_else(|| "FILE".into())
}

impl eframe::App for MainWindow {
    /// Runs while the window is hidden for a capture, too.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        self.run_pending();
        self.standby.tick();
        self.check_running(ctx);
        if self.pending_capture.is_some() || self.running.is_some() {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        if let Some(action) = std::env::var("SNAPSMITH_TEST_CLICK").ok().filter(|_| self.frame_no == 15) {
            // For timing tests: press a capture button without a user.
            self.frame_no += 1;
            let args: Vec<&'static str> = match action.as_str() {
                "record-gif" => vec!["record", "--gif"],
                "region" => vec!["capture", "region"],
                _ => Vec::new(),
            };
            if !args.is_empty() {
                crate::log::timing("main window: capture button");
                self.start_capture(ctx, args);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame_no += 1;

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(6.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::left("nav").resizable(false).default_size(150.0).show(ui, |ui| self.nav(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(self.page.label()).auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(4.0);
                match self.page {
                    Page::History => self.history_page(ui),
                    Page::Capture => self.capture_page(ui),
                    Page::Upload => self.upload_page(ui),
                    Page::Hotkeys => self.hotkeys_page(ui),
                    Page::Recording => self.recording_page(ui),
                    Page::General => self.general_page(ui),
                }
            });
        });
        if let Some((msg, t)) = &self.toast {
            let now = ctx.input(|i| i.time);
            if now - t < 2.5 {
                let screen = ctx.viewport_rect();
                egui::Area::new(egui::Id::unique("main-toast"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(screen.center().x - 80.0, screen.max.y - 48.0))
                    .show(&ctx, |ui| {
                        egui::Frame::new()
                            .fill(ui::PANEL)
                            .corner_radius(6)
                            .inner_margin(egui::Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                ui.label(msg.as_str());
                            });
                    });
                ctx.request_repaint_after(Duration::from_millis(300));
            } else {
                self.toast = None;
            }
        }
        if self.pending_capture.is_some() || self.running.is_some() || self.test_upload.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        } else {
            ctx.request_repaint_after(Duration::from_millis(800));
        }

        if let Some(path) = self.demo.clone()
            && self.frame_no == 30
        {
            let c = ctx.clone();
            ctx.request_screenshot(move |img| {
                if let Err(e) = crate::app::save_color_image(&img, &path) {
                    eprintln!("could not save screenshot: {e}");
                }
                c.send_viewport_cmd(egui::ViewportCommand::Close);
            });
        }
        if self.demo.is_some() {
            ctx.request_repaint();
        }
    }
}

/// Sample history for the README screenshot, with generated thumbnails.
fn demo_history() -> Vec<Entry> {
    let dir = std::env::temp_dir().join("snapsmith-demo");
    let _ = std::fs::create_dir_all(&dir);
    let desk = crate::selftest::synthetic_desktop(1280, 800);
    let items = [
        (
            "Snapsmith_2026-10-17_14-02-11.png",
            Some("https://files.catbox.moe/q3k9xa.png"),
            Some("Catbox"),
            (100, 80, 700, 480),
        ),
        ("Snapsmith_2026-10-17_13-48-55.png", None, None, (700, 340, 520, 360)),
        (
            "Snapsmith_rec_2026-10-17_13-30-02.gif",
            Some("https://litter.catbox.moe/8f2kz1.gif"),
            Some("Litterbox"),
            (60, 60, 900, 600),
        ),
        ("Snapsmith_2026-10-16_18-21-40.png", None, None, (0, 0, 1280, 800)),
    ];
    let mut out = Vec::new();
    for (i, (name, url, uploader, (x, y, w, h))) in items.iter().enumerate() {
        let path = dir.join(name);
        let crop = image::imageops::crop_imm(&desk, *x, *y, *w, *h).to_image();
        let _ = crop.save(&path);
        out.push(Entry {
            time: chrono::Utc::now().timestamp_millis() - (i as i64) * 900_000,
            kind: if name.ends_with(".gif") { "file".into() } else { "image".into() },
            path: Some(path),
            url: url.map(str::to_string),
            uploader: uploader.map(str::to_string),
            width: *w,
            height: *h,
            ..Default::default()
        });
    }
    out
}
