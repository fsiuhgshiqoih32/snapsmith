//! Text recognition: asks before downloading the models the first time, then shows the text
//! (already copied) in an editable box.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, RichText, vec2};
use image::RgbaImage;

use snapsmith::ocr::{self, Ocr};

use crate::ui;

enum State {
    AskDownload,
    Downloading { done: Arc<AtomicU64>, rx: Receiver<Result<(), String>> },
    Reading { rx: Receiver<Result<String, String>> },
    Result { text: String, copied: bool },
    Failed(String),
}

pub fn run(image: RgbaImage) -> Result<(), String> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Snapsmith - Text")
        .with_app_id("snapsmith")
        .with_inner_size(vec2(520.0, 360.0))
        .with_min_inner_size(vec2(360.0, 220.0))
        .with_always_on_top();
    let image = Arc::new(image);
    crate::app::run_window("Snapsmith - Text", viewport, crate::app::Kind::Accessory, |cc| {
        let mut app = OcrApp { image: image.clone(), state: State::AskDownload, frame_no: 0 };
        if ocr::models_present(&ocr::models_dir()) {
            app.start_reading(&cc.egui_ctx);
        }
        app
    })
    .map_err(|e| format!("could not open the text window: {e}"))
}

struct OcrApp {
    image: Arc<RgbaImage>,
    state: State,
    frame_no: u64,
}

impl OcrApp {
    fn start_download(&mut self, ctx: &egui::Context) {
        let done = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let (d, c) = (done.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = ocr::download_models(&ocr::models_dir(), |n, _| {
                d.store(n, Ordering::Relaxed);
                c.request_repaint();
            });
            let _ = tx.send(r);
            c.request_repaint();
        });
        self.state = State::Downloading { done, rx };
    }

    fn start_reading(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let (img, c) = (self.image.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = Ocr::load(&ocr::models_dir()).and_then(|o| o.recognize(&img));
            let _ = tx.send(r);
            c.request_repaint();
        });
        self.state = State::Reading { rx };
    }
}

impl eframe::App for OcrApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame_no += 1;
        if !matches!(self.state, State::Reading { .. } | State::Downloading { .. }) {
            crate::app::shot_hook(&ctx, self.frame_no, "ocr");
        } else {
            self.frame_no = self.frame_no.min(10);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Advance background work.
        let mut next = None;
        match &self.state {
            State::Downloading { rx, .. } => match rx.try_recv() {
                Ok(Ok(())) => next = Some(true),
                Ok(Err(e)) => self.state = State::Failed(e),
                Err(_) => {}
            },
            State::Reading { rx } => match rx.try_recv() {
                Ok(Ok(text)) => {
                    let copied = !text.is_empty() && crate::clipboard::try_copy_text(&text).is_ok();
                    self.state = State::Result { text, copied };
                }
                Ok(Err(e)) => self.state = State::Failed(e),
                Err(_) => {}
            },
            _ => {}
        }
        if next.is_some() {
            self.start_reading(&ctx);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(4.0);
            match &mut self.state {
                State::AskDownload => {
                    ui.label(RichText::new("Text recognition needs a one-time download").strong().size(15.0));
                    ui.add_space(6.0);
                    ui.label(format!(
                        "Snapsmith reads text with the open-source ocrs engine. Its two model files ({:.1} MB) \
                         come from ocrs-models.s3-accelerate.amazonaws.com and are stored in {}.",
                        ocr::total_download_size() as f64 / 1_000_000.0,
                        ocr::models_dir().display()
                    ));
                    ui.add_space(4.0);
                    ui::hint(ui, "Recognition then runs entirely on this computer. Latin script only for now.");
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Download").clicked() {
                            self.start_download(&ctx);
                        }
                        if ui.button("Cancel").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
                State::Downloading { done, .. } => {
                    let total = ocr::total_download_size() as f32;
                    let d = done.load(Ordering::Relaxed) as f32;
                    ui.label("Downloading the text recognition models...");
                    ui.add(egui::ProgressBar::new(d / total).show_percentage());
                }
                State::Reading { .. } => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label("Reading text...");
                    });
                }
                State::Result { text, copied } => {
                    if text.is_empty() {
                        ui.label("No text found in the selection.");
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(if *copied { "Copied to the clipboard." } else { "Recognised text:" });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Close").clicked() {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                                if ui.button("Copy").clicked() {
                                    *copied = crate::clipboard::try_copy_text(text).is_ok();
                                }
                            });
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.add_sized(
                                ui.available_size(),
                                egui::TextEdit::multiline(text).font(egui::TextStyle::Monospace),
                            );
                        });
                    }
                }
                State::Failed(e) => {
                    ui.label(RichText::new("Text recognition failed").strong());
                    ui.label(RichText::new(e.as_str()).color(ui::DANGER));
                }
            }
        });
    }
}
