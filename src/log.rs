//! Minimal logging to stderr and a small rotating file in the config directory.

use std::fmt::Arguments;
use std::io::Write;
use std::sync::Mutex;

const MAX_LOG_BYTES: u64 = 256 * 1024;

pub fn write(level: &str, args: Arguments<'_>) {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock();
    let line = format!("{level}: {args}");
    eprintln!("snapsmith: {line}");
    let dir = snapsmith::config::config_dir();
    let path = dir.join("snapsmith.log");
    if std::fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("snapsmith.old.log"));
    }
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{now}] [{}] {line}", std::process::id());
    }
}

macro_rules! warning {
    ($($arg:tt)*) => { $crate::log::write("warning", format_args!($($arg)*)) };
}

macro_rules! info {
    ($($arg:tt)*) => { $crate::log::write("info", format_args!($($arg)*)) };
}

macro_rules! error {
    ($($arg:tt)*) => { $crate::log::write("error", format_args!($($arg)*)) };
}

static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// With `SNAPSMITH_TIMING` set, prints how long after start-up each step happened, along with
/// the libraries' own log lines (eframe, wgpu) at the level in `SNAPSMITH_TIMING` (default info).
pub fn timing(step: &str) {
    let start = *START.get_or_init(std::time::Instant::now);
    if let Some(level) = std::env::var_os("SNAPSMITH_TIMING") {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let filter = match level.to_string_lossy().to_ascii_lowercase().as_str() {
                "trace" => ::log::LevelFilter::Trace,
                "debug" => ::log::LevelFilter::Debug,
                _ => ::log::LevelFilter::Info,
            };
            if ::log::set_logger(&TimingLogger).is_ok() {
                ::log::set_max_level(filter);
            }
        });
        eprintln!("timing {:>7.1} ms  {step}", start.elapsed().as_secs_f64() * 1000.0);
    }
}

struct TimingLogger;

impl ::log::Log for TimingLogger {
    fn enabled(&self, _: &::log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &::log::Record<'_>) {
        let start = *START.get_or_init(std::time::Instant::now);
        eprintln!(
            "timing {:>7.1} ms    [{} {}] {}",
            start.elapsed().as_secs_f64() * 1000.0,
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {}
}

pub(crate) use {error, info, warning};
