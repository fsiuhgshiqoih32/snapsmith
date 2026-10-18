//! Copying images and text.
//!
//! On Windows and macOS the system keeps clipboard contents after we exit. On Linux (X11 and
//! Wayland) the copying program has to stay running to serve them, so there a small detached
//! `snapsmith clipboard` process holds the data until something else is copied.

use image::RgbaImage;

use crate::log::warning;

pub fn copy_image(img: &RgbaImage) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let path = crate::app::temp_file("clip", "png");
        img.save(&path).map_err(|e| e.to_string())?;
        let path_s = path.to_string_lossy().into_owned();
        crate::app::spawn_self(&["clipboard", "--image", &path_s]).map(|_| ())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
        cb.set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: std::borrow::Cow::Borrowed(img.as_raw()),
        })
        .map_err(|e| format!("could not copy the image: {e}"))
    }
}

pub fn copy_text(text: &str) {
    if let Err(e) = try_copy_text(text) {
        warning!("{e}");
    }
}

pub fn try_copy_text(text: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["clipboard", "--text"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::app::detach(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("could not copy: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
        cb.set_text(text.to_string()).map_err(|e| format!("could not copy: {e}"))
    }
}

/// `snapsmith clipboard --image FILE | --text`: owns the clipboard until something else is
/// copied (Linux), then exits. The image file is deleted once loaded.
pub fn hold(image: Option<&std::path::Path>, text: bool) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    let data = if let Some(path) = image {
        let img = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
        let _ = std::fs::remove_file(path);
        Some(img)
    } else {
        None
    };
    let text_value = if text {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
        Some(s)
    } else {
        None
    };
    #[cfg(target_os = "linux")]
    {
        use arboard::SetExtLinux;
        if let Some(img) = &data {
            let d = arboard::ImageData {
                width: img.width() as usize,
                height: img.height() as usize,
                bytes: std::borrow::Cow::Borrowed(img.as_raw()),
            };
            return cb.set().wait().image(d).map_err(|e| e.to_string());
        }
        if let Some(t) = text_value {
            return cb.set().wait().text(t).map_err(|e| e.to_string());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        if let Some(img) = &data {
            let d = arboard::ImageData {
                width: img.width() as usize,
                height: img.height() as usize,
                bytes: std::borrow::Cow::Borrowed(img.as_raw()),
            };
            cb.set_image(d).map_err(|e| e.to_string())?;
        }
        if let Some(t) = text_value {
            cb.set_text(t).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
