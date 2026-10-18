//! Per-user locks: one background process (two would register the same hotkeys twice), one main
//! window, one recording at a time.

use std::fs::{File, OpenOptions, TryLockError};
use std::time::Duration;

/// Held by the background process (tray and hotkeys).
pub const DAEMON: &str = "daemon.lock";
/// Held by the open main window.
pub const WINDOW: &str = "window.lock";
/// Held while a screen recording runs.
pub const RECORDING: &str = "recording.lock";
/// Held while a region overlay is open, so a second hotkey press doesn't stack another one.
pub const OVERLAY: &str = "overlay.lock";

/// Held for the life of the process; the OS releases the lock when it exits.
pub struct InstanceLock(#[allow(dead_code)] File);

/// `Ok(None)` means another process already holds the lock.
pub fn acquire(name: &str) -> Result<Option<InstanceLock>, String> {
    let dir = snapsmith::config::config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file =
        OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(name)).map_err(|e| e.to_string())?;
    match file.try_lock() {
        Ok(()) => Ok(Some(InstanceLock(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e.to_string()),
    }
}

/// Like [`acquire`], but keeps trying for a few seconds, for a restart where the old process is
/// still shutting down.
pub fn acquire_patiently(name: &str) -> Result<Option<InstanceLock>, String> {
    for _ in 0..25 {
        if let Some(lock) = acquire(name)? {
            return Ok(Some(lock));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(None)
}

/// True if another process holds the lock.
pub fn is_held(name: &str) -> bool {
    matches!(acquire(name), Ok(None))
}
