//! Embedded dashboard (spec: single-binary distribution). `forgeman
//! dashboard` serves the compiled web dashboard plus a small JSON API that
//! reads live run records from `<repo>/.forgeman/runs/`.

use std::sync::Arc;

use rust_embed::RustEmbed;
use serde_json::json;

use crate::core::store::RunStore;

#[derive(RustEmbed)]
#[folder = "target/dashboard"]
struct Assets;

const INDEX_HTML: &str = "index.html";

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Handle one HTTP GET request. Returns (status, content-type, body).
/// Kept pure-ish (no socket) so it is directly unit-testable.
pub fn handle(store: &RunStore, path: &str) -> (u16, &'static str, Vec<u8>) {
    let path = path.split('?').next().unwrap_or(path);

    if let Some(api_path) = path.strip_prefix("/api/") {
        return handle_api(store, api_path);
    }

    // Routes without a file extension may be SPA routes (`/run`); remember
    // that before mapping them onto `.html` asset names.
    let looks_like_route = !path.contains('.');
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    let mut asset_path = if trimmed.is_empty() {
        INDEX_HTML.to_string()
    } else {
        trimmed.to_string()
    };

    // Static export (trailingSlash=false) maps `/run` and `/run/` to run.html.
    if !asset_path.contains('.') {
        asset_path.push_str(".html");
    }

    if let Some(file) = Assets::get(&asset_path) {
        let mut data = file.data.to_vec();
        if asset_path == INDEX_HTML {
            data = inject_api_banner(data);
        }
        return (200, content_type(&asset_path), data);
    }

    // SPA fallback: unknown extension-less routes go to the app shell.
    if looks_like_route && let Some(file) = Assets::get(INDEX_HTML) {
        return (200, content_type(INDEX_HTML), file.data.to_vec());
    }

    (404, "text/plain; charset=utf-8", b"not found".to_vec())
}

/// Remind the user when they opened the standalone HTML instead of
/// `forgeman dashboard` (the UI needs the API to load data).
fn inject_api_banner(html: Vec<u8>) -> Vec<u8> {
    if let Ok(text) = std::str::from_utf8(&html) {
        let banner = "<body><div style=\"display:none\" id=\"api-note\"></div>";
        text.replacen("<body>", banner, 1).into_bytes()
    } else {
        html
    }
}

fn handle_api(store: &RunStore, api_path: &str) -> (u16, &'static str, Vec<u8>) {
    let content = "application/json";
    if api_path.contains("..") {
        return (400, content, error_body("invalid path"));
    }
    let segments: Vec<&str> = api_path.trim_end_matches('/').split('/').collect();

    match segments.as_slice() {
        ["runs"] => {
            let mut runs: Vec<serde_json::Value> = Vec::new();
            if let Ok(ids) = store.list_run_ids() {
                for id in ids {
                    if let Ok(run) = store.load_run(&id)
                        && let Ok(value) = serde_json::to_value(&run)
                    {
                        runs.push(value);
                    }
                }
            }
            // In-progress runs have no run.json yet — synthesize them from
            // the streamed event log so the dashboard can watch live.
            runs.extend(in_progress_runs(store));
            runs.sort_by(|a, b| {
                let id_of = |r: &serde_json::Value| {
                    r.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };
                id_of(b).cmp(&id_of(a))
            });
            let body = serde_json::to_vec(&runs).unwrap_or_else(|_| b"[]".to_vec());
            (200, content, body)
        }
        ["runs", id] => {
            if !valid_run_id(id) {
                return (400, content, error_body("invalid run id"));
            }
            match store.load_run(id) {
                Ok(run) => (
                    200,
                    content,
                    serde_json::to_vec(&run).unwrap_or_else(|_| error_body("serialize failed")),
                ),
                Err(_) => (404, content, error_body("run not found")),
            }
        }
        ["runs", id, "events"] => {
            if !valid_run_id(id) {
                return (400, content, error_body("invalid run id"));
            }
            let mut events = Vec::new();
            if let Ok(raw) = std::fs::read_to_string(store.events_path(id)) {
                for line in raw.lines().filter(|line| !line.trim().is_empty()) {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                        events.push(value);
                    }
                }
            }
            let body = serde_json::to_vec(&events).unwrap_or_else(|_| b"[]".to_vec());
            (200, content, body)
        }
        _ => (404, content, error_body("unknown api route")),
    }
}

fn valid_run_id(id: &str) -> bool {
    id.starts_with("run_")
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && id.len() <= 64
}

