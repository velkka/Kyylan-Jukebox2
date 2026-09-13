//! The data directory must be the one Electron uses, on every platform.

use jukebox_core::paths::{default_root_in, DataDir, APP_DIR_NAME, DATA_DIR_ENV};

#[test]
fn uses_the_package_name_folder() {
    let base = tempfile::tempdir().unwrap();
    assert_eq!(default_root_in(base.path()), base.path().join(APP_DIR_NAME));
}

#[test]
fn falls_back_to_a_product_name_folder_only_when_it_is_the_one_that_exists() {
    let base = tempfile::tempdir().unwrap();
    std::fs::create_dir(base.path().join("Kyylan Jukebox")).unwrap();
    assert_eq!(
        default_root_in(base.path()),
        base.path().join("Kyylan Jukebox")
    );

    std::fs::create_dir(base.path().join(APP_DIR_NAME)).unwrap();
    assert_eq!(
        default_root_in(base.path()),
        base.path().join(APP_DIR_NAME),
        "the real folder wins when both exist"
    );
}

#[test]
fn resolves_to_electrons_user_data_folder_for_this_os() {
    // This binary holds the only test that touches the environment.
    std::env::remove_var(DATA_DIR_ENV);
    let root = DataDir::resolve().expect("a config directory");
    let shown = root.root().display().to_string();
    assert!(
        shown.ends_with(APP_DIR_NAME) || shown.ends_with("Kyylan Jukebox"),
        "{shown}"
    );

    #[cfg(target_os = "macos")]
    assert!(shown.contains("/Library/Application Support/"), "{shown}");
    #[cfg(target_os = "windows")]
    assert!(shown.contains("\\AppData\\Roaming\\"), "{shown}");
    #[cfg(target_os = "linux")]
    if std::env::var_os("XDG_CONFIG_HOME").is_none() {
        assert!(shown.contains("/.config/"), "{shown}");
    }

    assert_eq!(root.config_path(), root.root().join("config.json"));
    assert_eq!(root.database_path(), root.root().join("jukebox.db"));

    let custom = tempfile::tempdir().unwrap();
    std::env::set_var(DATA_DIR_ENV, custom.path());
    assert_eq!(DataDir::resolve().unwrap().root(), custom.path());
    std::env::remove_var(DATA_DIR_ENV);
}
