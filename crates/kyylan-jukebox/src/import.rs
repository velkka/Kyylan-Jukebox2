//! `kyylan-jukebox import <dir>`: brings a v0.2.x data directory — the Electron build kept it
//! in `~/.config/kyylan-jukebox` — into the Linux service's `/var/lib/kyylan-jukebox`.
//!
//! The service is stopped for the copy and started again afterwards. The database is copied
//! through SQLite's backup API, so a copy taken while the old app is still open is still a
//! consistent one. The files end up owned by the service's user, and any library folder that
//! user can't read is named, since the service would otherwise scan nothing from it.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use jukebox_core::config::{AppConfig, ConfigStore};
use jukebox_core::db;
use jukebox_core::paths::DataDir;
use rusqlite::Connection;

use crate::instance;

/// Where the system unit keeps its data.
pub const SERVICE_DATA_DIR: &str = "/var/lib/kyylan-jukebox";
pub const UNIT: &str = "kyylan-jukebox.service";
/// The database the import replaced, kept beside it.
pub const PREVIOUS_DATABASE: &str = "jukebox.db.before-import";
const IMPORTING_DATABASE: &str = "jukebox.db.importing";

/// The jukebox as a service: stopped for the copy, and asked whether it can read a folder.
pub trait Service {
    /// Stops the service if it's running, returning whether it was.
    fn stop(&self) -> Result<bool, String>;
    fn start(&self) -> Result<(), String>;
    /// Whether the user with this uid and gid can list a folder, or why not.
    fn can_read(&self, folder: &Path, uid: u32, gid: u32) -> Result<(), String>;
}

/// The system unit, through `systemctl`.
pub struct Systemd;

fn systemctl(args: &[&str]) -> io::Result<std::process::ExitStatus> {
    Command::new("systemctl").args(args).status()
}

impl Service for Systemd {
    fn stop(&self) -> Result<bool, String> {
        match systemctl(&["is-active", "--quiet", UNIT]) {
            Ok(status) if status.success() => {}
            // Not running, or no systemd at all.
            _ => return Ok(false),
        }
        match systemctl(&["stop", UNIT]) {
            Ok(status) if status.success() => Ok(true),
            _ => Err(format!("couldn't stop {UNIT}. Run the import with sudo.")),
        }
    }

    fn start(&self) -> Result<(), String> {
        match systemctl(&["start", UNIT]) {
            Ok(status) if status.success() => Ok(()),
            _ => Err(format!("couldn't start {UNIT}; see journalctl -u {UNIT}")),
        }
    }

    fn can_read(&self, folder: &Path, uid: u32, gid: u32) -> Result<(), String> {
        // As root, which the import runs as, every folder is readable. Ask as the service's
        // user instead, with its groups — `audio`, and any a music folder is shared through.
        if unsafe { libc::geteuid() } == 0 && uid != 0 {
            let checked = Command::new("setpriv")
                .args([
                    &format!("--reuid={uid}"),
                    &format!("--regid={gid}"),
                    "--init-groups",
                    "test",
                    "-r",
                ])
                .arg(folder)
                .args(["-a", "-x"])
                .arg(folder)
                .status();
            match checked {
                Ok(status) if status.success() => return Ok(()),
                Ok(_) => return Err("permission denied for the service's user".into()),
                // No setpriv: fall back to checking as ourselves.
                Err(_) => {}
            }
        }
        fs::read_dir(folder)
            .map(drop)
            .map_err(|err| err.to_string())
    }
}

/// Imports `source` into `destination`, reporting each step to `out`.
pub fn run(
    source: &Path,
    destination: &Path,
    service: &dyn Service,
    out: &mut dyn Write,
) -> Result<(), String> {
    let source_dir = DataDir::at(source);
    let destination_dir = DataDir::at(destination);
    for (file, name) in [
        (source_dir.config_path(), "config.json"),
        (source_dir.database_path(), "jukebox.db"),
    ] {
        if !file.is_file() {
            return Err(format!(
                "{} has no {name}. Point the import at the folder the old jukebox kept its data \
                 in, such as ~/.config/kyylan-jukebox.",
                source.display()
            ));
        }
    }
    if same_folder(source, destination) {
        return Err(format!(
            "{} is already the jukebox's data directory",
            destination.display()
        ));
    }
    let config = AppConfig::read(&source_dir.config_path()).map_err(|e| e.to_string())?;
    let source_db = db::open_read_only(&source_dir.database_path())
        .map_err(|e| format!("can't open {}: {e}", source_dir.database_path().display()))?;
    let recorded = db::current_migration(&source_db).map_err(|e| e.to_string())?;
    if recorded > db::latest_migration() {
        return Err(format!(
            "{} was last used by a newer version of Kyylan Jukebox than this one",
            source.display()
        ));
    }

    let say = |out: &mut dyn Write, line: String| {
        let _ = writeln!(out, "{line}");
    };
    say(
        out,
        format!(
            "Importing {} into {}",
            source.display(),
            destination.display()
        ),
    );
    let was_running = service.stop()?;
    if was_running {
        say(out, format!("Stopped {UNIT}"));
    }
    let copied = copy(&config, &source_db, &destination_dir, service, &mut *out);
    drop(source_db);
    if was_running {
        match service.start() {
            Ok(()) => say(out, format!("Started {UNIT}")),
            Err(err) if copied.is_ok() => return Err(err),
            Err(err) => say(out, format!("warning: {err}")),
        }
    } else if copied.is_ok() && destination == Path::new(SERVICE_DATA_DIR) {
        say(
            out,
            format!("Start the jukebox with: sudo systemctl start {UNIT}"),
        );
    }
    copied
}

