//! The audio player on virtual devices, played faster than real time: what it reports, what
//! reaches the device, and how it copes with devices coming and going.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jukebox_audio::{AudioPlayer, VirtualBackend};
use jukebox_core::player::{Player, PlayerEvent};

/// Virtual devices play this many times faster than real time.
const SPEED: f64 = 8.0;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Track ids are indexes into this list; anything past its end isn't in the library.
const TRACKS: &[&str] = &[
    "tone.flac",     // 0
    "tone.mp3",      // 1
    "tone.opus",     // 2
    "not-audio.mp3", // 3
    "he-aac.m4a",    // 4
    "tone-48k.flac", // 5
    "missing.flac",  // 6: in the library, gone from disk
];

fn player(backend: &VirtualBackend) -> (AudioPlayer, Receiver<PlayerEvent>) {
    let resolver = Arc::new(|id: i64| TRACKS.get(id as usize).map(|name| fixture(name)));
    let player = AudioPlayer::new(backend.clone(), resolver, None);
    let (tx, rx) = channel();
    player.on_event(Arc::new(move |event| {
        let _ = tx.send(event);
    }));
    (player, rx)
}

fn next_event(events: &Receiver<PlayerEvent>) -> PlayerEvent {
    events
        .recv_timeout(Duration::from_secs(10))
        .expect("an event within 10 s")
}

