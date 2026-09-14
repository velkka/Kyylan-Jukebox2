//! The engine keeps queue.ts's guarantees when requests arrive at the same moment: Node ran
//! one request at a time, the Rust server doesn't, and the engine's lock has to make up for
//! it.

use std::sync::{Arc, Barrier};

use jukebox_core::config::ConfigStore;
use jukebox_core::db;
use jukebox_core::engine::{Engine, EngineError};
use jukebox_core::player::SilentPlayer;

fn engine_with_tracks(
    tracks: usize,
    configure: impl FnOnce(&mut jukebox_core::config::AppConfig),
) -> (tempfile::TempDir, Arc<Engine>) {
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(ConfigStore::open(dir.path().join("config.json")).unwrap());
    config.update(configure).unwrap();
    let (conn, _) = db::open(&dir.path().join("jukebox.db")).unwrap();
    for i in 0..tracks {
        conn.execute(
            "INSERT INTO tracks (path, title, artist, mtime_ms, added_at) VALUES (?, ?, ?, 0, 'x')",
            (
                format!("/music/{i}.mp3"),
                format!("Song {i}"),
                format!("Artist {i}"),
            ),
        )
        .unwrap();
    }
    let engine = Engine::new(config, Arc::new(SilentPlayer::new()), conn);
    (dir, engine)
}

/// Runs `op` on `n` threads released at the same instant, collecting the results.
fn at_once<T: Send + 'static>(n: usize, op: impl Fn(usize) -> T + Send + Sync + 'static) -> Vec<T> {
    let barrier = Arc::new(Barrier::new(n));
    let op = Arc::new(op);
    let threads: Vec<_> = (0..n)
        .map(|i| {
            let (barrier, op) = (barrier.clone(), op.clone());
            std::thread::spawn(move || {
                barrier.wait();
                op(i)
            })
        })
        .collect();
    threads.into_iter().map(|t| t.join().unwrap()).collect()
}

