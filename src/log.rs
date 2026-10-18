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

pub(crate) use {error, info, warning};
