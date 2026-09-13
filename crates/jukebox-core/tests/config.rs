//! config.json must be readable and writable interchangeably with the Electron build. The
//! expected bytes in tests/fixtures were produced by Node's own `JSON.stringify(…, null, 2)`.

use std::fs;

use jukebox_core::config::{AppConfig, ConfigError, ConfigStore};

const DEFAULTS: &str = include_str!("fixtures/config-defaults.json");
const ELECTRON_WRITTEN: &str = include_str!("fixtures/config-electron-written.json");
const UNKNOWN_KEY_INPUT: &str = include_str!("fixtures/config-with-unknown-key.input.json");
const UNKNOWN_KEY_SAVED: &str = include_str!("fixtures/config-with-unknown-key.saved.json");

#[test]
fn defaults_serialize_exactly_like_json_stringify() {
    assert_eq!(AppConfig::default().to_json(), DEFAULTS);
}

#[test]
fn missing_file_is_created_with_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/data/config.json");
    let store = ConfigStore::open(&path).unwrap();
    assert_eq!(store.get(), AppConfig::default());
    assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULTS);
}

#[test]
fn a_file_electron_wrote_saves_back_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    fs::write(&path, ELECTRON_WRITTEN).unwrap();

    let store = ConfigStore::open(&path).unwrap();
    let config = store.get();
    assert_eq!(config.port, 8090);
    assert_eq!(config.per_user_queue_limit, -3);
    assert_eq!(config.library_paths[1], "D:\\Musiikki\\Äänet");
    assert_eq!(config.admin_password, "party \"hat\" ö");

    store.update(|_| {}).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), ELECTRON_WRITTEN);
}

#[test]
fn missing_keys_come_from_defaults_and_unknown_keys_survive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    fs::write(&path, UNKNOWN_KEY_INPUT).unwrap();

    let store = ConfigStore::open(&path).unwrap();
    assert_eq!(store.get().port, 9000);
    assert_eq!(store.get().per_user_queue_limit, 3, "filled from defaults");

    store.update(|_| {}).unwrap();
    // config.ts saves `{ ...DEFAULT_CONFIG, ...raw }`: known keys in default order, then the
    // unknown ones. Node produced the expected file.
    assert_eq!(fs::read_to_string(&path).unwrap(), UNKNOWN_KEY_SAVED);
}

#[test]
fn update_changes_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let store = ConfigStore::open(&path).unwrap();
    let updated = store.update(|c| c.add_rate_limit_minutes = 5).unwrap();
    assert_eq!(updated.add_rate_limit_minutes, 5);
    assert_eq!(AppConfig::read(&path).unwrap().add_rate_limit_minutes, 5);
    assert!(
        !dir.path().join("config.json.tmp").exists(),
        "no temp file left behind"
    );
}

#[test]
fn a_broken_file_is_an_error_and_is_kept_when_settings_are_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let broken = "{\n  \"port\": 8090,\n  \"adminPassword\": \"oops\"\n  \"configured\": true\n}";
    fs::write(&path, broken).unwrap();

    match ConfigStore::open(&path) {
        Err(ConfigError::Invalid { line, .. }) => assert_eq!(line, 4),
        other => panic!(
            "expected an Invalid error, got {:?}",
            other.map(|s| s.get())
        ),
    }

    let (store, problem) = ConfigStore::open_or_defaults(&path);
    assert!(matches!(problem, Some(ConfigError::Invalid { .. })));
    assert_eq!(store.get(), AppConfig::default());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        broken,
        "untouched until a save"
    );

    store.update(|c| c.port = 9100).unwrap();
    let kept: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("config.json.invalid-"))
        .collect();
    assert_eq!(
        kept.len(),
        1,
        "the unreadable file is set aside, not overwritten"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(&kept[0])).unwrap(),
        broken
    );
    assert_eq!(AppConfig::read(&path).unwrap().port, 9100);
}

#[test]
fn the_password_never_shows_in_debug_output() {
    let config = AppConfig::from_json(ELECTRON_WRITTEN).unwrap();
    let debug = format!("{config:?}");
    assert!(!debug.contains("party"), "{debug}");
    assert!(debug.contains("<redacted>"));
}

/// Re-saves real config files an Electron build wrote, when provided, and requires the
/// bytes to come back unchanged. Set `KYYLAN_CONFIG_FILES` to a path-separated list; each
/// file is copied to a temporary folder first, so the originals are never touched.
#[test]
fn real_electron_config_files_save_back_byte_for_byte() {
    let Some(list) = std::env::var_os("KYYLAN_CONFIG_FILES") else {
        eprintln!("skipped: set KYYLAN_CONFIG_FILES to config.json files written by Electron");
        return;
    };
    for original in std::env::split_paths(&list) {
        let bytes = fs::read(&original).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("config.json");
        fs::write(&copy, &bytes).unwrap();
        ConfigStore::open(&copy).unwrap().update(|_| {}).unwrap();
        assert_eq!(
            fs::read(&copy).unwrap(),
            bytes,
            "{} did not survive a save",
            original.display()
        );
        eprintln!("ok: {}", original.display());
    }
}
