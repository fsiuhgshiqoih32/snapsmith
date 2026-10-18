//! Settings, stored as TOML in the user's config directory. The background process reloads the
//! file when it changes, so edits from the main window (or a text editor) apply right away.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "Snapsmith";

/// `SNAPSMITH_HOME` puts settings, history and default saves in one folder (for portable use
/// and for tests).
fn home_override() -> Option<PathBuf> {
    std::env::var_os("SNAPSMITH_HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn config_dir() -> PathBuf {
    home_override().unwrap_or_else(|| dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("snapsmith"))
}

/// History and downloaded OCR models live here.
pub fn data_dir() -> PathBuf {
    home_override().unwrap_or_else(|| dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("snapsmith"))
}

pub fn default_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Imported `.sxcu` custom uploaders.
pub fn uploaders_dir() -> PathBuf {
    config_dir().join("uploaders")
}

pub fn default_save_dir() -> PathBuf {
    if let Some(home) = home_override() {
        return home.join("Pictures");
    }
    dirs::picture_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join("Snapsmith")
}

/// Something to do with a finished capture. Runs in the order listed in the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    /// Copy the image to the clipboard.
    Copy,
    /// Save the image to the screenshots folder.
    Save,
    /// Upload to the default destination and copy the link.
    Upload,
    /// Recognise text and copy it.
    Ocr,
    /// Keep the image on screen in a floating window.
    Pin,
    /// Show a small notification with a thumbnail.
    Toast,
}

impl Task {
    pub const ALL: [Task; 6] = [Task::Copy, Task::Save, Task::Upload, Task::Ocr, Task::Pin, Task::Toast];

    pub fn label(self) -> &'static str {
        match self {
            Task::Copy => "Copy image to clipboard",
            Task::Save => "Save to folder",
            Task::Upload => "Upload and copy link",
            Task::Ocr => "Recognise text and copy it",
            Task::Pin => "Pin to screen",
            Task::Toast => "Show notification",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Jpg,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpg => "jpg",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    /// Empty means the default, `Pictures/Snapsmith`.
    pub save_dir: String,
    /// See `naming::expand`. `/` starts a subfolder.
    pub file_pattern: String,
    pub format: ImageFormat,
    pub jpeg_quality: u8,
    pub after_capture: Vec<Task>,
    pub magnifier: bool,
    /// Seconds to wait before a screen or window capture.
    pub delay: f32,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        CaptureSettings {
            save_dir: String::new(),
            file_pattern: "%y-%mo/Snapsmith_%y-%mo-%d_%h-%mi-%s".into(),
            format: ImageFormat::Png,
            jpeg_quality: 90,
            after_capture: vec![Task::Copy, Task::Save, Task::Toast],
            magnifier: true,
            delay: 0.0,
        }
    }
}

