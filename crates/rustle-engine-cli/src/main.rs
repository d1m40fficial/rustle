use std::path::PathBuf;

use rustle_engine::{RenderRequest, RendererKind, StaticRenderer};
use rustle_obscura::ObscuraRenderer;
use rustle_servo::ServoStaticRenderer;

fn main() {
    let mut engine = String::from("servo");
    let mut url = String::new();
    let mut out = PathBuf::from("capture.png");
    let mut width = 1280u32;
    let mut height = 720u32;
    let mut timeout_ms = 30_000u64;
    let mut repeat = 1u32;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--engine" => {
                if let Some(value) = args.get(index + 1) {
                    engine = value.clone();
                }
                index += 1;
            }
            "--url" => {
                if let Some(value) = args.get(index + 1) {
                    url = value.clone();
                }
                index += 1;
            }
            "--out" => {
                if let Some(value) = args.get(index + 1) {
                    out = PathBuf::from(value);
                }
                index += 1;
            }
            "--width" => {
                if let Some(value) = args.get(index + 1).and_then(|value| value.parse().ok()) {
                    width = value;
                }
                index += 1;
            }
            "--height" => {
                if let Some(value) = args.get(index + 1).and_then(|value| value.parse().ok()) {
                    height = value;
                }
                index += 1;
            }
            "--timeout" => {
                if let Some(value) = args.get(index + 1).and_then(|value| value.parse().ok()) {
                    timeout_ms = value;
                }
                index += 1;
            }
            "--repeat" => {
                if let Some(value) = args
                    .get(index + 1)
                    .and_then(|value| value.parse::<u32>().ok())
                {
                    repeat = value.max(1);
                }
                index += 1;
            }
            _ => {}
        }
        index += 1;
    }

    if url.is_empty() {
        eprintln!(
            "usage: rustle-capture --engine servo|obscura --url https://example.com --out capture.png"
        );
        std::process::exit(2);
    }

    let request = RenderRequest::new(url, width, height, timeout_ms);
    let renderer: Box<dyn StaticRenderer> = match engine.as_str() {
        "obscura" => match ObscuraRenderer::locate() {
            Ok(renderer) => Box::new(renderer),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        },
        _ => match ServoStaticRenderer::locate() {
            Ok(renderer) => Box::new(renderer),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        },
    };

    let mut best = None;
    for run in 0..repeat {
        match renderer.render(&request) {
            Ok(page) => {
                let path = if repeat > 1 {
                    let stem = out
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("capture");
                    let ext = out
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("png");
                    out.with_file_name(format!("{stem}-{run}.{ext}"))
                } else {
                    out.clone()
                };
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(error) = std::fs::write(&path, &page.png) {
                    eprintln!("write failed: {error}");
                    std::process::exit(1);
                }
                let result = serde_json::json!({
                    "ok": true,
                    "engine": match renderer.kind() {
                        RendererKind::Servo => "servo",
                        RendererKind::Obscura => "obscura",
                    },
                    "url": page.final_url,
                    "elapsed_ms": page.elapsed_ms,
                    "bytes": page.png.len(),
                    "out": path,
                });
                println!("{result}");
                best = Some(page.elapsed_ms);
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    }
    let _ = best;
}
