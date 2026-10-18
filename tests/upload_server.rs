//! End-to-end uploads against a local HTTP server standing in for an image host.

use std::sync::mpsc;
use std::thread;

use snapsmith::upload::{self, CustomUploader, Payload};

struct Seen {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Serves one request with `status` and `body`, and reports what it received.
fn serve_once(status: u16, body: &'static str) -> (String, mpsc::Receiver<Seen>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", server.server_addr().to_ip().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut req = server.recv().unwrap();
        let mut data = Vec::new();
        req.as_reader().read_to_end(&mut data).unwrap();
        let seen = Seen {
            method: req.method().to_string(),
            url: req.url().to_string(),
            headers: req.headers().iter().map(|h| (h.field.to_string(), h.value.to_string())).collect(),
            body: data,
        };
        let resp = tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header("X-Delete: https://host.example/del/77".parse::<tiny_http::Header>().unwrap());
        req.respond(resp).unwrap();
        tx.send(seen).unwrap();
    });
    (addr, rx)
}

fn header<'a>(seen: &'a Seen, name: &str) -> Option<&'a str> {
    seen.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

#[test]
fn multipart_upload_with_json_response() {
    let (addr, rx) = serve_once(200, r#"{"data":{"link":"https://i.host.example/abc.png","id":"abc"}}"#);
    let sxcu = format!(
        r#"{{"Name":"Local","RequestMethod":"POST","RequestURL":"{addr}/upload","Parameters":{{"v":"2"}},
            "Headers":{{"Authorization":"Client-ID xyz"}},"Body":"MultipartFormData",
            "Arguments":{{"title":"{{filename}}"}},"FileFormName":"image",
            "URL":"{{json:data.link}}","DeletionURL":"{{header:x-delete}}"}}"#
    );
    let uploader = CustomUploader::parse(&sxcu).unwrap();
    let result = upload::upload(&uploader, &Payload { filename: "cap.png", data: b"\x89PNG fake" }).unwrap();
    assert_eq!(result.url, "https://i.host.example/abc.png");
    assert_eq!(result.deletion_url.as_deref(), Some("https://host.example/del/77"));

    let seen = rx.recv().unwrap();
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.url, "/upload?v=2");
    assert_eq!(header(&seen, "Authorization"), Some("Client-ID xyz"));
    assert!(header(&seen, "User-Agent").unwrap().starts_with("Snapsmith/"));
    let ct = header(&seen, "Content-Type").unwrap();
    let boundary = ct.split("boundary=").nth(1).unwrap();
    let body = String::from_utf8_lossy(&seen.body);
    assert!(body.starts_with(&format!("--{boundary}\r\n")));
    assert!(body.contains("name=\"title\"\r\n\r\ncap.png\r\n"));
    assert!(body.contains("name=\"image\"; filename=\"cap.png\"\r\nContent-Type: image/png\r\n\r\n"));
    assert!(body.ends_with(&format!("--{boundary}--\r\n")));
}

#[test]
fn server_errors_are_reported() {
    let (addr, _rx) = serve_once(500, r#"{"error":"disk full"}"#);
    let sxcu = format!(
        r#"{{"Name":"Broken","RequestURL":"{addr}/u","FileFormName":"f","URL":"{{json:url}}","ErrorMessage":"{{json:error}}"}}"#
    );
    let uploader = CustomUploader::parse(&sxcu).unwrap();
    let err = upload::upload(&uploader, &Payload { filename: "a.png", data: b"x" }).unwrap_err();
    assert_eq!(err, "Broken answered HTTP 500: disk full");
}

#[test]
fn binary_put_with_text_response() {
    let (addr, rx) = serve_once(201, "https://files.example/z.gif\n");
    let sxcu =
        format!(r#"{{"Name":"Put","RequestMethod":"PUT","RequestURL":"{addr}/files/{{filename}}","Body":"Binary"}}"#);
    let uploader = CustomUploader::parse(&sxcu).unwrap();
    let result = upload::upload(&uploader, &Payload { filename: "z.gif", data: b"GIF89a" }).unwrap();
    assert_eq!(result.url, "https://files.example/z.gif");
    let seen = rx.recv().unwrap();
    assert_eq!(seen.method, "PUT");
    assert_eq!(seen.url, "/files/z.gif");
    assert_eq!(seen.body, b"GIF89a");
    assert_eq!(header(&seen, "Content-Type"), Some("image/gif"));
}

#[test]
fn unreachable_host_fails_cleanly() {
    let uploader = CustomUploader::parse(r#"{"RequestURL":"http://127.0.0.1:9/u","FileFormName":"f"}"#).unwrap();
    let err = upload::upload(&uploader, &Payload { filename: "a.png", data: b"x" }).unwrap_err();
    assert!(err.starts_with("upload failed"), "{err}");
}
