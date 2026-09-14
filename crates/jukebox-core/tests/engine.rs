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
