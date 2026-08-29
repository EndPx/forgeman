//! Minimal localhost HTTP server used to test providers against real HTTP
//! round-trips without external services. Test-only.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, channel};

pub struct CapturedRequest {
    pub headers: String,
    pub body: String,
}

/// Serve exactly one HTTP request on a random port, capture it, and respond
/// with `status` + `resp_body`. Returns the base URL and a receiver for the
/// captured request.
pub fn spawn_one_shot(status: u16, resp_body: &'static str) -> (String, Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("port binds");
    let addr = listener.local_addr().expect("addr known");
    let (tx, rx) = channel();

    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = stream.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                let content_length = head
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if buf.len() >= pos + 4 + content_length {
                    break;
                }
            }
        }
        let raw_request = String::from_utf8_lossy(&buf).to_string();
        let (headers, req_body) = raw_request
            .split_once("\r\n\r\n")
            .unwrap_or((&raw_request, ""));
        let _ = tx.send(CapturedRequest {
            headers: headers.to_string(),
            body: req_body.to_string(),
        });

        let reason = if status == 200 { "OK" } else { "Error" };
        let http = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{resp_body}",
            resp_body.len()
        );
        let _ = stream.write_all(http.as_bytes());
        let _ = stream.flush();
    });

    (format!("http://{addr}"), rx)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
