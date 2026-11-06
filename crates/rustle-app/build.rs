use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../../assets/webview2/x64/WebView2Loader.dll");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source = manifest.join("../../assets/webview2/x64/WebView2Loader.dll");
    if !source.exists() {
        println!(
            "cargo:warning=WebView2Loader.dll not found at {}",
            source.display()
        );
        return;
    }

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let Some(profile_dir) = out.ancestors().nth(3) else {
        return;
    };
    let destination = profile_dir.join("WebView2Loader.dll");
    if let Err(error) = fs::copy(&source, &destination) {
        println!("cargo:warning=failed to copy WebView2Loader.dll: {error}");
    }
}
