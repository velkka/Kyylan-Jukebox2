//! `sudo kyylan-jukebox uninstall` on macOS. A pkg has no uninstaller, so this is it: the
//! LaunchAgent is booted out and removed, the app and the command-line link are deleted, and
//! the package receipt is forgotten. Everyone's library, settings and stats stay.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The LaunchAgent's label, which is also the package identifier.
pub const LABEL: &str = "org.kyylan.jukebox";

/// Where the pkg puts things.
pub struct Layout {
    pub agent: PathBuf,
    pub app: PathBuf,
    /// A link on `PATH` to the program inside the app.
    pub command: PathBuf,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            agent: PathBuf::from(format!("/Library/LaunchAgents/{LABEL}.plist")),
            app: PathBuf::from("/Applications/Kyylan Jukebox.app"),
            command: PathBuf::from("/usr/local/bin/kyylan-jukebox"),
        }
    }
}

/// launchd and the package database.
pub trait System {
    /// Unloads the agent from a user's GUI session — which stops a running jukebox — and
    /// returns whether it was loaded there.
    fn bootout(&self, uid: u32) -> bool;
    /// Forgets the package receipt, returning whether there was one.
    fn forget_receipt(&self) -> bool;
}

pub struct Launchctl;

impl System for Launchctl {
    fn bootout(&self, uid: u32) -> bool {
        Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{LABEL}")])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn forget_receipt(&self) -> bool {
        Command::new("pkgutil")
            .args(["--forget", LABEL])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

/// The users whose sessions may have the agent loaded: whoever is at the Mac, and whoever
/// ran sudo.
pub fn session_users() -> Vec<u32> {
    let mut uids = Vec::new();
    if let Ok(console) = fs::metadata("/dev/console") {
        uids.push(console.uid());
    }
    if let Some(uid) = std::env::var("SUDO_UID").ok().and_then(|u| u.parse().ok()) {
        uids.push(uid);
    }
    uids.retain(|&uid| uid != 0);
    uids.dedup();
    uids
}

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn run(
    layout: &Layout,
    users: &[u32],
    system: &dyn System,
    out: &mut dyn Write,
) -> Result<(), String> {
    let mut say = |line: String| {
        let _ = writeln!(out, "{line}");
    };
    for &uid in users {
        if system.bootout(uid) {
            say(format!("Stopped the jukebox for user {uid}"));
        }
    }
    let fail = |what: &Path, err: io::Error| match err.kind() {
        io::ErrorKind::PermissionDenied => format!(
            "can't remove {}: permission denied. Run it with sudo: sudo kyylan-jukebox uninstall",
            what.display()
        ),
        _ => format!("can't remove {}: {err}", what.display()),
    };

    match fs::remove_file(&layout.agent) {
        Ok(()) => say(format!("Removed {}", layout.agent.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(fail(&layout.agent, err)),
    }
    // Only a link into the app: anything else at that path isn't ours.
    if let Ok(target) = fs::read_link(&layout.command) {
        if target.starts_with(&layout.app) {
            fs::remove_file(&layout.command).map_err(|e| fail(&layout.command, e))?;
            say(format!("Removed {}", layout.command.display()));
        }
    }
    match fs::symlink_metadata(&layout.app) {
        Ok(_) => {
            fs::remove_dir_all(&layout.app).map_err(|e| fail(&layout.app, e))?;
            say(format!("Removed {}", layout.app.display()));
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(fail(&layout.app, err)),
    }
    if system.forget_receipt() {
        say(format!("Forgot the package receipt {LABEL}"));
    }
    say(
        "Kyylan Jukebox is uninstalled. Each user's library, settings and stats stay in \
         ~/Library/Application Support/kyylan-jukebox, and logs in ~/Library/Logs/kyylan-jukebox."
            .into(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct FakeSystem {
        loaded_for: Vec<u32>,
        calls: RefCell<Vec<String>>,
    }

    impl System for FakeSystem {
        fn bootout(&self, uid: u32) -> bool {
            self.calls.borrow_mut().push(format!("bootout {uid}"));
            self.loaded_for.contains(&uid)
        }
        fn forget_receipt(&self) -> bool {
            self.calls.borrow_mut().push("forget".into());
            true
        }
    }

    fn installed(root: &Path) -> Layout {
        let layout = Layout {
            agent: root.join("Library/LaunchAgents/org.kyylan.jukebox.plist"),
            app: root.join("Applications/Kyylan Jukebox.app"),
            command: root.join("usr/local/bin/kyylan-jukebox"),
        };
        let program = layout.app.join("Contents/MacOS/kyylan-jukebox");
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, "").unwrap();
        fs::create_dir_all(layout.agent.parent().unwrap()).unwrap();
        fs::write(&layout.agent, "<plist/>").unwrap();
        fs::create_dir_all(layout.command.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&program, &layout.command).unwrap();
        layout
    }

    #[test]
    fn removes_the_agent_the_app_and_the_receipt() {
        let root = tempfile::tempdir().unwrap();
        let layout = installed(root.path());
        let data = root
            .path()
            .join("Users/host/Library/Application Support/kyylan-jukebox");
        fs::create_dir_all(&data).unwrap();
        let system = FakeSystem {
            loaded_for: vec![501],
            ..Default::default()
        };
        let mut out = Vec::new();
        run(&layout, &[501, 502], &system, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();

        assert_eq!(
            *system.calls.borrow(),
            ["bootout 501", "bootout 502", "forget"]
        );
        assert!(!layout.agent.exists());
        assert!(!layout.app.exists());
        assert!(fs::symlink_metadata(&layout.command).is_err());
        assert!(data.is_dir(), "data stays");
        assert!(
            out.starts_with("Stopped the jukebox for user 501\n"),
            "{out}"
        );
        assert!(!out.contains("user 502"), "{out}");

        // Again, with everything already gone: nothing to do, and no error.
        run(&layout, &[501], &FakeSystem::default(), &mut Vec::new()).unwrap();
    }

    #[test]
    fn a_command_that_isnt_a_link_into_the_app_is_left() {
        let root = tempfile::tempdir().unwrap();
        let layout = installed(root.path());
        fs::remove_file(&layout.command).unwrap();
        fs::write(&layout.command, "someone else's").unwrap();
        run(&layout, &[], &FakeSystem::default(), &mut Vec::new()).unwrap();
        assert!(layout.command.is_file());
    }
}