/// The copy itself, with the service stopped.
fn copy(
    config: &AppConfig,
    source_db: &Connection,
    destination: &DataDir,
    service: &dyn Service,
    out: &mut dyn Write,
) -> Result<(), String> {
    let root = destination.root();
    fs::create_dir_all(root).map_err(|e| format!("can't create {}: {e}", root.display()))?;
    let lock = match instance::acquire(root) {
        Ok(lock) => lock,
        Err(instance::AcquireError::Held) => {
            return Err(format!(
                "a jukebox is still running on {}. Stop it first.",
                root.display()
            ))
        }
        Err(instance::AcquireError::Io(err)) => {
            return Err(format!("can't write to {}: {err}", root.display()))
        }
    };

    // The database: copied beside the live one, checked, then swapped in.
    let importing = root.join(IMPORTING_DATABASE);
    remove_database(&importing).map_err(|e| e.to_string())?;
    {
        let mut copy = Connection::open(&importing).map_err(|e| e.to_string())?;
        rusqlite::backup::Backup::new(source_db, &mut copy)
            .and_then(|backup| backup.run_to_completion(1024, Duration::ZERO, None))
            .map_err(|e| format!("copying the database failed: {e}"))?;
        copy.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        let check: String = copy
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if check != "ok" {
            return Err(format!("the copied database is damaged: {check}"));
        }
    }
    let live = destination.database_path();
    if live.exists() {
        let previous = root.join(PREVIOUS_DATABASE);
        remove_database(&previous).map_err(|e| e.to_string())?;
        move_database(&live, &previous).map_err(|e| e.to_string())?;
        let _ = writeln!(
            out,
            "Kept the database it replaced as {}",
            previous.display()
        );
    }
    move_database(&importing, &live).map_err(|e| e.to_string())?;
    let conn = db::open_read_only(&live).map_err(|e| e.to_string())?;
    let count = |table: &str| {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
            r.get::<_, i64>(0)
        })
    };
    let counts = (count("tracks"), count("play_history"));
    drop(conn);
    if let (Ok(tracks), Ok(plays)) = counts {
        let _ = writeln!(out, "Copied the database: {tracks} tracks, {plays} plays");
    }

    // The settings. An old install that was never set up has no admin password; the
    // service's own, generated at install, is kept then.
    let (store, _) = ConfigStore::open_or_defaults(destination.config_path());
    let replaced = store.get();
    let mut imported = config.clone();
    if imported.admin_password.is_empty() && !replaced.admin_password.is_empty() {
        imported.admin_password = replaced.admin_password.clone();
        imported.configured = replaced.configured;
        let _ = writeln!(
            out,
            "The imported settings had no admin password; kept this install's"
        );
    } else if !replaced.admin_password.is_empty()
        && replaced.admin_password != imported.admin_password
    {
        let _ = writeln!(
            out,
            "The admin password is now the one the imported jukebox used"
        );
    }
    let imported = store.update(|c| *c = imported).map_err(|e| e.to_string())?;
    let _ = writeln!(out, "Copied the settings to {}", store.path().display());
    if let Some(reason) =
        crate::check::refuses_to_start(&imported, store.path(), crate::check::Setup::InFile)
    {
        let _ = writeln!(out, "warning: {reason}");
    }

    // Ownership: everything here belongs to whoever owns the folder — the service's user.
    let owner = fs::metadata(root).map_err(|e| e.to_string())?;
    let (uid, gid) = (owner.uid(), owner.gid());
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.is_file() && (meta.uid() != uid || meta.gid() != gid) {
            std::os::unix::fs::chown(&path, Some(uid), Some(gid))
                .map_err(|e| format!("can't hand {} to the service: {e}", path.display()))?;
        }
    }
    drop(lock);

    for folder in &imported.library_paths {
        let path = Path::new(folder);
        if !path.is_dir() {
            let _ = writeln!(
                out,
                "warning: library folder {folder} doesn't exist on this machine"
            );
        } else if let Err(reason) = service.can_read(path, uid, gid) {
            let _ = writeln!(
                out,
                "warning: the jukebox can't read library folder {folder}: {reason}. Move the \
                 music somewhere it can read, such as /srv/music, or give it access; until then \
                 nothing from that folder plays."
            );
        }
    }
    Ok(())
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A database's files: the database, and its write-ahead log and index when there are any.
fn database_files(path: &Path) -> [PathBuf; 3] {
    let with = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    [path.to_path_buf(), with("-wal"), with("-shm")]
}

