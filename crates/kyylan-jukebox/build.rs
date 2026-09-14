//! The tray icon is generated from the logo, as the Electron build's is, rather than
//! committed: `node scripts/gen-icons.cjs` writes build/tray.png and build/tray@2x.png, and
//! build/icon.ico, which the Windows program carries as its icon.

use std::path::Path;

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os != "windows" && os != "macos" {
        return;
    }
    let build = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build");
    let mut icons = vec!["tray.png", "tray@2x.png"];
    if os == "windows" {
        icons.push("icon.ico");
    }
    for icon in &icons {
        let path = build.join(icon);
        println!("cargo:rerun-if-changed={}", path.display());
        if !path.is_file() {
            panic!(
                "build/{icon} is missing: generate the icons first with `node scripts/gen-icons.cjs`"
            );
        }
    }
    if os == "windows" {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(build.join("icon.ico").to_str().expect("a UTF-8 path"))
            .set("ProductName", "Kyylan Jukebox")
            .set("FileDescription", "Kyylan Jukebox");
        resource
            .compile()
            .expect("embedding the icon and version details");
    }
}