impl CaptureSettings {
    pub fn save_dir(&self) -> PathBuf {
        if self.save_dir.trim().is_empty() { default_save_dir() } else { PathBuf::from(self.save_dir.trim()) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadSettings {
    /// A built-in destination id (`catbox`, `litterbox`, `uguu`) or the name of an imported uploader.
    pub destination: String,
    /// After an upload, put the link on the clipboard instead of the image.
    pub copy_url: bool,
    /// How long Litterbox keeps files: 1h, 12h, 24h or 72h.
    pub litterbox_time: String,
    /// Optional Catbox account hash, so uploads show up in your Catbox account.
    pub catbox_userhash: String,
}

impl Default for UploadSettings {
    fn default() -> Self {
        UploadSettings {
            destination: "catbox".into(),
            copy_url: true,
            litterbox_time: "24h".into(),
            catbox_userhash: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub region: String,
    pub screen: String,
    pub window: String,
    pub monitor: String,
    pub record_gif: String,
    pub record_video: String,
    pub ocr: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        // macOS has no Print Screen key, and Cmd+Shift+3/4/5 belong to the system.
        if cfg!(target_os = "macos") {
            Hotkeys {
                region: "Ctrl+Shift+4".into(),
                screen: "Ctrl+Shift+3".into(),
                window: "Ctrl+Shift+2".into(),
                monitor: String::new(),
                record_gif: "Ctrl+Shift+5".into(),
                record_video: "Ctrl+Shift+6".into(),
                ocr: "Ctrl+Shift+7".into(),
            }
        } else {
            // The same keys ShareX uses.
            Hotkeys {
                region: "Ctrl+PrintScreen".into(),
                screen: "PrintScreen".into(),
                window: "Alt+PrintScreen".into(),
                monitor: String::new(),
                record_gif: "Ctrl+Shift+PrintScreen".into(),
                record_video: "Shift+PrintScreen".into(),
                ocr: String::new(),
            }
        }
    }
}

impl Hotkeys {
    /// Every binding with the action it triggers.
    pub fn bindings(&self) -> [(HotkeyAction, &str); 7] {
        [
            (HotkeyAction::Region, self.region.as_str()),
            (HotkeyAction::Screen, self.screen.as_str()),
            (HotkeyAction::Window, self.window.as_str()),
            (HotkeyAction::Monitor, self.monitor.as_str()),
            (HotkeyAction::RecordGif, self.record_gif.as_str()),
            (HotkeyAction::RecordVideo, self.record_video.as_str()),
            (HotkeyAction::Ocr, self.ocr.as_str()),
        ]
    }

    pub fn get_mut(&mut self, action: HotkeyAction) -> &mut String {
        match action {
            HotkeyAction::Region => &mut self.region,
            HotkeyAction::Screen => &mut self.screen,
            HotkeyAction::Window => &mut self.window,
            HotkeyAction::Monitor => &mut self.monitor,
            HotkeyAction::RecordGif => &mut self.record_gif,
            HotkeyAction::RecordVideo => &mut self.record_video,
            HotkeyAction::Ocr => &mut self.ocr,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    Region,
    Screen,
    Window,
    Monitor,
    RecordGif,
    RecordVideo,
    Ocr,
}

impl HotkeyAction {
    pub fn label(self) -> &'static str {
        match self {
            HotkeyAction::Region => "Capture region",
            HotkeyAction::Screen => "Capture screen",
            HotkeyAction::Window => "Capture window",
            HotkeyAction::Monitor => "Capture monitor",
            HotkeyAction::RecordGif => "Record GIF",
            HotkeyAction::RecordVideo => "Record video",
            HotkeyAction::Ocr => "Extract text",
        }
    }

    /// Command-line arguments for the child process that does the work.
    pub fn args(self) -> &'static [&'static str] {
        match self {
            HotkeyAction::Region => &["capture", "region"],
            HotkeyAction::Screen => &["capture", "screen"],
            HotkeyAction::Window => &["capture", "window"],
            HotkeyAction::Monitor => &["capture", "monitor"],
            HotkeyAction::RecordGif => &["record", "--gif"],
            HotkeyAction::RecordVideo => &["record", "--mp4"],
            HotkeyAction::Ocr => &["ocr"],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordSettings {
    pub gif_fps: u32,
    pub video_fps: u32,
    pub max_seconds: u32,
    /// Path to ffmpeg for MP4 recording. Empty means "find it on PATH".
    pub ffmpeg: String,
    pub show_cursor: bool,
}

impl Default for RecordSettings {
    fn default() -> Self {
        RecordSettings { gif_fps: 15, video_fps: 30, max_seconds: 300, ffmpeg: String::new(), show_cursor: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    /// How long a notification stays up, in seconds.
    pub toast_seconds: f32,
    pub check_updates: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        GeneralSettings { toast_seconds: 4.0, check_updates: true }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub capture: CaptureSettings,
    pub upload: UploadSettings,
    pub hotkeys: Hotkeys,
    pub record: RecordSettings,
    pub general: GeneralSettings,
}

impl Settings {
    /// Clamps values a hand-edited file could get wrong.
    pub fn sanitized(mut self) -> Self {
        self.capture.jpeg_quality = self.capture.jpeg_quality.clamp(1, 100);
        self.capture.delay = self.capture.delay.clamp(0.0, 60.0);
        if self.capture.file_pattern.trim().is_empty() {
            self.capture.file_pattern = CaptureSettings::default().file_pattern;
        }
        let mut seen = Vec::new();
        self.capture.after_capture.retain(|t| {
            let new = !seen.contains(t);
            seen.push(*t);
            new
        });
        self.record.gif_fps = self.record.gif_fps.clamp(1, 50);
        self.record.video_fps = self.record.video_fps.clamp(1, 60);
        self.record.max_seconds = self.record.max_seconds.clamp(1, 3600);
        self.general.toast_seconds = self.general.toast_seconds.clamp(1.0, 60.0);
        if self.upload.destination.trim().is_empty() {
            self.upload.destination = UploadSettings::default().destination;
        }
        self
    }
}

pub fn load(path: &Path) -> Result<Settings, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let settings: Settings = toml::from_str(&text).map_err(|e| format!("{} is not valid: {e}", path.display()))?;
    Ok(settings.sanitized())
}

/// Writes through a temporary file so a reader never sees half a file.
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    let text = toml::to_string_pretty(settings).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Loads the config, writing the defaults first if there isn't one yet. Never fails: a broken
/// file falls back to the defaults (and is left alone so the user can fix it).
pub fn load_or_create(path: &Path) -> (Settings, Option<String>) {
    if path.exists() {
        match load(path) {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(e)),
        }
    } else {
        let s = Settings::default();
        let warning = save(path, &s).err().map(|e| format!("could not write {}: {e}", path.display()));
        (s, warning)
    }
}

pub fn mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let s = Settings::default();
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = toml::from_str("[capture]\nformat = \"jpg\"\n").unwrap();
        assert_eq!(s.capture.format, ImageFormat::Jpg);
        assert_eq!(s.capture.after_capture, CaptureSettings::default().after_capture);
        assert_eq!(s.upload.destination, "catbox");
    }

    #[test]
    fn sanitizing_clamps_and_dedups() {
        let mut s = Settings::default();
        s.capture.jpeg_quality = 0;
        s.capture.after_capture = vec![Task::Copy, Task::Copy, Task::Save];
        s.record.gif_fps = 500;
        let s = s.sanitized();
        assert_eq!(s.capture.jpeg_quality, 1);
        assert_eq!(s.capture.after_capture, vec![Task::Copy, Task::Save]);
        assert_eq!(s.record.gif_fps, 50);
    }

    #[test]
    fn save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let mut s = Settings::default();
        s.upload.destination = "my host".into();
        save(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap(), s);
    }
}
