//! What happens to a finished capture: save, copy, upload, record in history. Runs on a worker
//! thread and reports progress to the notification window.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use image::RgbaImage;

use snapsmith::config::{Settings, Task};
use snapsmith::history::{self, Entry};
use snapsmith::naming::{self, NameContext};
use snapsmith::upload;

use crate::clipboard;

#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    Saved(PathBuf),
    Copied(&'static str),
    Uploading(String),
    Uploaded { url: String, deletion_url: Option<String> },
    Failed(String),
    Done,
}

pub struct Job {
    pub image: RgbaImage,
    pub tasks: Vec<Task>,
    /// Window title, for `%t` in file names.
    pub title: String,
    /// Save exactly here instead of using the file name pattern.
    pub save_to: Option<PathBuf>,
}

/// The path the pattern gives for an image of this size, made unique.
pub fn planned_path(settings: &Settings, width: u32, height: u32, title: &str) -> PathBuf {
    let ctx = NameContext { width, height, title: title.to_string() };
    let expanded = naming::expand(&settings.capture.file_pattern, &ctx);
    naming::unique(naming::build_path(&settings.capture.save_dir(), &expanded, settings.capture.format.extension()))
}

/// Runs the job, sending progress to `tx` (ignored if nobody listens). Returns what happened.
pub fn run(job: Job, settings: &Settings, tx: &Sender<Progress>) -> Outcome {
    let mut out = Outcome::default();
    let send = |p: Progress| {
        let _ = tx.send(p);
    };
    let wants = |t: Task| job.tasks.contains(&t);
    let format = match &job.save_to {
        Some(p) if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg")) => {
            snapsmith::config::ImageFormat::Jpg
        }
        Some(_) => snapsmith::config::ImageFormat::Png,
        None => settings.capture.format,
    };

    let mut bytes = None;
    if wants(Task::Save) || wants(Task::Upload) || job.save_to.is_some() {
        match crate::app::encode(&job.image, format, settings.capture.jpeg_quality) {
            Ok(b) => bytes = Some(b),
            Err(e) => {
                out.fail(format!("could not encode the image: {e}"), &send);
            }
        }
    }

    if let Some(b) = &bytes
        && (wants(Task::Save) || job.save_to.is_some())
    {
        let path = job
            .save_to
            .clone()
            .unwrap_or_else(|| planned_path(settings, job.image.width(), job.image.height(), &job.title));
        let result = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, b));
        match result {
            Ok(()) => {
                out.saved = Some(path.clone());
                send(Progress::Saved(path));
            }
            Err(e) => out.fail(format!("could not save to {}: {e}", path.display()), &send),
        }
    }

    if wants(Task::Copy) {
        match clipboard::copy_image(&job.image) {
            Ok(()) => send(Progress::Copied("Image")),
            Err(e) => out.fail(e, &send),
        }
    }

    if let Some(b) = &bytes
        && wants(Task::Upload)
    {
        let filename =
            out.saved.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(
                || {
                    let ctx =
                        NameContext { width: job.image.width(), height: job.image.height(), title: job.title.clone() };
                    let stem = naming::expand(&settings.capture.file_pattern, &ctx);
                    let stem = stem.rsplit(['/', '\\']).next().unwrap_or("Snapsmith").to_string();
                    format!("{}.{}", naming::sanitize_component(&stem), format.extension())
                },
            );
        let dest = upload::resolve(&settings.upload.destination, &settings.upload, &upload::uploaders_dir());
        match dest {
            Err(e) => out.fail(e, &send),
            Ok(uploader) => {
                send(Progress::Uploading(uploader.name.clone()));
                match upload::upload(&uploader, &upload::Payload { filename: &filename, data: b }) {
                    Ok(r) => {
                        out.uploader = Some(uploader.name.clone());
                        out.url = Some(r.url.clone());
                        out.deletion_url = r.deletion_url.clone();
                        out.thumbnail_url = r.thumbnail_url.clone();
                        send(Progress::Uploaded { url: r.url.clone(), deletion_url: r.deletion_url.clone() });
                        if settings.upload.copy_url {
                            match clipboard::try_copy_text(&r.url) {
                                Ok(()) => send(Progress::Copied("Link")),
                                Err(e) => out.fail(e, &send),
                            }
                        }
                    }
                    Err(e) => out.fail(e, &send),
                }
            }
        }
    }

    if out.saved.is_some() || out.url.is_some() {
        let mut entry = Entry::now("image");
        entry.path = out.saved.clone();
        entry.url = out.url.clone();
        entry.deletion_url = out.deletion_url.clone();
        entry.thumbnail_url = out.thumbnail_url.clone();
        entry.uploader = out.uploader.clone();
        entry.width = job.image.width();
        entry.height = job.image.height();
        if let Err(e) = history::append(&history::default_path(), &entry) {
            crate::log::warning!("could not write history: {e}");
        }
    }
    send(Progress::Done);
    out
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub saved: Option<PathBuf>,
    pub url: Option<String>,
    pub deletion_url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub uploader: Option<String>,
    pub errors: Vec<String>,
}

impl Outcome {
    fn fail(&mut self, e: String, send: &impl Fn(Progress)) {
        crate::log::warning!("{e}");
        self.errors.push(e.clone());
        send(Progress::Failed(e));
    }
}

/// Uploads any file (the `upload` command and the main window's "Upload file").
pub fn upload_file(settings: &Settings, path: &std::path::Path, tx: &Sender<Progress>) -> Outcome {
    let mut out = Outcome::default();
    let send = |p: Progress| {
        let _ = tx.send(p);
    };
    let dest = upload::resolve(&settings.upload.destination, &settings.upload, &upload::uploaders_dir());
    match dest.and_then(|u| {
        send(Progress::Uploading(u.name.clone()));
        let name = u.name.clone();
        let data = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        upload::upload(&u, &upload::Payload { filename: &file_name, data: &data }).map(|r| (name, r))
    }) {
        Ok((name, r)) => {
            send(Progress::Uploaded { url: r.url.clone(), deletion_url: r.deletion_url.clone() });
            if settings.upload.copy_url {
                match clipboard::try_copy_text(&r.url) {
                    Ok(()) => send(Progress::Copied("Link")),
                    Err(e) => out.fail(e, &send),
                }
            }
            let mut entry = Entry::now("file");
            entry.path = Some(path.to_path_buf());
            entry.url = Some(r.url.clone());
            entry.deletion_url = r.deletion_url.clone();
            entry.uploader = Some(name.clone());
            let _ = history::append(&history::default_path(), &entry);
            out.url = Some(r.url);
            out.deletion_url = r.deletion_url;
            out.uploader = Some(name);
        }
        Err(e) => out.fail(e, &send),
    }
    send(Progress::Done);
    out
}
