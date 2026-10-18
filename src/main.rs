// Release builds on Windows are GUI apps (no console window); see platform::windows::init_process.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod art;
mod autostart;
mod capture;
mod clipboard;
mod commands;
mod daemon;
mod editor_window;
mod hotkey;
mod log;
mod main_window;
mod ocr_window;
mod overlay;
mod pin;
mod platform;
mod record;
mod selftest;
mod single_instance;
mod standby;
mod tasks;
mod toast;
mod tray;
mod ui;
mod updater;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use snapsmith::config::{self, Task};

use crate::commands::{CaptureKind, CaptureOptions};
use crate::log::error;
use crate::platform::DesktopRect;

#[derive(Parser)]
#[command(name = "snapsmith", version, about = "Screenshots, annotation, uploads, text recognition and GIF recording.")]
#[command(after_help = "With no command, opens the Snapsmith window and starts the tray icon.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Run the built-in checks (capture, overlay, editor export, GIF) and exit.
    #[arg(long, hide = true)]
    self_test: bool,
    /// Write README screenshots of the UI into this folder.
    #[arg(long, hide = true, value_name = "DIR")]
    demo_shots: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Take a screenshot.
    Capture {
        #[arg(value_enum, default_value = "region")]
        kind: CaptureKind,
        /// Wait this many seconds first.
        #[arg(long)]
        delay: Option<f32>,
        /// Comma-separated tasks instead of the configured ones: copy, save, upload, ocr, pin, toast.
        #[arg(long, value_delimiter = ',', value_parser = parse_task)]
        tasks: Option<Vec<Task>>,
        /// Save to this file and print its path.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Record an area as a GIF or MP4. Run it again (or press the hotkey) to stop.
    Record {
        /// Record a GIF (the default).
        #[arg(long, conflicts_with = "mp4")]
        gif: bool,
        /// Record an MP4 (needs ffmpeg).
        #[arg(long)]
        mp4: bool,
        /// Stop the recording in progress.
        #[arg(long)]
        stop: bool,
    },
    /// Copy text from an image file, or from an area of the screen.
    Ocr { file: Option<PathBuf> },
    /// Open an image in the editor.
    Edit { file: PathBuf },
    /// Upload a file to the configured destination and print the link.
    Upload {
        file: PathBuf,
        /// No notification window.
        #[arg(long)]
        quiet: bool,
    },
    /// Show an image in a window that stays on top.
    Pin {
        file: PathBuf,
        /// Position: x,y,width,height in desktop coordinates.
        #[arg(long, value_parser = parse_rect)]
        at: Option<DesktopRect>,
        /// Delete the file once it's loaded.
        #[arg(long)]
        delete: bool,
    },
    /// Add a ShareX custom uploader (.sxcu).
    ImportUploader { file: PathBuf },
    /// Start the tray icon and global hotkeys (what runs at login).
    Daemon,
    /// Open the Snapsmith window.
    Window {
        /// Open on this page: history, capture, upload, hotkeys, recording, general.
        #[arg(long)]
        page: Option<String>,
    },
    /// Check for a newer release and install it.
    Update {
        /// Only check, don't install.
        #[arg(long)]
        check: bool,
    },
    /// Print where the settings file is.
    ConfigPath,
    #[command(hide = true)]
    Standby,
    #[command(hide = true)]
    Clipboard {
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(long)]
        text: bool,
    },
}

fn parse_task(s: &str) -> Result<Task, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "copy" => Ok(Task::Copy),
        "save" => Ok(Task::Save),
        "upload" => Ok(Task::Upload),
        "ocr" => Ok(Task::Ocr),
        "pin" => Ok(Task::Pin),
        "toast" | "notify" => Ok(Task::Toast),
        other => Err(format!("unknown task {other:?}; use copy, save, upload, ocr, pin or toast")),
    }
}

fn parse_rect(s: &str) -> Result<DesktopRect, String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    let bad = || format!("expected x,y,width,height, got {s:?}");
    if parts.len() != 4 {
        return Err(bad());
    }
    Ok(DesktopRect {
        x: parts[0].parse().map_err(|_| bad())?,
        y: parts[1].parse().map_err(|_| bad())?,
        w: parts[2].parse().map_err(|_| bad())?,
        h: parts[3].parse().map_err(|_| bad())?,
    })
}

fn main() {
    log::timing("start");
    platform::init_process();
    let cli = Cli::parse();
    // Commands started from the tray or a shortcut have no terminal: show their errors.
    let windowed = !cli.self_test
        && cli.demo_shots.is_none()
        && matches!(
            cli.command,
            None | Some(
                Command::Capture { .. }
                    | Command::Standby
                    | Command::Record { .. }
                    | Command::Ocr { .. }
                    | Command::Edit { .. }
                    | Command::Pin { .. }
                    | Command::Window { .. }
            )
        );
    let result = if cli.self_test {
        selftest::run()
    } else if let Some(dir) = &cli.demo_shots {
        selftest::demo_shots(dir)
    } else {
        match cli.command {
            None => main_window::open(None),
            Some(Command::Window { page }) => main_window::open(page),
            Some(Command::Daemon) => daemon::run(),
            Some(Command::Capture { kind, delay, tasks, output }) => {
                commands::capture(kind, CaptureOptions { delay, tasks, output })
            }
            Some(Command::Record { mp4, stop, .. }) => record::command(mp4, stop),
            Some(Command::Ocr { file }) => commands::ocr(file.as_deref()),
            Some(Command::Edit { file }) => commands::edit(&file),
            Some(Command::Upload { file, quiet }) => commands::upload(&file, quiet),
            Some(Command::Pin { file, at, delete }) => pin::run(&file, at, delete),
            Some(Command::ImportUploader { file }) => {
                snapsmith::upload::import(&file, &config::uploaders_dir()).map(|id| println!("added uploader {id:?}"))
            }
            Some(Command::Update { check }) => update(check),
            Some(Command::ConfigPath) => {
                println!("{}", config::default_path().display());
                Ok(())
            }
            Some(Command::Clipboard { image, text }) => clipboard::hold(image.as_deref(), text),
            Some(Command::Standby) => standby::run(),
        }
    };
    if let Err(e) = result {
        error!("{e}");
        eprintln!("snapsmith: {e}");
        if windowed {
            commands::show_errors("Snapsmith", &[e], platform::cursor_pos(), &commands::load_settings());
        }
        std::process::exit(1);
    }
}

fn update(check_only: bool) -> Result<(), String> {
    match updater::check()? {
        None => {
            println!("snapsmith {} is up to date", updater::current_version());
            Ok(())
        }
        Some(u) if check_only => {
            println!("snapsmith {} is available (this is {})", u.version, updater::current_version());
            Ok(())
        }
        Some(u) => {
            updater::install(&u)?;
            println!("updated to {}; restart Snapsmith to use it", u.version);
            Ok(())
        }
    }
}
