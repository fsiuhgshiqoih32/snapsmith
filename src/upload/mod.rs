//! Uploading files: built-in hosts plus imported ShareX custom uploaders (`.sxcu`).
//!
//! The built-in hosts are written as custom uploaders too, so every upload goes through the same
//! request builder and response parser.

pub mod multipart;
pub mod sxcu;
pub mod template;

use std::path::{Path, PathBuf};
use std::time::Duration;

use ureq::ResponseExt;

use crate::config::{self, UploadSettings};
pub use sxcu::{CustomUploader, Payload, Request, Response, UploadResult};

/// Uploads bigger than this are refused before sending (Catbox's own limit is 200 MB).
pub const MAX_UPLOAD: usize = 512 * 1024 * 1024;
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

/// A place uploads can go, as listed in the settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Destination {
    /// What `upload.destination` holds: a built-in id or the uploader's file stem.
    pub id: String,
    pub name: String,
    pub builtin: bool,
}

pub const BUILTINS: [(&str, &str); 2] = [("catbox", "Catbox (permanent)"), ("litterbox", "Litterbox (temporary)")];

fn builtin(id: &str, settings: &UploadSettings) -> Option<CustomUploader> {
    let json = match id {
        "catbox" => {
            let hash = settings.catbox_userhash.trim();
            let userhash = if hash.is_empty() { String::new() } else { format!(r#","userhash":{}"#, json_str(hash)) };
            format!(
                r#"{{"Name":"Catbox","DestinationType":"ImageUploader, FileUploader","RequestMethod":"POST",
                    "RequestURL":"https://catbox.moe/user/api.php","Body":"MultipartFormData",
                    "Arguments":{{"reqtype":"fileupload"{userhash}}},"FileFormName":"fileToUpload"}}"#
            )
        }
        "litterbox" => {
            let time = match settings.litterbox_time.trim() {
                t @ ("1h" | "12h" | "24h" | "72h") => t,
                _ => "24h",
            };
            format!(
                r#"{{"Name":"Litterbox","DestinationType":"ImageUploader, FileUploader","RequestMethod":"POST",
                    "RequestURL":"https://litterbox.catbox.moe/resources/internals/api.php","Body":"MultipartFormData",
                    "Arguments":{{"reqtype":"fileupload","time":"{time}"}},"FileFormName":"fileToUpload"}}"#
            )
        }
        _ => return None,
    };
    CustomUploader::parse(&json).ok()
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Built-ins first, then imported uploaders sorted by name.
pub fn destinations(dir: &Path) -> Vec<Destination> {
    let mut out: Vec<Destination> = BUILTINS
        .iter()
        .map(|(id, name)| Destination { id: id.to_string(), name: name.to_string(), builtin: true })
        .collect();
    let mut custom = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sxcu"))
                && let Some(stem) = path.file_stem()
            {
                let id = stem.to_string_lossy().into_owned();
                let name = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| CustomUploader::parse(&t).ok())
                    .map(|u| u.name)
                    .unwrap_or_else(|| format!("{id} (invalid)"));
                custom.push(Destination { id, name, builtin: false });
            }
        }
    }
    custom.sort_by_key(|d| d.name.to_lowercase());
    out.extend(custom);
    out
}