#[test]
fn simultaneous_adds_stop_at_the_per_guest_limit() {
    let (_dir, engine) = engine_with_tracks(16, |c| c.per_user_queue_limit = 2);
    let adder = engine.clone();
    let results = at_once(16, move |i| {
        adder.enqueue(i as i64 + 1, "192.0.2.21", Some("phone"))
    });

    let accepted = results.iter().filter(|r| r.is_ok()).count();
    // The first add starts playing at once, so it isn't pending; two more fill the limit.
    assert_eq!(accepted, 3, "{results:?}");
    for rejected in results.iter().filter_map(|r| r.as_ref().err()) {
        match rejected {
            EngineError::Rejected {
                status: 409,
                message,
            } => {
                assert_eq!(message, "You can have at most 2 songs in the queue.")
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    let state = engine.queue_state("192.0.2.21").unwrap();
    assert!(state.now_playing.entry.is_some());
    assert_eq!((state.queue.len(), state.my_queue_count), (2, 2));
}

#[test]
fn simultaneous_downvotes_each_count_once() {
    let (_dir, engine) = engine_with_tracks(3, |c| c.downvote_skip_threshold = 3);
    for track in 1..=3 {
        engine.enqueue(track, "192.0.2.21", None).unwrap();
    }
    let voter = engine.clone();
    at_once(12, move |i| {
        voter
            .downvote(&format!("192.0.2.{}", 100 + i), None)
            .unwrap()
    });

    // Twelve voters against a threshold of three, one at a time: three votes skip each
    // of the three songs, and the last three find nothing playing and aren't counted. Any
    // interleaving that lost or double-counted a vote would end differently.
    let state = engine.queue_state("192.0.2.21").unwrap();
    assert!(state.now_playing.entry.is_none() && state.queue.is_empty());
    assert_eq!(engine.stats(100, 10, true).unwrap().totals.downvotes, 9);
}

fn playing_track(engine: &Engine) -> Option<i64> {
    engine
        .queue_state("nobody")
        .unwrap()
        .now_playing
        .entry
        .map(|e| e.track.id)
}

fn play_history(engine: &Engine) -> Vec<i64> {
    let db = engine.db().lock().unwrap();
    let mut stmt = db
        .prepare("SELECT track_id FROM play_history ORDER BY id DESC")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn random_fill_avoids_the_last_fifty_plays() {
    let (_dir, engine) = engine_with_tracks(60, |c| c.standby_random_enabled = true);
    for _ in 0..40 {
        let recent: Vec<i64> = play_history(&engine).into_iter().take(50).collect();
        engine.advance().unwrap();
        let picked = playing_track(&engine).expect("random fill always finds a track");
        assert!(
            !recent.contains(&picked),
            "picked {picked}, played within the last 50: {recent:?}"
        );
    }
}

#[test]
fn random_fill_falls_back_to_a_recent_track_in_a_small_library() {
    let (_dir, engine) = engine_with_tracks(3, |c| c.standby_random_enabled = true);
    for _ in 0..10 {
        engine.advance().unwrap();
        assert!(
            playing_track(&engine).is_some(),
            "a library smaller than the window still fills"
        );
    }
    assert_eq!(play_history(&engine).len(), 10);
}

#[test]
fn shuffled_standby_never_repeats_back_to_back() {
    let (_dir, engine) = engine_with_tracks(5, |c| {
        c.standby_enabled = true;
        c.standby_shuffle = true;
    });
    for track in 1..=5 {
        engine.add_standby(track).unwrap();
    }
    let mut previous = None;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..60 {
        engine.advance().unwrap();
        let track = playing_track(&engine).unwrap();
        // With five tracks and eight retries, a repeat slips through once in ~2 million picks.
        assert_ne!(
            Some(track),
            previous,
            "the same standby track twice in a row"
        );
        seen.insert(track);
        previous = Some(track);
    }
    assert_eq!(seen.len(), 5, "shuffle reaches every track");
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Every queue operation from many threads at once, with random fill on and a rescan
/// running through the same database, then the queue's invariants checked. A lock ordering
/// mistake shows up as a hang, which the watchdog turns into a failure.
#[test]
fn a_busy_queue_keeps_its_invariants() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    copy_dir(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library"),
        &music,
    );
    let config = Arc::new(ConfigStore::open(dir.path().join("config.json")).unwrap());
    config
        .update(|c| {
            c.per_user_queue_limit = 3;
            c.downvote_skip_threshold = 2;
            c.standby_random_enabled = true;
        })
        .unwrap();
    let (conn, _) = db::open(&dir.path().join("jukebox.db")).unwrap();
    let engine = Engine::new(config, Arc::new(SilentPlayer::new()), conn);
    let scanner = Arc::new(jukebox_core::library::Scanner::new());
    let roots = vec![music.to_str().unwrap().to_string()];
    scanner.scan(engine.db(), &roots);
    let tracks: i64 = engine
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap();

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = engine.clone();
    let rescans = (scanner.clone(), roots.clone());
    std::thread::spawn(move || {
        let busy = worker.clone();
        let rescan = std::thread::spawn(move || {
            for _ in 0..5 {
                rescans.0.scan(busy.db(), &rescans.1);
            }
        });
        at_once(8, move |thread| {
            let ip = format!("192.0.2.{}", thread % 4 + 1);
            for i in 0..150 {
                let track = (thread as i64 * 7 + i) % tracks + 1;
                let _ = match rand::random_range(0..8) {
                    0..=2 => worker.enqueue(track, &ip, Some("guest")),
                    3 => worker.downvote(&ip, None),
                    4 => worker.remove_entry((i % 40) as f64, &ip, thread == 0),
                    5 => worker.reorder((i % 40) as f64, (i % 5) as f64),
                    6 => worker.advance(),
                    _ => worker.queue_state(&ip).map(|_| ()),
                };
            }
        });
        rescan.join().unwrap();
        done_tx.send(()).unwrap();
    });
    done_rx
        .recv_timeout(std::time::Duration::from_secs(120))
        .expect("the engine deadlocked or stalled");

    let db = engine.db().lock().unwrap();
    let count = |sql: &str| -> i64 { db.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert!(count("SELECT COUNT(*) FROM queue WHERE status = 'playing'") <= 1);
    assert_eq!(
        count("SELECT COUNT(*) FROM queue WHERE status = 'pending' AND added_by_ip LIKE '\\_\\_%' ESCAPE '\\'"),
        0,
        "filler is never left waiting in the queue"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM (SELECT added_by_ip FROM queue WHERE status = 'pending' GROUP BY added_by_ip HAVING COUNT(*) > 3)"),
        0,
        "no guest ever got past the limit"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM queue WHERE track_id NOT IN (SELECT id FROM tracks)"),
        0
    );
}

fn history_len(engine: &Engine) -> usize {
    play_history(engine).len()
}

#[test]
fn an_event_about_a_replaced_song_is_ignored() {
    use jukebox_core::player::PlayerEvent;

    let (_dir, engine) = engine_with_tracks(3, |c| c.per_user_queue_limit = 0);
    for track in 1..=3 {
        engine.enqueue(track, "192.0.2.21", None).unwrap();
    }
    let first = engine.player().current_load().unwrap();
    // The first song is skipped just as it ends: its "ended" arrives after the skip.
    engine.skip().unwrap();
    engine
        .handle_player_event(PlayerEvent::Ended { load: first })
        .unwrap();
    assert_eq!(
        playing_track(&engine),
        Some(2),
        "the second song keeps playing"
    );
    let failed = PlayerEvent::Failed {
        load: first,
        reason: "late".into(),
    };
    engine.handle_player_event(failed).unwrap();
    assert_eq!(playing_track(&engine), Some(2));
    assert_eq!(history_len(&engine), 2, "and nothing is un-logged");
}

#[test]
fn a_failed_song_is_skipped_and_not_counted_as_played() {
    use jukebox_core::engine::MAX_FAILURES_IN_A_ROW;
    use jukebox_core::player::PlayerEvent;

    let (_dir, engine) = engine_with_tracks(10, |c| c.per_user_queue_limit = 0);
    for track in 1..=10 {
        engine.enqueue(track, "192.0.2.21", None).unwrap();
    }
    let fail = |engine: &Engine| {
        let load = engine.player().current_load().unwrap();
        let failed = PlayerEvent::Failed {
            load,
            reason: "the file can't be opened".into(),
        };
        engine.handle_player_event(failed).unwrap();
    };

    // One failure, then a song that starts: the count starts over.
    fail(&engine);
    assert_eq!(playing_track(&engine), Some(2));
    let load = engine.player().current_load().unwrap();
    engine
        .handle_player_event(PlayerEvent::Started { load })
        .unwrap();
    engine
        .handle_player_event(PlayerEvent::Ended { load })
        .unwrap();
    assert_eq!(
        play_history(&engine),
        [3, 2],
        "the failed song left no play"
    );

    for _ in 0..MAX_FAILURES_IN_A_ROW - 1 {
        fail(&engine);
        assert!(engine
            .queue_state("x")
            .unwrap()
            .now_playing
            .problem
            .is_none());
    }
    fail(&engine);
    let state = engine.queue_state("x").unwrap();
    assert!(state.now_playing.problem.is_some());
    assert!(state.now_playing.entry.is_none());
    assert!(!engine.player().state().playing, "paused");
    assert_eq!(state.queue.len(), 3, "songs 8 to 10 still wait");
    assert_eq!(play_history(&engine), [2]);

    // A song loaded by hand isn't the queue's to skip.
    engine.player().load(9, true);
    fail(&engine);
    assert!(engine
        .queue_state("x")
        .unwrap()
        .now_playing
        .problem
        .is_some());
    assert_eq!(engine.queue_state("x").unwrap().queue.len(), 3);
}
