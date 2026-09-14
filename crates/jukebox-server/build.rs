//! Points the embedded web UI at the built React app.
//!
//! The UI is built with Node (`npm run build`, into out/renderer) before the Rust build.
//! Without a build there's nothing to embed, and the server says so at `/` — as Electron's
//! did in development — rather than failing to compile. `KYYLAN_UI_DIR` overrides the folder.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=KYYLAN_UI_DIR");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let built = std::env::var_os("KYYLAN_UI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../out/renderer"));
    let dir = if built.join("web/index.html").is_file() {
        println!(
            "cargo:rerun-if-changed={}",
            built.join("web/index.html").display()
        );
        built
    } else {
        let empty = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("no-ui");
        std::fs::create_dir_all(&empty).unwrap();
        println!("cargo:rerun-if-changed={}", built.display());
        empty
    };
    println!(
        "cargo:rustc-env=KYYLAN_UI_DIR={}",
        dir.canonicalize().unwrap().display()
    );
}