/// Build partial run records for runs that are still executing (events.jsonl
/// exists, run.json not yet written) so the dashboard can watch live.
fn in_progress_runs(store: &RunStore) -> Vec<serde_json::Value> {
    let mut runs = Vec::new();
    let Ok(entries) = std::fs::read_dir(&store.root) else {
        return runs;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let id = entry.file_name().to_string_lossy().to_string();
        if !valid_run_id(&id) || dir.join("run.json").exists() {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(dir.join("events.jsonl")) else {
            continue;
        };

        let mut description = String::new();
        let mut started_at = String::new();
        let mut iterations_count = 0usize;
        let mut last_stage = String::new();
        for line in raw.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            match value.get("event").and_then(|event| event.as_str()) {
                Some("task.created") => {
                    description = value
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or_default()
                        .to_string();
                }
                Some("iteration.started") => {
                    iterations_count = value
                        .get("index")
                        .and_then(|index| index.as_u64())
                        .unwrap_or(0) as usize
                        + 1;
                }
                Some("stage.started") => {
                    last_stage = value
                        .get("stage")
                        .and_then(|stage| stage.as_str())
                        .unwrap_or_default()
                        .to_string();
                }
                _ => {}
            }
            if started_at.is_empty() {
                started_at = value
                    .get("timestamp")
                    .and_then(|timestamp| timestamp.as_str())
                    .unwrap_or_default()
                    .to_string();
            }
        }

        runs.push(json!({
            "id": id,
            "task": { "description": description },
            "started_at": started_at,
            "status": { "status": "running" },
            "iterations": [],
            "iterations_count": iterations_count,
            "current_stage": last_stage,
            "total_cost_usd": 0.0,
        }));
    }
    runs
}

fn error_body(message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "error": message })).unwrap_or_else(|_| b"{}".to_vec())
}

/// Tiny HTTP server: GET-only, one request per connection. Runs until the
/// process is terminated (Ctrl+C takes the default console path).
pub async fn serve(store: RunStore, port: u16) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let store = Arc::new(store);

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(_) => continue,
        };
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            let read = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                .await
                .unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();

            let (status, content_type, body) = handle(&store, &path);
            let reason = if status == 200 {
                "OK"
            } else if status == 404 {
                "Not Found"
            } else {
                "Error"
            };
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n",
                body.len()
            );
            use tokio::io::AsyncWriteExt;
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&body).await;
            let _ = stream.flush().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::core::model::{Run, Task, new_task_id};
    use chrono::Utc;
    use std::path::PathBuf;

    fn store_with_run() -> (tempfile::TempDir, RunStore, String) {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path());
        let task = Task {
            id: new_task_id(),
            description: "Fix the bug".into(),
            repo_root: PathBuf::from("."),
            created_at: Utc::now(),
        };
        let run = Run::starting(task, Config::default());
        let id = run.id.clone();
        store.save_run(&run).unwrap();
        (tmp, store, id)
    }

    #[test]
    fn api_runs_lists_saved_runs() {
        let (_tmp, store, id) = store_with_run();
        let (status, content_type, body) = handle(&store, "/api/runs");
        assert_eq!(status, 200);
        assert_eq!(content_type, "application/json");
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains(&id));
        assert!(text.contains("Fix the bug"));
    }

    #[test]
    fn api_run_detail_and_events() {
        let (_tmp, store, id) = store_with_run();
        let (status, _, body) = handle(&store, &format!("/api/runs/{id}"));
        assert_eq!(status, 200);
        assert!(String::from_utf8(body).unwrap().contains(&id));

        let (status, _, body) = handle(&store, &format!("/api/runs/{id}/events"));
        assert_eq!(status, 200);
        assert_eq!(String::from_utf8(body).unwrap(), "[]");
    }

    #[test]
    fn api_rejects_invalid_ids_and_unknown_routes() {
        let (_tmp, store, _id) = store_with_run();
        assert_eq!(handle(&store, "/api/runs/../../etc").0, 400);
        assert_eq!(handle(&store, "/api/runs/run_missing").0, 404);
        assert_eq!(handle(&store, "/api/unknown").0, 404);
    }

    #[test]
    fn static_fallback_serves_index() {
        let (_tmp, store, _id) = store_with_run();
        let (status, content_type, _) = handle(&store, "/");
        assert_eq!(status, 200);
        assert!(content_type.starts_with("text/html"));
        // Extension-less SPA route falls back to the app shell too.
        let (status, _, _) = handle(&store, "/some/route");
        assert_eq!(status, 200);
        // Missing assets with extensions 404.
        let (status, _, _) = handle(&store, "/missing.js");
        assert_eq!(status, 404);
    }
}
