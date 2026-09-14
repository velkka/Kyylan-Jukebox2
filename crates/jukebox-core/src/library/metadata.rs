//! Reading a track's tags, cover art and duration — the values the Electron scanner got
//! from music-metadata, read here with lofty.
//!
//! A file can carry several tags at once: an MP3 often has ID3v2 at the front and ID3v1 or
//! APEv2 at the back. music-metadata merges them field by field, taking each field from the
//! highest-priority tag that has it, and this follows the same order and the same per-field
//! rules. tests/library_electron.rs holds the result to what Electron stored for a set of
//! fixtures, including the one place this deliberately disagrees.

use std::path::Path;

use lofty::config::{ParseOptions, ParsingMode};
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::picture::Picture;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagType};
use sha1::{Digest, Sha1};

use crate::js;

/// Everything the scanner stores about one file, before it has a row.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackMeta {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    /// Every genre, joined with `", "`.
    pub genre: Option<String>,
    /// Seconds.
    pub duration: Option<f64>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub year: Option<i64>,
    pub art: Option<Art>,
}

/// An embedded cover image, keyed by the SHA-1 of its bytes so an album's tracks share one
/// stored copy — the same hex key Electron used, so existing `art` rows stay valid.
#[derive(Clone, PartialEq)]
pub struct Art {
    pub hash: String,
    pub mime: String,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Art {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Art")
            .field("hash", &self.hash)
            .field("mime", &self.mime)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// Reads a file's metadata. Never fails: a file that can't be parsed at all still becomes a
/// track titled after its file name, as it did in Electron, so the library shows it.
pub fn read(path: &Path) -> TrackMeta {
    let file = parse(path, true).or_else(|| parse(path, false));
    let mut meta = match &file {
        Some(file) => from_tags(file),
        None => TrackMeta::untagged(),
    };
    meta.duration = file
        .as_ref()
        .map(|f| f.properties().duration())
        .filter(|d| !d.is_zero())
        .map(|d| d.as_secs_f64());
    if js::trim(&meta.title).is_empty() {
        meta.title = file_stem(path);
    } else {
        meta.title = js::trim(&meta.title).to_string();
    }
    meta
}

/// Detects the format from the file's contents rather than trusting the extension — lofty
/// doesn't know `.oga` — and parses leniently, since a tag with one malformed frame should
/// still yield its other fields. The second attempt skips the audio properties, for files
/// whose tags are fine but whose audio lofty can't make sense of.
fn parse(path: &Path, properties: bool) -> Option<TaggedFile> {
    let options = ParseOptions::new()
        .parsing_mode(ParsingMode::Relaxed)
        .read_properties(properties);
    let probe = Probe::open(path)
        .ok()?
        .options(options)
        .guess_file_type()
        .ok()?;
    probe.read().ok()
}

/// `basename(path, extname(path))`.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl TrackMeta {
    fn untagged() -> Self {
        TrackMeta {
            title: String::new(),
            artist: None,
            album: None,
            album_artist: None,
            genre: None,
            duration: None,
            track_no: None,
            disc_no: None,
            year: None,
            art: None,
        }
    }
}

/// music-metadata's tag priority, best first. Tag types it doesn't read are left out.
fn priority(tag_type: TagType) -> Option<u8> {
    Some(match tag_type {
        TagType::Ape => 2,
        TagType::VorbisComments => 3,
        TagType::Id3v2 => 4,
        TagType::RiffInfo => 7,
        TagType::Mp4Ilst => 9,
        TagType::AiffText => 10,
        TagType::Id3v1 => 11,
        _ => return None,
    })
}

fn from_tags(file: &TaggedFile) -> TrackMeta {
    let mut tags: Vec<&Tag> = file
        .tags()
        .iter()
        .filter(|t| priority(t.tag_type()).is_some())
        .collect();
    tags.sort_by_key(|t| priority(t.tag_type()));

    // Every value a tag holds for a key, as music-metadata saw it: ID3 values come trimmed,
    // other formats' don't.
    let values = |tag: &Tag, key: ItemKey| -> Vec<String> {
        let trimmed = matches!(tag.tag_type(), TagType::Id3v2 | TagType::Id3v1);
        tag.get_strings(key)
            .map(|v| if trimmed { js::trim(v) } else { v }.to_string())
            .collect()
    };
    // Values from the best tag that has any.
    let best = |key: ItemKey| -> Vec<String> {
        tags.iter()
            .map(|t| values(t, key))
            .find(|v| !v.is_empty())
            .unwrap_or_default()
    };
    // A single-valued field: when one tag repeats it, the last value wins.
    let last = |key: ItemKey| best(key).pop();

    let artist = best(ItemKey::TrackArtist).into_iter().next().or_else(|| {
        // No artist, but a list of artists: music-metadata joins them "A, B & C".
        let mut artists = best(ItemKey::TrackArtists);
        let mut seen = std::collections::HashSet::new();
        artists.retain(|a| seen.insert(a.clone()));
        join_artists(&artists)
    });

    let mut genres = best(ItemKey::Genre);
    let mut seen = std::collections::HashSet::new();
    genres.retain(|g| seen.insert(g.clone()));

    TrackMeta {
        title: last(ItemKey::TrackTitle).unwrap_or_default(),
        artist,
        album: last(ItemKey::AlbumTitle),
        album_artist: last(ItemKey::AlbumArtist),
        genre: (!genres.is_empty()).then(|| genres.join(", ")),
        duration: None,
        // `parseInt(no) || null`: a track or disc number of 0 counts as none.
        track_no: last(ItemKey::TrackNumber)
            .and_then(|v| js::parse_int(&v))
            .filter(|&n| n != 0),
        disc_no: last(ItemKey::DiscNumber)
            .and_then(|v| js::parse_int(&v))
            .filter(|&n| n != 0),
        year: year(&tags),
        art: tags.iter().find_map(|t| t.pictures().iter().find_map(art)),
    }
}

/// music-metadata takes the year from a date's first four characters, or from a bare year
/// field. Vorbis comments have no year field for it — only `DATE` — so a `YEAR` comment,
/// which lofty does read, is ignored here too.
fn year(tags: &[&Tag]) -> Option<i64> {
    tags.iter().find_map(|tag| {
        let from_date = tag
            .get_string(ItemKey::RecordingDate)
            .and_then(|d| js::parse_int(&d.chars().take(4).collect::<String>()));
        let from_year = (tag.tag_type() != TagType::VorbisComments)
            .then(|| tag.get_string(ItemKey::Year))
            .flatten()
            .and_then(js::parse_int);
        from_date.or(from_year)
    })
}

fn join_artists(artists: &[String]) -> Option<String> {
    match artists {
        [] => None,
        [one] => Some(one.clone()),
        [init @ .., last] if init.len() > 1 => Some(format!("{} & {last}", init.join(", "))),
        _ => Some(artists.join(" & ")),
    }
}

/// A picture as music-metadata kept it: an empty one is skipped, the type is lower-cased
/// with `image/jpg` corrected, and one with no type is identified from its bytes or dropped.
fn art(picture: &Picture) -> Option<Art> {
    let data = picture.data();
    if data.is_empty() {
        return None;
    }
    let mime = match picture.mime_type() {
        Some(m) if !m.as_str().is_empty() => m.as_str().to_lowercase(),
        _ => sniff_image(data)?.to_string(),
    };
    let mime = if mime == "image/jpg" {
        "image/jpeg".into()
    } else {
        mime
    };
    let hash = Sha1::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Some(Art {
        hash,
        mime,
        data: data.to_vec(),
    })
}

fn sniff_image(data: &[u8]) -> Option<&'static str> {
    Some(match data {
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        [b'I', b'I', 0x2a, 0x00, ..] | [b'M', b'M', 0x00, 0x2a, ..] => "image/tiff",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artists_join_like_music_metadata() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(join_artists(&s(&[])), None);
        assert_eq!(join_artists(&s(&["A"])).as_deref(), Some("A"));
        assert_eq!(join_artists(&s(&["A", "B"])).as_deref(), Some("A & B"));
        assert_eq!(
            join_artists(&s(&["A", "B", "C"])).as_deref(),
            Some("A, B & C")
        );
    }
}
