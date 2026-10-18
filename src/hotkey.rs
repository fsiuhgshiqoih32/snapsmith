//! Global shortcuts. Registration failures (a key another app already owns, or Wayland, where
//! apps can't grab keys) are kept so the main window can explain them.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyManager, HotKeyState};
use serde::{Deserialize, Serialize};

use snapsmith::config::{self, HotkeyAction};

use crate::log::warning;

/// Written by the background process for the main window: which shortcuts work.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Status {
    pub registered: Vec<(HotkeyAction, String)>,
    pub failed: Vec<(HotkeyAction, String, String)>,
    pub note: Option<String>,
}

pub fn status_path() -> std::path::PathBuf {
    config::config_dir().join("hotkeys.json")
}

pub fn read_status() -> Option<Status> {
    serde_json::from_str(&std::fs::read_to_string(status_path()).ok()?).ok()
}

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<(HotKey, HotkeyAction, String)>,
    pub status: Status,
}

/// Parses "Ctrl+Shift+PrintScreen" and friends; empty means "no shortcut".
pub fn parse(spec: &str) -> Result<Option<HotKey>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(None);
    }
    spec.parse::<HotKey>().map(Some).map_err(|e| e.to_string())
}

impl Hotkeys {
    pub fn new() -> Self {
        let mut status = Status::default();
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => Some(m),
            Err(e) => {
                warning!("global hotkeys unavailable: {e}");
                status.note = Some(if crate::platform::is_wayland() {
                    "Wayland doesn't let apps register global shortcuts. Bind `snapsmith capture region` (and the \
                     other commands) to keys in your desktop's keyboard settings instead."
                        .into()
                } else {
                    format!("Global shortcuts are unavailable: {e}")
                });
                None
            }
        };
        Hotkeys { manager, registered: Vec::new(), status }
    }

    /// Replaces every registration with the ones in `keys`.
    pub fn apply(&mut self, keys: &config::Hotkeys) {
        let Some(manager) = &self.manager else {
            self.write_status();
            return;
        };
        for (hk, _, _) in self.registered.drain(..) {
            let _ = manager.unregister(hk);
        }
        self.status.registered.clear();
        self.status.failed.clear();
        for (action, spec) in keys.bindings() {
            match parse(spec) {
                Ok(None) => {}
                Ok(Some(hk)) => match manager.register(hk) {
                    Ok(()) => {
                        self.registered.push((hk, action, spec.to_string()));
                        self.status.registered.push((action, spec.to_string()));
                    }
                    Err(e) => {
                        warning!("could not register {spec} for {}: {e}", action.label());
                        self.status.failed.push((action, spec.to_string(), friendly_error(spec, &e.to_string())));
                    }
                },
                Err(e) => self.status.failed.push((action, spec.to_string(), format!("not a valid shortcut: {e}"))),
            }
        }
        // Registration of plain Print Screen succeeds even when Windows hands the key to the
        // Snipping Tool, but the presses never arrive. Say so.
        let plain_print = self.status.registered.iter().any(|(_, s)| s.trim().eq_ignore_ascii_case("printscreen"));
        self.status.note = if plain_print && crate::platform::print_screen_taken() {
            Some(
                "Windows is set to open the Snipping Tool with Print Screen, so Snapsmith won't see that key. \
                 Turn off \"Use the Print screen key to open screen capture\" in Settings > Accessibility > \
                 Keyboard, or choose another shortcut."
                    .into(),
            )
        } else {
            self.status.note.take().filter(|n| n.contains("Wayland") || n.contains("unavailable"))
        };
        self.write_status();
    }

    fn write_status(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.status) {
            let _ = std::fs::write(status_path(), text);
        }
    }

    /// The action for a hotkey event, if it's a press of one of ours.
    pub fn action_for(&self, id: u32, state: HotKeyState) -> Option<HotkeyAction> {
        if state != HotKeyState::Pressed {
            return None;
        }
        self.registered.iter().find(|(hk, _, _)| hk.id() == id).map(|(_, a, _)| *a)
    }

    pub fn label(&self, action: HotkeyAction) -> Option<&str> {
        self.registered.iter().find(|(_, a, _)| *a == action).map(|(_, _, s)| s.as_str())
    }
}

fn friendly_error(spec: &str, e: &str) -> String {
    let print = spec.to_ascii_lowercase().contains("printscreen");
    if cfg!(windows) && print && crate::platform::print_screen_taken() {
        "Windows gives Print Screen to the Snipping Tool. Turn off \"Use the Print screen key to open screen capture\" \
         in Settings > Accessibility > Keyboard, or pick another shortcut."
            .into()
    } else if e.to_ascii_lowercase().contains("already") || e.contains("BadAccess") {
        "Another app already uses this shortcut.".into()
    } else {
        e.to_string()
    }
}
