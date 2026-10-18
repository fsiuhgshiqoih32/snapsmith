//! The tray (menu bar) icon and its menu.

use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use snapsmith::config::HotkeyAction;

use crate::art;
use crate::hotkey::Hotkeys;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Run(HotkeyAction),
    StopRecording,
    OpenWindow,
    OpenFolder,
    ToggleAutostart,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    actions: Vec<(MenuItem, HotkeyAction)>,
    stop: MenuItem,
    window: MenuItem,
    folder: MenuItem,
    autostart: CheckMenuItem,
    quit: MenuItem,
    recording_shown: bool,
}

const ENTRIES: [HotkeyAction; 7] = [
    HotkeyAction::Region,
    HotkeyAction::Window,
    HotkeyAction::Screen,
    HotkeyAction::Monitor,
    HotkeyAction::Ocr,
    HotkeyAction::RecordGif,
    HotkeyAction::RecordVideo,
];

fn label(action: HotkeyAction, keys: &Hotkeys) -> String {
    match keys.label(action) {
        Some(k) => format!("{}\t{k}", action.label()),
        None => action.label().to_string(),
    }
}

impl Tray {
    pub fn new(keys: &Hotkeys, autostart: Option<bool>) -> Result<Self, String> {
        let actions: Vec<(MenuItem, HotkeyAction)> =
            ENTRIES.iter().map(|a| (MenuItem::new(label(*a, keys), true, None), *a)).collect();
        let stop = MenuItem::new("Stop recording", false, None);
        let window = MenuItem::new("Open Snapsmith", true, None);
        let folder = MenuItem::new("Screenshots folder", true, None);
        let autostart_item =
            CheckMenuItem::new("Start at login", autostart.is_some(), autostart.unwrap_or(false), None);
        let quit = MenuItem::new("Quit Snapsmith", true, None);

        let menu = Menu::new();
        let sep1 = PredefinedMenuItem::separator();
        let sep2 = PredefinedMenuItem::separator();
        let sep3 = PredefinedMenuItem::separator();
        let mut items: Vec<&dyn IsMenuItem> = Vec::new();
        for (i, (item, _)) in actions.iter().enumerate() {
            if i == 5 {
                items.push(&sep1);
            }
            items.push(item);
        }
        items.push(&stop);
        items.push(&sep2);
        items.push(&window);
        items.push(&folder);
        items.push(&sep3);
        items.push(&autostart_item);
        items.push(&quit);
        menu.append_items(&items).map_err(|e| e.to_string())?;

        let builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Snapsmith");
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(icon(false)?);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(icon(false)?);
        let icon = builder.build().map_err(|e| e.to_string())?;
        Ok(Tray { icon, actions, stop, window, folder, autostart: autostart_item, quit, recording_shown: false })
    }

    pub fn action_for(&self, id: &MenuId) -> Option<MenuAction> {
        if let Some((_, a)) = self.actions.iter().find(|(item, _)| item.id() == id) {
            return Some(MenuAction::Run(*a));
        }
        let pairs = [
            (&self.stop, MenuAction::StopRecording),
            (&self.window, MenuAction::OpenWindow),
            (&self.folder, MenuAction::OpenFolder),
            (&self.quit, MenuAction::Quit),
        ];
        if let Some((_, a)) = pairs.iter().find(|(item, _)| item.id() == id) {
            return Some(*a);
        }
        (self.autostart.id() == id).then_some(MenuAction::ToggleAutostart)
    }

    /// Refreshes shortcut labels, the login item tick, and the recording state.
    pub fn sync(&mut self, keys: &Hotkeys, autostart: Option<bool>, recording: bool) {
        for (item, a) in &self.actions {
            item.set_text(label(*a, keys));
        }
        self.autostart.set_checked(autostart.unwrap_or(false));
        if recording != self.recording_shown {
            self.recording_shown = recording;
            self.stop.set_enabled(recording);
            for (item, a) in &self.actions {
                if matches!(a, HotkeyAction::RecordGif | HotkeyAction::RecordVideo) {
                    item.set_enabled(!recording);
                }
            }
            if let Ok(i) = icon(recording) {
                #[cfg(target_os = "macos")]
                let _ = self.icon.set_icon_templated(Some(i));
                #[cfg(not(target_os = "macos"))]
                let _ = self.icon.set_icon(Some(i));
            }
            let _ = self.icon.set_tooltip(Some(if recording { "Snapsmith - recording" } else { "Snapsmith" }));
        }
    }
}

fn icon(recording: bool) -> Result<Icon, String> {
    let size = if cfg!(target_os = "macos") {
        36
    } else if cfg!(target_os = "linux") {
        48
    } else {
        32
    };
    Icon::from_rgba(art::tray_icon(size, recording), size as u32, size as u32).map_err(|e| e.to_string())
}
