//! Finding the audio files under a library folder, in the order Electron's `walk` found them.

use std::fs;
use std::vec::IntoIter;

use crate::js;

/// The extensions the scanner indexes. Electron also indexed `.wma`; the Rust build can't
/// play it, so those tracks drop out on the first rescan.
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "m4a", "aac", "flac", "ogg", "oga", "opus", "wav"];

/// Whether a file name has one of [`AUDIO_EXTENSIONS`], compared case-insensitively. Like
/// Node's `extname`, a leading dot doesn't start an extension: `.mp3` has none.
pub fn is_audio_file(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// Depth-first over a folder, yielding audio file paths as Node's `path.join` spells them.
///
/// Each directory's entries are visited in byte order of their names — the order libuv's
/// `scandir`, and so Electron, returned them in — with a subdirectory walked at its place
/// in that order. Symbolic links are neither followed nor indexed, and a directory that
/// can't be read is skipped, all as before.
pub struct Walker {
    stack: Vec<IntoIter<Entry>>,
}

struct Entry {
    path: String,
    is_dir: bool,
}

impl Walker {
    pub fn new(root: &str) -> Self {
        let mut walker = Walker { stack: Vec::new() };
        walker.push_dir(root);
        walker
    }

    fn push_dir(&mut self, dir: &str) {
        let Ok(read) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<(String, Entry)> = Vec::new();
        for entry in read.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                tracing::warn!(dir, name = ?name, "skipping a file name that isn't valid Unicode");
                continue;
            };
            let is_dir = file_type.is_dir();
            if is_dir || (file_type.is_file() && is_audio_file(name)) {
                let path = js::path_join(dir, name);
                entries.push((name.to_string(), Entry { path, is_dir }));
            }
        }
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        self.stack.push(
            entries
                .into_iter()
                .map(|(_, e)| e)
                .collect::<Vec<_>>()
                .into_iter(),
        );
    }
}

impl Iterator for Walker {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        loop {
            let entry = match self.stack.last_mut()?.next() {
                Some(entry) => entry,
                None => {
                    self.stack.pop();
                    continue;
                }
            };
            if entry.is_dir {
                self.push_dir(&entry.path);
            } else {
                return Some(entry.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_extensions_follow_node_extname() {
        assert!(is_audio_file("Song.MP3"));
        assert!(is_audio_file(".hidden.flac"));
        assert!(is_audio_file("a..opus"));
        assert!(!is_audio_file(".mp3"));
        assert!(!is_audio_file("old.wma"));
        assert!(!is_audio_file("cover.jpg"));
        assert!(!is_audio_file("mp3"));
    }
}
