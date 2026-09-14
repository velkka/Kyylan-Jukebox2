//! Decoding each format: audio starts exactly where it should — so encoder delay is trimmed
//! — seeks land on the frame asked for, and files that can't be played are refused with a
//! reason.
//!
//! Every fixture is the same two seconds: silence, then a tone from 0.25 s.

use std::path::{Path, PathBuf};

use jukebox_audio::source::Source;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The first frame of the tone after decoding from wherever the source stands, in seconds.
fn onset(source: &mut Source) -> f64 {
    let mut frames = 0usize;
    while let Some(chunk) = source.next_chunk().unwrap() {
        for (i, frame) in chunk.samples.chunks(chunk.channels).enumerate() {
            if frame[0].abs() > 0.05 {
                return (frames + i) as f64 / f64::from(chunk.rate);
            }
        }
        frames += chunk.samples.len() / chunk.channels;
    }
    panic!("no tone found");
}

/// (file, how far the tone may start from 0.25 s). Lossless and trimmed-lossy formats are
/// exact to the frame; LAME's own gapless information is 47 frames short.
const PLAYABLE: &[(&str, f64)] = &[
    ("reference.wav", 0.0),
    ("tone.flac", 0.0),
    ("tone-48k.flac", 0.0),
    ("tone.mp3", 0.0012),
    ("tone.m4a", 0.0),
    ("tone-apple.m4a", 0.0),
    ("tone.opus", 0.0),
    ("tone.ogg", 0.0),
];

const FRAME: f64 = 1.0 / 44_100.0;

#[test]
fn audio_starts_where_the_file_starts() {
    for (name, tolerance) in PLAYABLE {
        let mut source = Source::open(&fixture(name)).unwrap();
        let at = onset(&mut source);
        assert!(
            (at - 0.25).abs() <= tolerance + 2.0 * FRAME,
            "{name}: the tone starts at {at:.5} s — untrimmed encoder delay shows up as late"
        );
    }
}

#[test]
fn seeks_land_on_the_frame_asked_for() {
    for (name, tolerance) in PLAYABLE {
        let mut source = Source::open(&fixture(name)).unwrap();
        let landed = source.seek(0.2).unwrap();
        assert!((landed - 0.2).abs() < 0.001, "{name}: landed at {landed}");
        let at = onset(&mut source);
        assert!(
            (at - 0.05).abs() <= tolerance + 2.0 * FRAME,
            "{name}: after seeking to 0.2 s the tone starts {at:.5} s in, not 0.05 s"
        );
    }
}

#[test]
fn durations_come_from_the_file() {
    for (name, _) in PLAYABLE {
        let source = Source::open(&fixture(name)).unwrap();
        let duration = source.duration().unwrap();
        assert!((duration - 2.0).abs() < 0.03, "{name}: {duration}");
    }
}

/// Decodes a whole file, returning the first error.
fn decode_error(name: &str) -> String {
    let mut source = match Source::open(&fixture(name)) {
        Ok(source) => source,
        Err(e) => return e,
    };
    loop {
        match source.next_chunk() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("{name} decoded without an error"),
            Err(e) => return e,
        }
    }
}

#[test]
fn unplayable_files_are_refused_with_a_reason() {
    assert!(decode_error("he-aac.m4a").starts_with("HE-AAC isn't supported"));
    assert!(decode_error("surround.m4a").starts_with("the audio format can't be decoded"));
    assert!(decode_error("not-audio.mp3").starts_with("not a playable audio file"));
    assert!(decode_error("missing.flac").starts_with("the file can't be opened"));
}