fn remove_database(path: &Path) -> io::Result<()> {
    for file in database_files(path) {
        match fs::remove_file(&file) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    Ok(())
}

/// Renames a database with its log, so what the log still holds stays with it.
fn move_database(from: &Path, to: &Path) -> io::Result<()> {
    for (from, to) in database_files(from).iter().zip(database_files(to)) {
        match fs::rename(from, &to) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use rusqlite::types::Value;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../jukebox-core/tests/fixtures/electron-v0.2.15")
    }

    /// The old app's data directory, as someone's home holds it.
    fn old_install(root: &Path) -> PathBuf {
        let dir = root.join("home/guest/.config/kyylan-jukebox");
        fs::create_dir_all(&dir).unwrap();
        for file in ["config.json", "jukebox.db"] {
            fs::copy(fixture().join(file), dir.join(file)).unwrap();
        }
        dir
    }

    /// What postinst leaves: a config marked as set up with a generated password, and the
    /// database the service created when it first started.
    fn fresh_service(root: &Path) -> PathBuf {
        let dir = root.join("var/lib/kyylan-jukebox");
        fs::create_dir_all(&dir).unwrap();
        ConfigStore::open(dir.join("config.json"))
            .unwrap()
            .update(|c| {
                c.configured = true;
                c.admin_password = "generated-at-install".into();
            })
            .unwrap();
        db::open(&dir.join("jukebox.db")).unwrap();
        dir
    }

    #[derive(Default)]
    struct FakeService {
        running: bool,
        unreadable: Vec<PathBuf>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl Service for FakeService {
        fn stop(&self) -> Result<bool, String> {
            self.calls.borrow_mut().push("stop");
            Ok(self.running)
        }
        fn start(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("start");
            Ok(())
        }
        fn can_read(&self, folder: &Path, _: u32, _: u32) -> Result<(), String> {
            match self.unreadable.iter().any(|f| f == folder) {
                true => Err("permission denied".into()),
                false => Ok(()),
            }
        }
    }

    fn import(
        source: &Path,
        destination: &Path,
        service: &FakeService,
    ) -> (Result<(), String>, String) {
        let mut out = Vec::new();
        let result = run(source, destination, service, &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    /// Every row of every table, schema included, in a comparable form.
    fn contents(path: &Path) -> Vec<(String, Vec<Vec<Value>>)> {
        let conn = db::open_read_only(path).unwrap();
        let mut tables: Vec<(String, String)> = conn
            .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        tables.retain(|(name, _)| !name.starts_with("sqlite_stat"));
        tables
            .into_iter()
            .map(|(name, sql)| {
                let mut stmt = conn.prepare(&format!("SELECT * FROM \"{name}\"")).unwrap();
                let columns = stmt.column_count();
                let rows = stmt
                    .query_map([], |r| (0..columns).map(|i| r.get::<_, Value>(i)).collect())
                    .unwrap()
                    .map(Result::unwrap)
                    .collect();
                (format!("{name}: {sql}"), rows)
            })
            .collect()
    }

    #[test]
    fn a_v0215_data_directory_comes_across() {
        let root = tempfile::tempdir().unwrap();
        let source = old_install(root.path());
        let destination = fresh_service(root.path());
        let replaced = contents(&destination.join("jukebox.db"));
        let service = FakeService {
            running: true,
            ..Default::default()
        };

        // Compared with a copy: even reading a database in WAL mode leaves files beside it,
        // and the fixture stays as committed.
        let original = old_install(&root.path().join("original"));
        let (result, out) = import(&source, &destination, &service);
        result.unwrap();

        assert_eq!(*service.calls.borrow(), ["stop", "start"]);
        assert_eq!(
            contents(&destination.join("jukebox.db")),
            contents(&original.join("jukebox.db")),
            "every table and row, as the old app left them"
        );
        assert_eq!(
            fs::read(destination.join("config.json")).unwrap(),
            fs::read(fixture().join("config.json")).unwrap(),
            "the settings, byte for byte"
        );
        assert_eq!(
            contents(&destination.join(PREVIOUS_DATABASE)),
            replaced,
            "the database it replaced is kept"
        );
        assert!(!destination.join(IMPORTING_DATABASE).exists());
        assert!(
            out.contains("Copied the database: 4 tracks, 2 plays"),
            "{out}"
        );
        assert!(
            out.contains("The admin password is now the one the imported jukebox used"),
            "{out}"
        );
        assert!(
            out.contains("warning: library folder /fixtures/music doesn't exist on this machine"),
            "{out}"
        );

        // The service starts on it as it is: nothing to migrate, and no lock left behind.
        let (_, report) = db::open(&destination.join("jukebox.db")).unwrap();
        assert!(report.applied.is_empty());
        assert!(instance::acquire(&destination).is_ok());
    }

    #[test]
    fn what_the_old_app_hasnt_checkpointed_yet_comes_too() {
        let root = tempfile::tempdir().unwrap();
        let source = old_install(root.path());
        let destination = fresh_service(root.path());
        // The old app still open, with a change only in its write-ahead log.
        let (writer, _) = db::open(&source.join("jukebox.db")).unwrap();
        writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        writer
            .execute(
                "INSERT INTO tracks (path, title, mtime_ms, added_at) VALUES ('/m/new.flac', 'New', 0, 'x')",
                [],
            )
            .unwrap();
        assert!(fs::metadata(source.join("jukebox.db-wal")).unwrap().len() > 0);

        let (result, out) = import(&source, &destination, &FakeService::default());
        result.unwrap();
        assert!(out.contains("Copied the database: 5 tracks"), "{out}");
        drop(writer);
    }

    #[test]
    fn unreadable_library_folders_are_named() {
        let root = tempfile::tempdir().unwrap();
        let source = old_install(root.path());
        let destination = fresh_service(root.path());
        let (music, shared) = (
            root.path().join("home/guest/Music"),
            root.path().join("srv/music"),
        );
        fs::create_dir_all(&music).unwrap();
        fs::create_dir_all(&shared).unwrap();
        ConfigStore::open(source.join("config.json"))
            .unwrap()
            .update(|c| {
                c.library_paths = vec![
                    music.to_str().unwrap().into(),
                    shared.to_str().unwrap().into(),
                ]
            })
            .unwrap();
        let service = FakeService {
            unreadable: vec![music.clone()],
            ..Default::default()
        };
        let (result, out) = import(&source, &destination, &service);
        result.unwrap();
        let warnings: Vec<&str> = out.lines().filter(|l| l.starts_with("warning")).collect();
        assert_eq!(warnings.len(), 1, "{out}");
        assert!(
            warnings[0].starts_with(&format!(
                "warning: the jukebox can't read library folder {}: permission denied.",
                music.display()
            )),
            "{out}"
        );
        assert_eq!(
            *service.calls.borrow(),
            ["stop"],
            "it wasn't running, so it stays stopped"
        );
    }

    #[test]
    fn an_install_never_set_up_keeps_the_generated_password() {
        let root = tempfile::tempdir().unwrap();
        let source = old_install(root.path());
        ConfigStore::open(source.join("config.json"))
            .unwrap()
            .update(|c| {
                c.configured = false;
                c.admin_password.clear();
            })
            .unwrap();
        let destination = fresh_service(root.path());
        let (result, out) = import(&source, &destination, &FakeService::default());
        result.unwrap();
        let config = AppConfig::read(&destination.join("config.json")).unwrap();
        assert_eq!(config.admin_password, "generated-at-install");
        assert!(config.configured);
        assert_eq!(config.port, 8094, "the rest is the imported settings");
        assert!(out.contains("kept this install's"), "{out}");
        assert!(!out.contains("warning: adminPassword"), "{out}");
    }

    #[test]
    fn a_running_jukebox_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let source = old_install(root.path());
        let destination = fresh_service(root.path());
        let before = contents(&destination.join("jukebox.db"));
        let _running = instance::acquire(&destination).ok().unwrap();
        let service = FakeService {
            running: true,
            ..Default::default()
        };
        let (result, _) = import(&source, &destination, &service);
        assert!(result.unwrap_err().contains("still running"));
        assert_eq!(contents(&destination.join("jukebox.db")), before);
        assert_eq!(
            *service.calls.borrow(),
            ["stop", "start"],
            "the service it stopped is started again"
        );
    }

    #[test]
    fn a_folder_without_old_data_or_the_service_folder_itself_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let destination = fresh_service(root.path());
        let service = FakeService::default();
        let empty = root.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let (result, _) = import(&empty, &destination, &service);
        assert!(result.unwrap_err().contains("has no config.json"));
        let (result, _) = import(&destination, &destination, &service);
        assert!(result
            .unwrap_err()
            .contains("is already the jukebox's data directory"));
        assert!(
            service.calls.borrow().is_empty(),
            "refused before stopping anything"
        );
    }
}
