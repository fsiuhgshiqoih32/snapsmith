//! ShareX custom uploaders (`.sxcu` files): parsing, building the HTTP request, and reading the
//! links out of the response.
//!
//! Field names are matched case-insensitively, the way ShareX's JSON reader does, and both the
//! current format and the pre-13 one (`RequestType`, `ResponseType`, `RegexList`, `$json:..$`)
//! are accepted.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::multipart::{Multipart, mime_for};
use super::template::{self, Context};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    None,
    Multipart,
    FormUrlEncoded,
    Json,
    Xml,
    Binary,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CustomUploader {
    pub name: String,
    pub destination_type: String,
    pub method: String,
    pub request_url: String,
    pub parameters: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub body: BodyKind,
    pub arguments: BTreeMap<String, String>,
    pub file_form_name: String,
    pub data: String,
    pub url: String,
    pub thumbnail_url: String,
    pub deletion_url: String,
    pub error_message: String,
    /// Pre-13 uploaders: `$...$` templates and a RegexList.
    pub legacy: bool,
    pub regex_list: Vec<String>,
}

/// What gets uploaded.
pub struct Payload<'a> {
    pub filename: &'a str,
    pub data: &'a [u8],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Response {
    pub status: u16,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UploadResult {
    pub url: String,
    pub thumbnail_url: Option<String>,
    pub deletion_url: Option<String>,
}

fn get<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    obj.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
}

