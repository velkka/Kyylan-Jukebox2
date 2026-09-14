//! The program itself, run as each platform's start-up mechanism runs it: it scans at start,
//! plays, and when it's stopped mid-song the way that mechanism stops it — `SIGTERM` from
//! systemd or launchd, or Windows ending the session — it closes its connections, leaves the
//! database checkpointed and whole, and starts again on it.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const PROGRAM: &str = env!("CARGO_BIN_EXE_kyylan-jukebox");

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A mono 16-bit WAV of a quiet tone, long enough to stop in the middle of.
fn write_song(path: &Path, seconds: u32) {
    let rate = 22_050u32;
    let samples = rate * seconds;
    let mut wav = Vec::with_capacity(44 + samples as usize * 2);
    wav.extend(b"RIFF");
    wav.extend((36 + samples * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((samples * 2).to_le_bytes());
    for i in 0..samples {
        let t = f64::from(i) / f64::from(rate);
        let sample = ((t * 440.0 * std::f64::consts::TAU).sin() * 3000.0) as i16;
        wav.extend(sample.to_le_bytes());
    }
    fs::write(path, wav).unwrap();
}

struct Jukebox {
    dir: tempfile::TempDir,
    port: u16,
}

impl Jukebox {
    fn new(config: Value) -> Jukebox {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        fs::create_dir(&music).unwrap();
        write_song(&music.join("long song.wav"), 60);
        let port = free_port();
        let mut config = config;
        config["port"] = port.into();
        config["libraryPaths"] = serde_json::json!([music]);
        fs::create_dir(dir.path().join("data")).unwrap();
        fs::write(
            dir.path().join("data/config.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
        Jukebox { dir, port }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(PROGRAM);
        command
            .arg("--data-dir")
            .arg(self.data())
            .env("KYYLAN_AUDIO", "virtual")
            .env_remove("KYYLAN_DATA_DIR");
        command
    }

    /// Starts the program, its output collected in a file.
    fn start(&self) -> Child {
        let output = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.path().join("output.txt"))
            .unwrap();
        self.command()
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .stdin(Stdio::null())
            .spawn()
            .unwrap()
    }

    /// Everything the program logged, wherever this platform puts it.
    fn log(&self) -> String {
        let mut text = fs::read_to_string(self.dir.path().join("output.txt")).unwrap_or_default();
        if let Ok(files) = fs::read_dir(self.data().join("logs")) {
            for file in files {
                text += &fs::read_to_string(file.unwrap().path()).unwrap();
            }
        }
        text
    }

    fn request(&self, method: &str, path: &str, body: Option<&str>) -> Option<(u16, String)> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .ok()?;
        let body = body.unwrap_or("");
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .ok()?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw).ok()?;
        let status = raw.get(9..12)?.parse().ok()?;
        let body = raw.split_once("\r\n\r\n")?.1.to_string();
        Some((status, body))
    }

    fn get_json(&self, path: &str) -> Value {
        let (status, body) = self.request("GET", path, None).expect(path);
        assert_eq!(status, 200, "{path}: {body}");
        serde_json::from_str(&body).unwrap()
    }

    fn wait_until(&self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}. Log:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn database(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.data().join("jukebox.db")).unwrap()
    }
}

fn wait_for_exit(child: &mut Child, jukebox: &Jukebox) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("didn't exit. Log:\n{}", jukebox.log());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stops the program the way its platform's start-up mechanism does.
#[cfg(unix)]
fn stop_like_the_platform(child: &Child) {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

/// Windows signing out: the session-ending message every top-level window gets.
#[cfg(windows)]
fn stop_like_the_platform(child: &Child) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GetWindowThreadProcessId, PostMessageW, WM_ENDSESSION,
    };
    let class: Vec<u16> = "KyylanJukeboxSession\0".encode_utf16().collect();
    let mut window = std::ptr::null_mut();
    loop {
        window = unsafe {
            FindWindowExW(
                std::ptr::null_mut(),
                window,
                class.as_ptr(),
                std::ptr::null(),
            )
        };
        assert!(!window.is_null(), "no session window for the jukebox");
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if pid == child.id() {
            break;
        }
    }
    assert_ne!(unsafe { PostMessageW(window, WM_ENDSESSION, 1, 0) }, 0);
}

const STOPPED_BY: &str = if cfg!(windows) {
    "stopping: the Windows session is ending"
} else {
    "stopping: SIGTERM"
};

/// A WebSocket on /ws, handshake done.
fn live_updates(port: u16) -> TcpStream {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        socket,
        "GET /ws HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    )
    .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8];
    while !head.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    socket
}

/// Reads frames until the connection ends, returning the opcode of the last frame.
fn last_frame(socket: &mut TcpStream) -> Option<u8> {
    let mut last = None;
    let mut header = [0u8; 2];
    while socket.read_exact(&mut header).is_ok() {
        let mut length = u64::from(header[1] & 0x7f);
        if length == 126 {
            let mut extended = [0u8; 2];
            socket.read_exact(&mut extended).ok()?;
            length = u64::from(u16::from_be_bytes(extended));
        } else if length == 127 {
            let mut extended = [0u8; 8];
            socket.read_exact(&mut extended).ok()?;
            length = u64::from_be_bytes(extended);
        }
        let mut payload = vec![0u8; length as usize];
        socket.read_exact(&mut payload).ok()?;
        last = Some(header[0] & 0x0f);
    }
    last
}

