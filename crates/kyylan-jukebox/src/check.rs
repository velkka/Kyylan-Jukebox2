//! `--check-config`: what's wrong with config.json before the program is started with it, for
//! someone who edited the file by hand. The rules the program itself refuses to start on
//! live here too, so the check and the start never disagree.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use jukebox_core::config::{AppConfig, ConfigError};
use jukebox_core::types::AudioDevice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The program won't start, or won't work.
    Error,
    /// It starts, but probably not as intended.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{label}: {}", self.message)
    }
}

fn error(message: impl Into<String>) -> Finding {
    Finding {
        severity: Severity::Error,
        message: message.into(),
    }
}

fn warning(message: impl Into<String>) -> Finding {
    Finding {
        severity: Severity::Warning,
        message: message.into(),
    }
}

/// Whether the install is set up in the file, with no setup page: the Linux service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    /// Setup is done from the console in a browser, on the host.
    InBrowser,
    /// The file must already hold the admin password.
    InFile,
}

impl Setup {
    pub const PLATFORM: Setup = if cfg!(target_os = "linux") {
        Setup::InFile
    } else {
        Setup::InBrowser
    };
}

/// What stops the program starting with this config, on a platform set up in the file.
pub fn refuses_to_start(config: &AppConfig, path: &Path, setup: Setup) -> Option<String> {
    if setup != Setup::InFile {
        return None;
    }
    if config.admin_password.is_empty() {
        return Some(format!(
            "adminPassword is not set in {}. Set it, and \"configured\": true, then start again.",
            path.display()
        ));
    }
    if !config.configured {
        return Some(format!(
            "\"configured\" is false in {}. With no setup page on this platform, set it to true.",
            path.display()
        ));
    }
    None
}

/// Checks a config file. `devices` is the output device list, when there's one to check
/// `outputDeviceId` against.
pub fn check(path: &Path, setup: Setup, devices: Option<&[AudioDevice]>) -> Vec<Finding> {
    let config = match AppConfig::read(path) {
        Ok(config) => config,
        Err(ConfigError::Read { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return vec![match setup {
                Setup::InFile => error(format!(
                    "{} doesn't exist. Create it with adminPassword set and \"configured\": true.",
                    path.display()
                )),
                Setup::InBrowser => warning(format!(
                    "{} doesn't exist yet; it's created with the defaults at the first start.",
                    path.display()
                )),
            }];
        }
        Err(err) => return vec![error(err.to_string())],
    };

    let mut findings = Vec::new();
    if let Some(reason) = refuses_to_start(&config, path, setup) {
        findings.push(error(reason));
    }
    if config.port == 0 {
        findings.push(error("port must be between 1 and 65535"));
    }
    for key in config.extra.keys() {
        findings.push(warning(format!("\"{key}\" isn't a setting; it's ignored")));
    }
    for folder in &config.library_paths {
        match fs::metadata(folder) {
            Ok(meta) if meta.is_dir() => {
                if let Err(err) = fs::read_dir(folder) {
                    findings.push(warning(format!(
                        "library folder {folder} can't be read: {err}"
                    )));
                }
            }
            Ok(_) => findings.push(warning(format!("library folder {folder} isn't a folder"))),
            Err(_) => findings.push(warning(format!("library folder {folder} doesn't exist"))),
        }
    }
    if let (Some(wanted), Some(devices)) = (config.output_device_id.as_deref(), devices) {
        let known = wanted.is_empty()
            || wanted == jukebox_audio::DEFAULT_DEVICE
            || devices
                .iter()
                .any(|d| d.device_id == wanted || d.label == wanted);
        if !known {
            findings.push(warning(format!(
                "output device \"{wanted}\" isn't connected; the default output plays instead \
                 (--list-devices shows the ones that are)"
            )));
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) -> std::path::PathBuf {
        let path = dir.join("config.json");
        fs::write(&path, text).unwrap();
        path
    }

    fn messages(findings: &[Finding]) -> Vec<String> {
        findings.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_file_the_electron_build_wrote_is_valid() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../jukebox-core/tests/fixtures/electron-v0.2.15/config.json");
        let findings = check(&fixture, Setup::InFile, Some(&[]));
        assert_eq!(
            messages(&findings),
            ["warning: library folder /fixtures/music doesn't exist"]
        );
    }

    #[test]
    fn broken_json_is_an_error_with_its_position() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "{\n  \"port\": 8080,\n}");
        let findings = check(&path, Setup::InBrowser, None);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("line 3"), "{}", findings[0]);
    }

    #[test]
    fn a_wrong_type_or_range_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            r#"{"port": "8080"}"#,
            r#"{"port": 70000}"#,
            r#"{"sameSongCooldownMinutes": -5}"#,
        ] {
            let findings = check(&write(dir.path(), text), Setup::InBrowser, None);
            assert_eq!(findings.len(), 1, "{text}: {findings:?}");
            assert_eq!(findings[0].severity, Severity::Error, "{text}");
        }
        let findings = check(&write(dir.path(), r#"{"port": 0}"#), Setup::InBrowser, None);
        assert_eq!(
            messages(&findings),
            ["error: port must be between 1 and 65535"]
        );
    }

    #[test]
    fn the_service_needs_the_admin_password_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let unset = write(dir.path(), r#"{"configured": true}"#);
        assert!(check(&unset, Setup::InBrowser, None).is_empty());
        let findings = check(&unset, Setup::InFile, None);
        assert_eq!(findings.len(), 1);
        assert!(findings[0]
            .message
            .starts_with("adminPassword is not set in"));

        let unconfigured = write(dir.path(), r#"{"adminPassword": "pw"}"#);
        let findings = check(&unconfigured, Setup::InFile, None);
        assert!(findings[0].message.starts_with("\"configured\" is false"));

        let missing = dir.path().join("elsewhere.json");
        assert_eq!(
            check(&missing, Setup::InFile, None)[0].severity,
            Severity::Error
        );
        assert_eq!(
            check(&missing, Setup::InBrowser, None)[0].severity,
            Severity::Warning
        );
    }

    #[test]
    fn typos_missing_folders_and_unknown_devices_are_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        fs::create_dir(&music).unwrap();
        let text = serde_json::json!({
            "configured": true,
            "adminPasword": "typo",
            "adminPassword": "pw",
            "libraryPaths": [music, dir.path().join("gone")],
            "outputDeviceId": "Living room",
        });
        let path = write(dir.path(), &text.to_string());
        let devices = [AudioDevice {
            device_id: "hdmi".into(),
            label: "HDMI".into(),
        }];
        let findings = check(&path, Setup::InFile, Some(&devices));
        assert!(findings.iter().all(|f| f.severity == Severity::Warning));
        let text = messages(&findings).join("\n");
        assert!(text.contains("\"adminPasword\" isn't a setting"), "{text}");
        assert!(text.contains("gone doesn't exist"), "{text}");
        assert!(
            text.contains("output device \"Living room\" isn't connected"),
            "{text}"
        );
        assert_eq!(findings.len(), 3, "{text}");

        for known in ["HDMI", "hdmi", "default"] {
            let text = serde_json::json!({"outputDeviceId": known});
            let path = write(dir.path(), &text.to_string());
            assert!(
                check(&path, Setup::InBrowser, Some(&devices)).is_empty(),
                "{known}"
            );
        }
    }
}
