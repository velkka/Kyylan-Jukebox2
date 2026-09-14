//! The queue engine and the audio player together, unattended: a night's worth of guests'
//! songs play through in order, and the ones that can't be played are skipped without
//! stalling and without being logged as plays.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jukebox_audio::{AudioPlayer, VirtualBackend};
use jukebox_core::config::ConfigStore;
use jukebox_core::db;
use jukebox_core::engine::{Engine, MAX_FAILURES_IN_A_ROW};
use jukebox_core::library::query::track_path;
use jukebox_core::player::Player;

/// Library tracks, id = position + 1.
const LIBRARY: &[&str] = &[
    "tone.flac",      // 1
    "tone.mp3",       // 2
    "tone.opus",      // 3
    "tone.ogg",       // 4
    "tone.m4a",       // 5
    "tone-48k.flac",  // 6
    "tone-apple.m4a", // 7
    "he-aac.m4a",     // 8: unsupported
    "deleted.flac",   // 9: gone since the last scan
    "not-audio.mp3",  // 10: unreadable
];

struct Jukebox {
    _dir: tempfile::TempDir,
    engine: Arc<Engine>,
    db: std::path::PathBuf,
}

/// The engine and player wired as the server wires them.
fn jukebox(configure: impl FnOnce(&mut jukebox_core::config::AppConfig)) -> Jukebox {
    let dir = tempfile::tempdir().unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let config = Arc::new(ConfigStore::open(dir.path().join("config.json")).unwrap());
    config
        .update(|c| {
            c.per_user_queue_limit = 0;
            configure(c);
        })
        .unwrap();
    let database = dir.path().join("jukebox.db");
    let (conn, _) = db::open(&database).unwrap();
    for name in LIBRARY {
        conn.execute(
            "INSERT INTO tracks (path, title, mtime_ms, added_at) VALUES (?, ?, 0, 'x')",
            (fixtures.join(name).to_str().unwrap(), name),
        )
        .unwrap();
    }

    let backend = VirtualBackend::new(8.0);
    backend.add_device("speakers", "Speakers", 48_000, 2);
    let reads = Mutex::new(db::open_read_only(&database).unwrap());
    let resolver = Arc::new(move |id: i64| {
        track_path(&reads.lock().unwrap(), id)
            .ok()
            .flatten()
            .map(Into::into)
    });
    let player: Arc<dyn Player> = Arc::new(AudioPlayer::new(backend, resolver, None));
    let engine = Engine::new(config, player.clone(), conn);
    let (events, received) = std::sync::mpsc::channel();
    player.on_event(Arc::new(move |event| {
        let _ = events.send(event);
    }));
    let for_events = Arc::downgrade(&engine);
    std::thread::spawn(move || {
        for event in received {
            let Some(engine) = for_events.upgrade() else {
                break;
            };
            engine.handle_player_event(event).unwrap();
        }
    });
    Jukebox {
        _dir: dir,
        engine,
        db: database,
    }
}

impl Jukebox {
    fn history(&self) -> Vec<i64> {
        let conn = db::open_read_only(&self.db).unwrap();
        let mut stmt = conn
            .prepare("SELECT track_id FROM play_history ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn wait_until(&self, what: &str, mut done: impl FnMut(&Engine) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done(&self.engine) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_whole_queue_plays_through_unattended() {
    let jukebox = jukebox(|_| {});
    let queue = [1, 2, 8, 3, 9, 4, 5, 10, 6, 7, 1, 2];
    for (i, track) in queue.iter().enumerate() {
        let guest = format!("192.0.2.{}", i % 4 + 1);
        jukebox
            .engine
            .enqueue(*track, &guest, Some("guest"))
            .unwrap();
    }
    jukebox.wait_until("the queue to play out", |engine| {
        let state = engine.queue_state("host").unwrap();
        state.queue.is_empty() && state.now_playing.entry.is_none()
    });

    assert_eq!(
        jukebox.history(),
        [1, 2, 3, 4, 5, 6, 7, 1, 2],
        "every playable song, in order; the three that can't play left no trace"
    );
    let state = jukebox.engine.queue_state("host").unwrap();
    assert_eq!(
        state.now_playing.problem, None,
        "three scattered failures never stop playback"
    );
}

#[test]
fn five_failures_in_a_row_stop_until_someone_adds_a_song() {
    let jukebox = jukebox(|_| {});
    for track in [8, 9, 10, 8, 9, 1] {
        jukebox
            .engine
            .enqueue(track, "192.0.2.1", Some("guest"))
            .unwrap();
    }
    jukebox.wait_until("playback to stop", |engine| {
        engine
            .queue_state("host")
            .unwrap()
            .now_playing
            .problem
            .is_some()
    });
    let state = jukebox.engine.queue_state("host").unwrap();
    let problem = state.now_playing.problem.unwrap();
    assert!(
        problem.starts_with(&format!(
            "Playback stopped: the last {MAX_FAILURES_IN_A_ROW} songs couldn't be played."
        )),
        "{problem}"
    );
    assert!(
        problem.contains("“deleted.flac”"),
        "names the last one: {problem}"
    );
    assert!(state.now_playing.entry.is_none());
    assert_eq!(state.queue.len(), 1, "the playable song is still waiting");
    assert!(jukebox.history().is_empty());

    // A guest adding a song is a fresh start: the queue plays on and the notice goes.
    jukebox
        .engine
        .enqueue(2, "192.0.2.2", Some("guest"))
        .unwrap();
    jukebox.wait_until("the rest of the queue", |engine| {
        let state = engine.queue_state("host").unwrap();
        state.queue.is_empty() && state.now_playing.entry.is_none()
    });
    assert_eq!(jukebox.history(), [1, 2]);
    assert_eq!(
        jukebox
            .engine
            .queue_state("host")
            .unwrap()
            .now_playing
            .problem,
        None
    );
}
