//! `multipart/form-data` bodies, built in memory.

pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Default for Multipart {
    fn default() -> Self {
        Self::new()
    }
}

impl Multipart {
    pub fn new() -> Self {
        Multipart {
            boundary: format!("----Snapsmith{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..)),
            body: Vec::new(),
        }
    }

    pub fn text(&mut self, name: &str, value: &str) {
        self.start(&format!("Content-Disposition: form-data; name=\"{}\"", quote(name)));
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
    }

    pub fn file(&mut self, name: &str, filename: &str, content_type: &str, data: &[u8]) {
        self.start(&format!(
            "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {content_type}",
            quote(name),
            quote(filename)
        ));
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
    }

    fn start(&mut self, headers: &str) {
        self.body.extend_from_slice(format!("--{}\r\n{headers}\r\n\r\n", self.boundary).as_bytes());
    }

    /// The Content-Type header value and the finished body.
    pub fn finish(mut self) -> (String, Vec<u8>) {
        self.body.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (format!("multipart/form-data; boundary={}", self.boundary), self.body)
    }
}

/// Field and file names go inside double quotes; keep them on one line and unambiguous.
fn quote(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '\r' && *c != '\n')
        .map(|c| if c == '"' { "%22".to_string() } else { c.to_string() })
        .collect()
}

/// A MIME type from a file name, for the file part and binary bodies.
pub fn mime_for(filename: &str) -> &'static str {
    let ext = filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "txt" | "log" | "md" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_layout() {
        let mut m = Multipart::new();
        m.text("reqtype", "fileupload");
        m.file("file\"x", "a.png", "image/png", b"\x89PNG");
        let boundary = m.boundary.clone();
        let (ct, body) = m.finish();
        assert_eq!(ct, format!("multipart/form-data; boundary={boundary}"));
        let expected = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"reqtype\"\r\n\r\nfileupload\r\n\
             --{b}\r\nContent-Disposition: form-data; name=\"file%22x\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n\x7fPNG\r\n\
             --{b}--\r\n",
            b = boundary
        );
        let mut expected = expected.into_bytes();
        let pos = expected.iter().position(|&b| b == 0x7f).unwrap();
        expected[pos] = 0x89;
        assert_eq!(body, expected);
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_for("Shot.PNG"), "image/png");
        assert_eq!(mime_for("noext"), "application/octet-stream");
    }
}
