//! Capture and upload history: one JSON object per line in `history.jsonl` in the data directory.
//! Appending is a single write, so several capture processes can add entries at once.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// Unix time in milliseconds.
    pub time: i64,
    /// `image`, `file` or `text`.
    pub kind: String,
    /// Where the file was saved, if it was.
    pub path: Option<PathBuf>,
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub deletion_url: Option<String>,
    /// Destination name for uploads.
    pub uploader: Option<String>,
    pub width: u32,
    pub height: u32,
}

impl Entry {
    pub fn now(kind: &str) -> Self {
        Entry { time: chrono::Utc::now().timestamp_millis(), kind: kind.into(), ..Default::default() }
    }

    pub fn display_name(&self) -> String {
        if let Some(p) = &self.path
            && let Some(name) = p.file_name()
        {
            return name.to_string_lossy().into_owned();
        }
        self.url.clone().unwrap_or_else(|| "(unsaved)".into())
    }
}

pub fn default_path() -> PathBuf {
    crate::config::data_dir().join("history.jsonl")
}

pub fn append(path: &Path, entry: &Entry) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| e.to_string())?;
    f.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

/// Oldest first. Lines that don't parse (a crash mid-write, a newer format) are skipped.
pub fn load(path: &Path) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Replaces the whole file, for removing entries.
pub fn rewrite(path: &Path, entries: &[Entry]) -> Result<(), String> {
    let mut text = String::new();
    for e in entries {
        text.push_str(&serde_json::to_string(e).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h").join("history.jsonl");
        let mut a = Entry::now("image");
        a.path = Some(PathBuf::from("/tmp/a.png"));
        a.width = 10;
        let mut b = Entry::now("file");
        b.url = Some("https://example.com/x".into());
        append(&path, &a).unwrap();
        append(&path, &b).unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{broken\n").unwrap();
        let all = load(&path);
        assert_eq!(all, vec![a.clone(), b.clone()]);
        assert_eq!(all[0].display_name(), "a.png");
        assert_eq!(all[1].display_name(), "https://example.com/x");
        rewrite(&path, &[b.clone()]).unwrap();
        assert_eq!(load(&path), vec![b]);
    }
}
