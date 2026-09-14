//! Phase 9's survey of a real library: which files the Rust player can't play, and why. It
//! decides whether HE-AAC needs a decoder of its own, and finds what would be skipped at a
//! party before a party does.
//!
//!   cargo run --release -p jukebox-audio --example playability_survey [-- <data-dir>] [--folder <path>]... [--report <file.tsv>]
//!
//! The folders are the data directory's library folders — this machine's when none is given —
//! or the ones named with --folder. Each file is opened, its first seconds decoded, and a seek
//! to the middle decoded too, as the player would. Files are only read. The report lists every
//! file that failed or decoded with errors, one per line.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use jukebox_audio::source::Source;
use jukebox_core::config::AppConfig;
use jukebox_core::library::Walker;
use jukebox_core::paths::DataDir;

/// Seconds decoded from the start, and after the seek.
const DECODE_SECS: f64 = 3.0;

#[derive(Debug)]
enum Outcome {
    Playable,
    /// Plays, but some packets failed to decode and were skipped.
    Damaged(u64),
    Unplayable(String),
}

fn decode(source: &mut Source, secs: f64) -> Result<f64, String> {
    let mut decoded = 0.0;
    while decoded < secs {
        match source.next_chunk()? {
            Some(chunk) => {
                decoded += chunk.samples.len() as f64
                    / chunk.channels.max(1) as f64
                    / f64::from(chunk.rate.max(1));
            }
            None => break,
        }
    }
    Ok(decoded)
}

fn survey(path: &Path) -> Outcome {
    let result = (|| {
        let mut source = Source::open(path)?;
        let from_start = decode(&mut source, DECODE_SECS)?;
        if let Some(duration) = source.duration().filter(|d| *d > DECODE_SECS * 3.0) {
            source.seek(duration / 2.0)?;
            decode(&mut source, DECODE_SECS)?;
        }
        if from_start == 0.0 {
            return Err("no audio decoded".to_string());
        }
        Ok(source.decode_errors)
    })();
    match result {
        Ok(0) => Outcome::Playable,
        Ok(errors) => Outcome::Damaged(errors),
        Err(reason) => Outcome::Unplayable(reason),
    }
}

/// A reason without the details that differ from file to file, so like failures count together.
fn kind(reason: &str) -> String {
    match reason.find(" (") {
        Some(at) => reason[..at].to_string(),
        None => reason.to_string(),
    }
}

fn main() -> ExitCode {
    let mut data_dir = None;
    let mut folders = Vec::new();
    let mut report = None;
    let mut args = std::env::args().skip(1);
    let usage = || {
        eprintln!(
            "usage: playability_survey [<data-dir>] [--folder <path>]... [--report <file.tsv>]"
        );
        ExitCode::from(2)
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--folder" => match args.next() {
                Some(folder) => folders.push(folder),
                None => return usage(),
            },
            "--report" => match args.next() {
                Some(file) => report = Some(PathBuf::from(file)),
                None => return usage(),
            },
            dir if !dir.starts_with("--") && data_dir.is_none() => {
                data_dir = Some(DataDir::at(dir))
            }
            _ => return usage(),
        }
    }
    if folders.is_empty() {
        let Some(dir) = data_dir.or_else(DataDir::resolve) else {
            eprintln!("no data directory; pass one, or --folder");
            return ExitCode::FAILURE;
        };
        match AppConfig::read(&dir.config_path()) {
            Ok(config) => folders = config.library_paths,
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::FAILURE;
            }
        }
    }

    let started = Instant::now();
    let files: Vec<String> = folders.iter().flat_map(|f| Walker::new(f)).collect();
    println!("{} audio files in {} folders", files.len(), folders.len());

    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(files.len()));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(file) = files.get(i) else { break };
                let outcome = survey(Path::new(file));
                let mut results = results.lock().unwrap();
                results.push((file.clone(), outcome));
                if results.len() % 500 == 0 {
                    eprintln!("  {} / {}", results.len(), files.len());
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));

    // Per extension: files, playable, damaged, unplayable.
    let mut by_extension: BTreeMap<String, [usize; 4]> = BTreeMap::new();
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for (file, outcome) in &results {
        let extension = Path::new(file)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let counts = by_extension.entry(extension).or_default();
        counts[0] += 1;
        match outcome {
            Outcome::Playable => counts[1] += 1,
            Outcome::Damaged(_) => counts[2] += 1,
            Outcome::Unplayable(reason) => {
                counts[3] += 1;
                *reasons.entry(kind(reason)).or_default() += 1;
            }
        }
    }

    println!(
        "\n{:<6} {:>7} {:>9} {:>8} {:>11}",
        "type", "files", "playable", "damaged", "unplayable"
    );
    for (extension, [total, playable, damaged, unplayable]) in &by_extension {
        println!("{extension:<6} {total:>7} {playable:>9} {damaged:>8} {unplayable:>11}");
    }
    if reasons.is_empty() {
        println!("\nEvery file plays.");
    } else {
        println!("\nWhy files can't be played:");
        let mut reasons: Vec<_> = reasons.into_iter().collect();
        reasons.sort_by_key(|r| std::cmp::Reverse(r.1));
        for (reason, count) in reasons {
            println!("{count:>7}  {reason}");
        }
    }
    println!("\n{:.1} s", started.elapsed().as_secs_f64());

    if let Some(report) = report {
        let written = std::fs::File::create(&report).and_then(|mut out| {
            writeln!(out, "file\toutcome\tdetail")?;
            for (file, outcome) in &results {
                match outcome {
                    Outcome::Playable => {}
                    Outcome::Damaged(errors) => {
                        writeln!(out, "{file}\tdamaged\t{errors} packets skipped")?
                    }
                    Outcome::Unplayable(reason) => writeln!(out, "{file}\tunplayable\t{reason}")?,
                }
            }
            Ok(())
        });
        match written {
            Ok(()) => println!("Report: {}", report.display()),
            Err(err) => {
                eprintln!("can't write {}: {err}", report.display());
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