fn get_str(obj: &Map<String, Value>, key: &str) -> String {
    match get(obj, key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn get_map(obj: &Map<String, Value>, key: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(Value::Object(m)) = get(obj, key) {
        for (k, v) in m {
            let v = match v {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            out.insert(k.clone(), v);
        }
    }
    out
}

impl CustomUploader {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim_start_matches('\u{feff}');
        let value: Value = serde_json::from_str(text).map_err(|e| format!("not a valid uploader file: {e}"))?;
        let Value::Object(obj) = value else { return Err("not a valid uploader file: expected a JSON object".into()) };

        let request_url = get_str(&obj, "RequestURL");
        if request_url.trim().is_empty() {
            return Err("the uploader has no RequestURL".into());
        }
        let legacy_method = get_str(&obj, "RequestType");
        let legacy =
            !legacy_method.is_empty() || get(&obj, "ResponseType").is_some() || get(&obj, "RegexList").is_some();
        let method = {
            let m = get_str(&obj, "RequestMethod");
            let m = if m.is_empty() { legacy_method } else { m };
            if m.is_empty() { "POST".to_string() } else { m.to_ascii_uppercase() }
        };
        let file_form_name = get_str(&obj, "FileFormName");
        let body_text = get_str(&obj, "Body");
        let body = match body_text.to_ascii_lowercase().as_str() {
            "multipartformdata" => BodyKind::Multipart,
            "formurlencoded" => BodyKind::FormUrlEncoded,
            "json" => BodyKind::Json,
            "xml" => BodyKind::Xml,
            "binary" => BodyKind::Binary,
            "none" => BodyKind::None,
            "" if !file_form_name.is_empty() => BodyKind::Multipart,
            "" if method == "GET" => BodyKind::None,
            "" => BodyKind::FormUrlEncoded,
            other => return Err(format!("unsupported Body type {other:?}")),
        };
        let regex_list = match get(&obj, "RegexList") {
            Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            _ => Vec::new(),
        };
        let name = get_str(&obj, "Name");
        let name = if name.trim().is_empty() { host_of(&request_url) } else { name.trim().to_string() };
        Ok(CustomUploader {
            name,
            destination_type: get_str(&obj, "DestinationType"),
            method,
            request_url,
            parameters: get_map(&obj, "Parameters"),
            headers: get_map(&obj, "Headers"),
            body,
            arguments: get_map(&obj, "Arguments"),
            file_form_name,
            data: get_str(&obj, "Data"),
            url: get_str(&obj, "URL"),
            thumbnail_url: get_str(&obj, "ThumbnailURL"),
            deletion_url: get_str(&obj, "DeletionURL"),
            error_message: get_str(&obj, "ErrorMessage"),
            legacy,
            regex_list,
        })
    }

    /// True if ShareX would offer this uploader for images or files (or doesn't say).
    pub fn accepts_files(&self) -> bool {
        let t = self.destination_type.to_ascii_lowercase();
        t.is_empty() || t.contains("image") || t.contains("file") || t == "none"
    }

    fn expand(&self, template: &str, ctx: &Context) -> String {
        if self.legacy { template::expand_legacy(template, ctx) } else { template::expand(template, ctx) }
    }

    pub fn build_request(&self, payload: &Payload<'_>) -> Result<Request, String> {
        let ctx = Context {
            filename: payload.filename.to_string(),
            input: String::new(),
            regex_list: self.regex_list.clone(),
            ..Default::default()
        };
        let mut url = self.expand(&self.request_url, &ctx).trim().to_string();
        let mut query: Vec<(String, String)> =
            self.parameters.iter().map(|(k, v)| (k.clone(), self.expand(v, &ctx))).collect();
        // Legacy GET uploaders sent their arguments in the query string.
        if self.legacy && self.method == "GET" {
            query.extend(self.arguments.iter().map(|(k, v)| (k.clone(), self.expand(v, &ctx))));
        }
        if !query.is_empty() {
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&urlencode_pairs(&query));
        }

        let mut headers: Vec<(String, String)> =
            self.headers.iter().map(|(k, v)| (k.clone(), self.expand(v, &ctx))).collect();
        let args: Vec<(String, String)> =
            self.arguments.iter().map(|(k, v)| (k.clone(), self.expand(v, &ctx))).collect();

        let (content_type, body) = match self.body {
            BodyKind::None => (None, Vec::new()),
            BodyKind::Multipart => {
                if self.file_form_name.is_empty() {
                    return Err("the uploader has no FileFormName, so it can't upload files".into());
                }
                let mut m = Multipart::new();
                for (k, v) in &args {
                    m.text(k, v);
                }
                m.file(&self.file_form_name, payload.filename, mime_for(payload.filename), payload.data);
                let (ct, body) = m.finish();
                (Some(ct), body)
            }
            BodyKind::FormUrlEncoded => {
                (Some("application/x-www-form-urlencoded".to_string()), urlencode_pairs(&args).into_bytes())
            }
            BodyKind::Json => {
                let json_ctx = Context { json_escape: true, ..ctx.clone() };
                (Some("application/json".to_string()), self.expand(&self.data, &json_ctx).into_bytes())
            }
            BodyKind::Xml => (Some("application/xml".to_string()), self.expand(&self.data, &ctx).into_bytes()),
            BodyKind::Binary => (Some(mime_for(payload.filename).to_string()), payload.data.to_vec()),
        };
        if let Some(ct) = content_type
            && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        {
            headers.push(("Content-Type".into(), ct));
        }
        Ok(Request { method: self.method.clone(), url, headers, body })
    }

    pub fn parse_response(&self, payload: &Payload<'_>, response: &Response) -> Result<UploadResult, String> {
        let ctx = Context {
            filename: payload.filename.to_string(),
            response: response.body.clone(),
            response_url: response.url.clone(),
            headers: response.headers.clone(),
            regex_list: self.regex_list.clone(),
            ..Default::default()
        };
        if !(200..300).contains(&response.status) {
            let detail = if self.error_message.is_empty() {
                String::new()
            } else {
                self.expand(&self.error_message, &ctx).trim().to_string()
            };
            let detail = if detail.is_empty() { snippet(&response.body) } else { detail };
            return Err(format!("{} answered HTTP {}{}", self.name, response.status, prefixed(": ", &detail)));
        }
        let url = if self.url.trim().is_empty() {
            response.body.trim().to_string()
        } else {
            self.expand(&self.url, &ctx).trim().to_string()
        };
        if url.is_empty() {
            let detail = if self.error_message.is_empty() {
                snippet(&response.body)
            } else {
                self.expand(&self.error_message, &ctx).trim().to_string()
            };
            return Err(format!("{} didn't return a link{}", self.name, prefixed(": ", &detail)));
        }
        let optional = |t: &str| {
            let v = self.expand(t, &ctx).trim().to_string();
            (!t.trim().is_empty() && !v.is_empty()).then_some(v)
        };
        Ok(UploadResult {
            url,
            thumbnail_url: optional(&self.thumbnail_url),
            deletion_url: optional(&self.deletion_url),
        })
    }
}

fn prefixed(prefix: &str, s: &str) -> String {
    if s.is_empty() { String::new() } else { format!("{prefix}{s}") }
}

/// The start of a response body, for error messages.
fn snippet(body: &str) -> String {
    let one_line: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s: String = one_line.chars().take(200).collect();
    if one_line.chars().count() > 200 {
        s.push_str("...");
    }
    s
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split(['/', '?', '#']).next().unwrap_or("custom").to_string()
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urlencode_pairs(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v))).collect::<Vec<_>>().join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODERN: &str = r#"{
      "Version": "17.0.0",
      "Name": "Example host",
      "DestinationType": "ImageUploader, FileUploader",
      "RequestMethod": "POST",
      "RequestURL": "https://up.example.com/api/upload",
      "Parameters": { "expires": "7d" },
      "Headers": { "Authorization": "Bearer abc" },
      "Body": "MultipartFormData",
      "Arguments": { "name": "{filename}" },
      "FileFormName": "file",
      "URL": "{json:data.url}",
      "ThumbnailURL": "{json:data.thumb}",
      "DeletionURL": "https://up.example.com/delete/{json:data.id}",
      "ErrorMessage": "{json:error.message}"
    }"#;

    fn payload() -> Payload<'static> {
        Payload { filename: "shot.png", data: b"PNGDATA" }
    }

    #[test]
    fn parses_modern_uploader() {
        let u = CustomUploader::parse(MODERN).unwrap();
        assert_eq!(u.name, "Example host");
        assert_eq!(u.body, BodyKind::Multipart);
        assert!(!u.legacy);
        assert!(u.accepts_files());
    }

    #[test]
    fn builds_multipart_request() {
        let u = CustomUploader::parse(MODERN).unwrap();
        let req = u.build_request(&payload()).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.url, "https://up.example.com/api/upload?expires=7d");
        assert!(req.headers.contains(&("Authorization".into(), "Bearer abc".into())));
        let ct = &req.headers.iter().find(|(k, _)| k == "Content-Type").unwrap().1;
        assert!(ct.starts_with("multipart/form-data; boundary="));
        let body = String::from_utf8_lossy(&req.body);
        assert!(body.contains("name=\"name\"\r\n\r\nshot.png\r\n"));
        assert!(body.contains("name=\"file\"; filename=\"shot.png\"\r\nContent-Type: image/png\r\n\r\nPNGDATA\r\n"));
    }

    #[test]
    fn reads_links_from_response() {
        let u = CustomUploader::parse(MODERN).unwrap();
        let resp = Response {
            status: 200,
            body: r#"{"data":{"url":"https://i.example.com/x.png","thumb":"https://i.example.com/t.png","id":"x9"}}"#
                .into(),
            ..Default::default()
        };
        let r = u.parse_response(&payload(), &resp).unwrap();
        assert_eq!(r.url, "https://i.example.com/x.png");
        assert_eq!(r.thumbnail_url.as_deref(), Some("https://i.example.com/t.png"));
        assert_eq!(r.deletion_url.as_deref(), Some("https://up.example.com/delete/x9"));
    }

    #[test]
    fn reports_errors() {
        let u = CustomUploader::parse(MODERN).unwrap();
        let resp =
            Response { status: 413, body: r#"{"error":{"message":"File too large"}}"#.into(), ..Default::default() };
        assert_eq!(u.parse_response(&payload(), &resp).unwrap_err(), "Example host answered HTTP 413: File too large");
        let resp = Response { status: 200, body: r#"{"data":{}}"#.into(), ..Default::default() };
        assert!(u.parse_response(&payload(), &resp).unwrap_err().contains("didn't return a link"));
    }

    #[test]
    fn plain_text_response_is_the_link() {
        let u = CustomUploader::parse(
            r#"{"RequestURL":"https://catbox.example/api.php","Body":"MultipartFormData","FileFormName":"fileToUpload","Arguments":{"reqtype":"fileupload"}}"#,
        )
        .unwrap();
        assert_eq!(u.name, "catbox.example");
        let resp = Response { status: 200, body: "https://files.example/abc.png\n".into(), ..Default::default() };
        assert_eq!(u.parse_response(&payload(), &resp).unwrap().url, "https://files.example/abc.png");
    }

    #[test]
    fn keys_are_case_insensitive_and_values_stringified() {
        let u = CustomUploader::parse(
            "\u{feff}{\"requesturl\":\"https://x.example/u\",\"fileformname\":\"f\",\"arguments\":{\"n\":5}}",
        )
        .unwrap();
        assert_eq!(u.body, BodyKind::Multipart);
        assert_eq!(u.arguments.get("n").map(String::as_str), Some("5"));
    }

    #[test]
    fn json_and_binary_bodies() {
        let u = CustomUploader::parse(
            r#"{"RequestURL":"https://x.example/u","Body":"JSON","Data":"{\"name\":\"{filename}\"}","URL":"{json:link}"}"#,
        )
        .unwrap();
        let p = Payload { filename: "a \"b\".png", data: b"" };
        let req = u.build_request(&p).unwrap();
        assert_eq!(String::from_utf8(req.body).unwrap(), r#"{"name":"a \"b\".png"}"#);

        let u = CustomUploader::parse(r#"{"RequestURL":"https://x.example/u","RequestMethod":"PUT","Body":"Binary"}"#)
            .unwrap();
        let req = u.build_request(&payload()).unwrap();
        assert_eq!(req.method, "PUT");
        assert_eq!(req.body, b"PNGDATA");
        assert!(req.headers.contains(&("Content-Type".into(), "image/png".into())));
    }

    #[test]
    fn legacy_uploader() {
        let u = CustomUploader::parse(
            r#"{"Name":"Old","RequestType":"POST","RequestURL":"https://old.example/up","FileFormName":"img",
                "ResponseType":"Text","RegexList":["\"id\":\"(\\w+)\""],"URL":"https://old.example/$regex:1,1$"}"#,
        )
        .unwrap();
        assert!(u.legacy);
        assert_eq!(u.body, BodyKind::Multipart);
        let resp = Response { status: 200, body: r#"{"id":"q7"}"#.into(), ..Default::default() };
        assert_eq!(u.parse_response(&payload(), &resp).unwrap().url, "https://old.example/q7");
    }

    #[test]
    fn rejects_bad_files() {
        assert!(CustomUploader::parse("not json").is_err());
        assert!(CustomUploader::parse("[]").is_err());
        assert!(CustomUploader::parse(r#"{"Name":"x"}"#).is_err());
        assert!(CustomUploader::parse(r#"{"RequestURL":"https://x","Body":"Telepathy"}"#).is_err());
        let u = CustomUploader::parse(r#"{"RequestURL":"https://x","Body":"MultipartFormData"}"#).unwrap();
        assert!(u.build_request(&payload()).is_err());
    }

    #[test]
    fn url_encoding() {
        assert_eq!(urlencode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }
}