/// The uploader for a destination id.
pub fn resolve(id: &str, settings: &UploadSettings, dir: &Path) -> Result<CustomUploader, String> {
    if let Some(u) = builtin(id, settings) {
        return Ok(u);
    }
    let path = dir.join(format!("{id}.sxcu"));
    let text = std::fs::read_to_string(&path).map_err(|_| format!("no uploader called {id:?}"))?;
    CustomUploader::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Validates an `.sxcu` file and copies it into the uploaders folder. Returns the destination id.
pub fn import(file: &Path, dir: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("could not read {}: {e}", file.display()))?;
    let uploader = CustomUploader::parse(&text)?;
    if !uploader.accepts_files() {
        return Err(format!(
            "{} is a {} uploader; Snapsmith uploads images and files",
            uploader.name, uploader.destination_type
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let base = crate::naming::sanitize_component(&uploader.name);
    let base = if base.is_empty() { "uploader".to_string() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    // Re-importing the same uploader replaces it; a different one with the same name gets a suffix.
    while let Ok(existing) = std::fs::read_to_string(dir.join(format!("{id}.sxcu"))) {
        if CustomUploader::parse(&existing).is_ok_and(|e| e == uploader) {
            break;
        }
        id = format!("{base} {n}");
        n += 1;
    }
    std::fs::write(dir.join(format!("{id}.sxcu")), text).map_err(|e| e.to_string())?;
    Ok(id)
}

pub fn remove(id: &str, dir: &Path) -> Result<(), String> {
    std::fs::remove_file(dir.join(format!("{id}.sxcu"))).map_err(|e| e.to_string())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .http_status_as_error(false)
        .user_agent(concat!("Snapsmith/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Sends a prepared request and collects the response, whatever its status.
pub fn send(req: &Request) -> Result<Response, String> {
    let mut builder = ureq::http::Request::builder().method(req.method.as_str()).uri(req.url.as_str());
    for (k, v) in &req.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let agent = agent();
    let result = if req.body.is_empty() && matches!(req.method.as_str(), "GET" | "DELETE" | "HEAD") {
        agent.run(builder.body(()).map_err(|e| format!("bad request: {e}"))?)
    } else {
        agent.run(builder.body(req.body.clone()).map_err(|e| format!("bad request: {e}"))?)
    };
    let mut resp = result.map_err(|e| format!("upload failed: {e}"))?;
    let status = resp.status().as_u16();
    let url = resp.get_uri().to_string();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE)
        .read_to_string()
        .map_err(|e| format!("could not read the response: {e}"))?;
    Ok(Response { status, url, headers, body })
}

pub fn upload(uploader: &CustomUploader, payload: &Payload<'_>) -> Result<UploadResult, String> {
    if payload.data.len() > MAX_UPLOAD {
        return Err("the file is too large to upload".into());
    }
    let req = uploader.build_request(payload)?;
    let resp = send(&req)?;
    uploader.parse_response(payload, &resp)
}

/// Uploads to the destination named in the settings.
pub fn upload_with_settings(
    settings: &UploadSettings,
    filename: &str,
    data: &[u8],
) -> Result<(String, UploadResult), String> {
    let dir = config::uploaders_dir();
    let uploader = resolve(&settings.destination, settings, &dir)?;
    let result = upload(&uploader, &Payload { filename, data })?;
    Ok((uploader.name, result))
}

pub fn upload_file(settings: &UploadSettings, path: &Path) -> Result<(String, UploadResult), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if meta.len() as usize > MAX_UPLOAD {
        return Err("the file is too large to upload".into());
    }
    let data = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    upload_with_settings(settings, &name, &data)
}

pub fn uploaders_dir() -> PathBuf {
    config::uploaders_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_resolve() {
        let s = UploadSettings { catbox_userhash: "abc\"".into(), litterbox_time: "99h".into(), ..Default::default() };
        let dir = tempfile::tempdir().unwrap();
        let catbox = resolve("catbox", &s, dir.path()).unwrap();
        assert_eq!(catbox.arguments.get("userhash").map(String::as_str), Some("abc\""));
        let litter = resolve("litterbox", &s, dir.path()).unwrap();
        assert_eq!(litter.arguments.get("time").map(String::as_str), Some("24h"));
        assert!(resolve("nope", &s, dir.path()).is_err());
    }

    #[test]
    fn import_lists_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.sxcu");
        std::fs::write(&src, r#"{"Name":"My: Host","RequestURL":"https://x.example/u","FileFormName":"f"}"#).unwrap();
        let updir = dir.path().join("uploaders");
        let id = import(&src, &updir).unwrap();
        assert_eq!(id, "My_ Host");
        // Same uploader again: replaced, not duplicated.
        assert_eq!(import(&src, &updir).unwrap(), id);
        let list = destinations(&updir);
        assert_eq!(list.len(), BUILTINS.len() + 1);
        assert_eq!(list.last().unwrap().name, "My: Host");
        let s = UploadSettings::default();
        assert_eq!(resolve(&id, &s, &updir).unwrap().name, "My: Host");
        remove(&id, &updir).unwrap();
        assert_eq!(destinations(&updir).len(), BUILTINS.len());

        std::fs::write(&src, r#"{"Name":"Short","DestinationType":"URLShortener","RequestURL":"https://s.example"}"#)
            .unwrap();
        assert!(import(&src, &updir).is_err());
    }
}