/// Seconds of the fixtures' tone (a 0.5-amplitude sine) in a recording, measured by energy:
/// a sine's mean square is half its amplitude squared.
fn tone_seconds(samples: &[f32], rate: u32, channels: usize) -> f64 {
    let energy: f64 = samples.iter().map(|s| f64::from(*s).powi(2)).sum();
    energy / 0.125 / f64::from(rate) / channels as f64
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_song_starts_plays_and_ends() {
    let backend = VirtualBackend::recording(SPEED);
    backend.add_device("speakers", "Speakers", 44_100, 2);
    let (player, events) = player(&backend);

    let load = player.load(0, true);
    assert_eq!(player.current_load(), Some(load));
    assert_eq!(next_event(&events), PlayerEvent::Started { load });
    wait_until("the duration", || player.state().duration > 1.9);
    assert_eq!(next_event(&events), PlayerEvent::Ended { load });
    assert!(!player.state().playing);

    // What reached the device: the whole file, never early. The device plays silence
    // while the first audio is buffered, so the tone can arrive later than 0.25 s of device
    // time — by how long that took on this machine — but not sooner.
    let recorded = backend.recorded("speakers");
    let onset = recorded
        .chunks(2)
        .position(|f| f[0].abs() > 0.01)
        .expect("the tone reached the device");
    let at = onset as f64 / 44_100.0;
    assert!((0.25..1.0).contains(&at), "tone at {at} s of device time");
    let tone = tone_seconds(&recorded, 44_100, 2);
    assert!((1.7..1.8).contains(&tone), "{tone} s of the 1.75 s tone");
}

#[test]
fn audio_is_resampled_and_mapped_to_the_device() {
    let backend = VirtualBackend::recording(SPEED);
    backend.add_device("mono-usb", "USB Speaker", 48_000, 1);
    let (player, events) = player(&backend);
    for track in [0, 2, 5] {
        let load = player.load(track, true);
        assert_eq!(next_event(&events), PlayerEvent::Started { load });
        assert_eq!(next_event(&events), PlayerEvent::Ended { load });
    }
    // Three songs of 1.75 s of tone each, arriving at the device's rate on its one channel.
    let tone = tone_seconds(&backend.recorded("mono-usb"), 48_000, 1);
    assert!((5.1..5.4).contains(&tone), "{tone} s of tone");
}

#[test]
fn unplayable_songs_fail_with_a_reason() {
    let backend = VirtualBackend::new(SPEED);
    backend.add_device("speakers", "Speakers", 44_100, 2);
    let (player, events) = player(&backend);
    for (track, reason) in [
        (3, "not a playable audio file"),
        (4, "HE-AAC isn't supported"),
        (6, "the file can't be opened"),
        (99, "the song is no longer in the library"),
    ] {
        let load = player.load(track, true);
        match next_event(&events) {
            PlayerEvent::Failed {
                load: failed,
                reason: why,
            } => {
                assert_eq!(failed, load);
                assert!(why.starts_with(reason), "track {track}: {why}");
            }
            other => panic!("track {track}: {other:?}"),
        }
    }
}

#[test]
fn replacing_a_song_silences_its_events() {
    let backend = VirtualBackend::new(SPEED);
    backend.add_device("speakers", "Speakers", 44_100, 2);
    let (player, events) = player(&backend);
    player.load(0, true);
    let second = player.load(1, true);
    assert_eq!(next_event(&events), PlayerEvent::Started { load: second });
    assert_eq!(next_event(&events), PlayerEvent::Ended { load: second });
}

#[test]
fn pausing_holds_the_position_and_seeking_moves_it() {
    let backend = VirtualBackend::new(SPEED);
    backend.add_device("speakers", "Speakers", 44_100, 2);
    let (player, events) = player(&backend);
    let load = player.load(0, true);
    assert_eq!(next_event(&events), PlayerEvent::Started { load });

    player.pause();
    std::thread::sleep(Duration::from_millis(100));
    let held = player.state().position;
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(player.state().position, held, "paused");

    player.seek(1.5);
    player.play();
    wait_until("the position after the seek", || {
        player.state().position >= 1.5
    });
    assert_eq!(next_event(&events), PlayerEvent::Ended { load });
}

#[test]
fn an_unplugged_device_hands_over_to_the_default() {
    let backend = VirtualBackend::recording(1.0);
    backend.add_device("usb", "USB DAC", 44_100, 2);
    backend.add_device("builtin", "Built-in Output", 48_000, 2);
    let resolver = Arc::new(|id: i64| TRACKS.get(id as usize).map(|name| fixture(name)));
    let player = AudioPlayer::new(backend.clone(), resolver, Some("usb".into()));
    let (tx, events) = channel();
    player.on_event(Arc::new(move |event| {
        let _ = tx.send(event);
    }));

    let load = player.load(0, true);
    assert_eq!(next_event(&events), PlayerEvent::Started { load });
    wait_until("half a second of playback", || {
        player.state().position > 0.5
    });
    backend.set_connected("usb", false);
    let ended = next_event(&events);
    assert_eq!(
        ended,
        PlayerEvent::Ended { load },
        "the song finishes on the other device"
    );
    // It carried on from where it was rather than starting again: between them the two
    // devices played the song's 1.75 s of tone once, give or take what was buffered when
    // the first one vanished.
    let first = tone_seconds(&backend.recorded("usb"), 44_100, 2);
    let second = tone_seconds(&backend.recorded("builtin"), 48_000, 2);
    assert!(second > 0.3, "{second} s of tone after the handover");
    assert!(
        (1.4..2.0).contains(&(first + second)),
        "{first} + {second} s of tone"
    );
}

#[test]
fn the_output_can_be_chosen_by_name_and_an_unknown_one_plays_on_the_default() {
    for (setting, plays_on) in [
        ("HDMI", "hdmi"),
        ("hdmi", "hdmi"),
        ("Living room", "speakers"),
    ] {
        let backend = VirtualBackend::recording(SPEED);
        backend.add_device("speakers", "Speakers", 44_100, 2);
        backend.add_device("hdmi", "HDMI", 48_000, 2);
        let resolver = Arc::new(|id: i64| TRACKS.get(id as usize).map(|name| fixture(name)));
        let player = AudioPlayer::new(backend.clone(), resolver, Some(setting.into()));
        let (tx, events) = channel();
        player.on_event(Arc::new(move |event| {
            let _ = tx.send(event);
        }));
        let load = player.load(0, true);
        assert_eq!(next_event(&events), PlayerEvent::Started { load });
        assert_eq!(next_event(&events), PlayerEvent::Ended { load });
        let rate = if plays_on == "hdmi" { 48_000 } else { 44_100 };
        let tone = tone_seconds(&backend.recorded(plays_on), rate, 2);
        assert!(tone > 1.5, "{setting:?} played {tone} s on {plays_on}");
    }
}

#[test]
fn without_a_device_it_waits_for_one() {
    let backend = VirtualBackend::new(SPEED);
    let (player, events) = player(&backend);
    let load = player.load(0, true);
    assert!(
        events.recv_timeout(Duration::from_millis(500)).is_err(),
        "nothing to report yet"
    );
    backend.add_device("late", "Plugged in later", 44_100, 2);
    assert_eq!(next_event(&events), PlayerEvent::Started { load });
    assert_eq!(next_event(&events), PlayerEvent::Ended { load });
}

#[test]
fn devices_are_listed_after_the_default() {
    let backend = VirtualBackend::new(SPEED);
    backend.add_device("speakers", "Speakers", 44_100, 2);
    backend.add_device("hdmi", "HDMI", 48_000, 2);
    let (player, _events) = player(&backend);
    wait_until("the device list", || !player.devices().is_empty());
    let labels: Vec<(String, String)> = player
        .devices()
        .into_iter()
        .map(|d| (d.device_id, d.label))
        .collect();
    assert_eq!(
        labels,
        [
            ("default".into(), "Default - Speakers".into()),
            ("speakers".into(), "Speakers".into()),
            ("hdmi".into(), "HDMI".into()),
        ]
    );
}

#[test]
fn volume_zero_is_silent() {
    let backend = VirtualBackend::recording(SPEED);
    backend.add_device("speakers", "Speakers", 44_100, 2);
    let (player, events) = player(&backend);
    player.set_volume(0.0);
    let load = player.load(0, true);
    assert_eq!(next_event(&events), PlayerEvent::Started { load });
    assert_eq!(next_event(&events), PlayerEvent::Ended { load });
    let loudest = backend
        .recorded("speakers")
        .iter()
        .fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(loudest < 0.01, "{loudest}");
}
