use std::time::Duration;

use rustle_obscura::ObscuraRenderer;
use rustle_obscura::cdp::ObscuraServer;

fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_owned());
    let out = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "obscura-cdp.png".to_owned());
    let renderer = match ObscuraRenderer::locate() {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let mut server = match ObscuraServer::start(renderer.binary()) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let target = match server.client.create_target("about:blank", None) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let session = match server.client.attach(&target) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let _ = server
        .client
        .command("Page.enable", serde_json::json!({}), Some(&session));
    if let Err(error) = server.client.command(
        "Page.navigate",
        serde_json::json!({"url": url}),
        Some(&session),
    ) {
        eprintln!("{error}");
        std::process::exit(1);
    }
    std::thread::sleep(Duration::from_millis(1500));
    match server.client.screenshot(&session) {
        Ok(png) => {
            if let Err(error) = std::fs::write(&out, &png) {
                eprintln!("write: {error}");
                std::process::exit(1);
            }
            println!("ok {out} {} bytes", png.len());
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
    server.stop();
}
