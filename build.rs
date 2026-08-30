//! Build script: guarantees the embeddable dashboard directory exists with a
//! fallback page, so `cargo build` always succeeds even without a web build.

use std::path::PathBuf;

const FALLBACK_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>ForgeMan</title>
<style>body{background:#0b0e14;color:#d7dde8;font-family:ui-monospace,Consolas,monospace;
display:grid;place-items:center;height:100vh;margin:0}code{color:#e8b34b}</style></head>
<body><div style="text-align:center">
<h1>FORGE<span style="color:#e8b34b">MAN</span></h1>
<p>Dashboard assets are not built into this binary.</p>
<p>Build them with <code>npm run build-all</code>, then rebuild.</p>
</div></body></html>"#;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir known");
    let dir = PathBuf::from(manifest).join("target").join("dashboard");
    std::fs::create_dir_all(&dir).expect("create embed dir");
    let index = dir.join("index.html");
    if !index.exists() {
        std::fs::write(&index, FALLBACK_HTML).expect("write fallback dashboard");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
