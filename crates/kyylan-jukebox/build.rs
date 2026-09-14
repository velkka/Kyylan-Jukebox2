//! The tray icon is generated from the logo, as the Electron build's is, rather than
//! committed: `node scripts/gen-icons.cjs` writes build/tray.png and build/tray@2x.png.

use std::path::Path;

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os != "windows" && os != "macos" {
        return;
    }
    let build = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build");
    for icon in ["tray.png", "tray@2x.png"] {
        let path = build.join(icon);
        println!("cargo:rerun-if-changed={}", path.display());
        if !path.is_file() {
            panic!(
                "build/{icon} is missing: generate the icons first with `node scripts/gen-icons.cjs`"
            );
        }
    }
}