#[test]
fn stopped_mid_song_it_closes_cleanly_and_starts_again() {
    let jukebox = Jukebox::new(serde_json::json!({
        "configured": true,
        "adminPassword": "service-test",
    }));
    let mut child = jukebox.start();

    // The library is scanned at start, with nobody asking.
    jukebox.wait_until("the start-up scan", || {
        jukebox
            .request("GET", "/api/tracks", None)
            .and_then(|(_, body)| serde_json::from_str::<Value>(&body).ok())
            .is_some_and(|tracks| tracks["tracks"].as_array().is_some_and(|t| t.len() == 1))
    });
    let tracks = jukebox.get_json("/api/tracks");
    let track = tracks["tracks"][0]["id"].as_i64().unwrap();
    let (status, body) = jukebox
        .request(
            "POST",
            "/api/queue",
            Some(&format!(r#"{{"trackId":{track}}}"#)),
        )
        .unwrap();
    assert_eq!(status, 200, "{body}");
    jukebox.wait_until("two seconds into the song", || {
        let state = jukebox.get_json("/api/player/state");
        state["playing"] == true && state["position"].as_f64().unwrap() > 2.0
    });

    let mut socket = live_updates(jukebox.port);
    stop_like_the_platform(&child);
    let status = wait_for_exit(&mut child, &jukebox);
    let log = jukebox.log();
    assert!(status.success(), "{status}. Log:\n{log}");
    assert!(log.contains(STOPPED_BY), "{log}");
    assert!(log.contains("stopped cleanly"), "{log}");
    assert_eq!(
        last_frame(&mut socket),
        Some(0x8),
        "live updates were closed, not dropped"
    );

    // Checkpointed: the database file holds everything, and it's whole.
    let wal = jukebox.data().join("jukebox.db-wal");
    assert!(
        fs::metadata(&wal).map_or(true, |m| m.len() == 0),
        "the write-ahead log was folded back in"
    );
    {
        let db = jukebox.database();
        let check: String = db
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        let plays: i64 = db
            .query_row(
                "SELECT count(*) FROM play_history WHERE track_id = ?",
                [track],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(plays, 1, "the song that was playing counts as played");
    }

    // And it starts again on what it left: as Electron did, the guest's song that was cut
    // off plays again from the top.
    let mut child = jukebox.start();
    jukebox.wait_until("the interrupted song to play again", || {
        jukebox.request("GET", "/api/health", None).is_some()
            && jukebox.get_json("/api/queue")["nowPlaying"]["entry"]["track"]["id"] == track
    });
    stop_like_the_platform(&child);
    assert!(wait_for_exit(&mut child, &jukebox).success());
}

/// Linux only: on a desktop the second copy opens the console in a browser.
#[cfg(target_os = "linux")]
#[test]
fn a_second_copy_on_the_same_data_leaves_the_first_running() {
    let jukebox = Jukebox::new(serde_json::json!({
        "configured": true,
        "adminPassword": "service-test",
    }));
    let mut first = jukebox.start();
    jukebox.wait_until("the first copy", || {
        jukebox.request("GET", "/api/health", None).is_some()
    });
    let second = jukebox.command().output().unwrap();
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("already running"));
    assert!(jukebox.request("GET", "/api/health", None).is_some());
    stop_like_the_platform(&first);
    assert!(wait_for_exit(&mut first, &jukebox).success());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_refuses_to_start_without_the_admin_password() {
    let jukebox = Jukebox::new(serde_json::json!({ "configured": true }));
    let output = jukebox.command().output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(78),
        "EX_CONFIG, which the unit doesn't restart on"
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("adminPassword is not set in"), "{text}");
    assert!(
        jukebox.request("GET", "/api/health", None).is_none(),
        "nothing is served"
    );
}

#[test]
fn check_config_reports_and_exits_non_zero_on_errors() {
    let jukebox = Jukebox::new(serde_json::json!({
        "configured": true,
        "adminPassword": "service-test",
        "outputDeviceId": "Virtual output",
    }));
    let output = jukebox.command().arg("--check-config").output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(text.trim_end().ends_with("config.json is valid"), "{text}");
    assert!(
        !text.contains("warning"),
        "a device matched by name: {text}"
    );

    fs::write(jukebox.data().join("config.json"), "{ \"port\": 80,, }").unwrap();
    let output = jukebox.command().arg("--check-config").output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("error: "), "{text}");
    assert!(text.contains("has 1 error"), "{text}");
}

#[test]
fn list_devices_prints_the_default_then_each_device() {
    let output = Command::new(PROGRAM)
        .arg("--list-devices")
        .env("KYYLAN_AUDIO", "virtual")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    assert_eq!(
        lines,
        [
            "ID       NAME",
            "default  Default - Virtual output",
            "virtual  Virtual output"
        ]
    );
}
