//! The encoder delay an MP4 audio file declares, which symphonia leaves in the decoded audio.
//!
//! AAC encoders start every file with 1,024–2,112 frames of priming — 23 to 48 ms of
//! lead-in — and record how much in the file: an edit list whose first edit starts
//! `media_time` units into the track, or, from iTunes and Apple's encoder, an `iTunSMPB`
//! tag. Without trimming it, every song starts late and every seek lands that far off.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Frames of priming, at `sample_rate`, to drop from the start of the file's first audio
/// track — if the file says.
pub fn priming_frames(path: &Path, sample_rate: u32) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let moov = find(&mut file, 0, len, b"moov")?;
    let from_edit_list = audio_track_edit(&mut file, moov)
        .map(|(time, scale)| time * u64::from(sample_rate) / u64::from(scale));
    from_edit_list.or_else(|| itunes_smpb(&mut file, moov))
}

/// A box's payload: where it starts, and where it ends.
type Span = (u64, u64);

/// The first child box of `kind` between `start` and `end`.
fn find(file: &mut File, start: u64, end: u64, kind: &[u8; 4]) -> Option<Span> {
    children(file, start, end)
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, span)| span)
}

fn children(file: &mut File, start: u64, end: u64) -> Vec<([u8; 4], Span)> {
    let mut out = Vec::new();
    let mut at = start;
    while at + 8 <= end {
        let mut header = [0u8; 8];
        if file.seek(SeekFrom::Start(at)).is_err() || file.read_exact(&mut header).is_err() {
            break;
        }
        let mut size = u64::from(u32::from_be_bytes(header[..4].try_into().unwrap()));
        let kind: [u8; 4] = header[4..].try_into().unwrap();
        let mut payload = at + 8;
        if size == 1 {
            let mut large = [0u8; 8];
            if file.read_exact(&mut large).is_err() {
                break;
            }
            size = u64::from_be_bytes(large);
            payload += 8;
        } else if size == 0 {
            size = end - at;
        }
        if size < payload - at || at + size > end {
            break;
        }
        out.push((kind, (payload, at + size)));
        at += size;
    }
    out
}

fn read_at(file: &mut File, at: u64, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len];
    file.seek(SeekFrom::Start(at)).ok()?;
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// `media_time` of the first real edit of the first sound track, with the track's time scale.
fn audio_track_edit(file: &mut File, (start, end): Span) -> Option<(u64, u32)> {
    for (kind, (tstart, tend)) in children(file, start, end) {
        if &kind != b"trak" {
            continue;
        }
        let (mstart, mend) = find(file, tstart, tend, b"mdia")?;
        let (hstart, _) = find(file, mstart, mend, b"hdlr")?;
        // version/flags (4), pre_defined (4), handler_type (4)
        if read_at(file, hstart + 8, 4)?.as_slice() != b"soun" {
            continue;
        }
        let (dstart, _) = find(file, mstart, mend, b"mdhd")?;
        let version = read_at(file, dstart, 1)?[0];
        let scale_at = if version == 1 {
            dstart + 4 + 16
        } else {
            dstart + 4 + 8
        };
        let timescale = u32::from_be_bytes(read_at(file, scale_at, 4)?.try_into().ok()?);

        let (estart, eend) = find(file, tstart, tend, b"edts")?;
        let (lstart, _) = find(file, estart, eend, b"elst")?;
        let head = read_at(file, lstart, 8)?;
        let version = head[0];
        let count = u32::from_be_bytes(head[4..8].try_into().ok()?);
        let entry = if version == 1 { 20 } else { 12 };
        for i in 0..count.min(4) {
            let raw = read_at(file, lstart + 8 + u64::from(i) * entry, entry as usize)?;
            let media_time = if version == 1 {
                i64::from_be_bytes(raw[8..16].try_into().ok()?)
            } else {
                i64::from(i32::from_be_bytes(raw[4..8].try_into().ok()?))
            };
            // -1 is an empty edit (a gap before the media); skip to the next.
            if media_time >= 0 {
                return (media_time > 0 && timescale > 0).then_some((media_time as u64, timescale));
            }
        }
        return None;
    }
    None
}

/// The encoder delay in an `iTunSMPB` tag: hex fields, the second of which is the delay.
fn itunes_smpb(file: &mut File, moov: Span) -> Option<u64> {
    let (ustart, uend) = find(file, moov.0, moov.1, b"udta")?;
    let (mstart, mend) = find(file, ustart, uend, b"meta")?;
    // `meta` is a full box: skip version and flags.
    let (istart, iend) = find(file, mstart + 4, mend, b"ilst")?;
    for (kind, (start, end)) in children(file, istart, iend) {
        if &kind != b"----" {
            continue;
        }
        let parts = children(file, start, end);
        let name = parts.iter().find(|(k, _)| k == b"name")?;
        let name = read_at(file, name.1 .0 + 4, (name.1 .1 - name.1 .0 - 4) as usize)?;
        if name != b"iTunSMPB" {
            continue;
        }
        let data = parts.iter().find(|(k, _)| k == b"data")?;
        let text = read_at(file, data.1 .0 + 8, (data.1 .1 - data.1 .0 - 8) as usize)?;
        let text = String::from_utf8_lossy(&text);
        let delay = text.split_whitespace().nth(1)?;
        return u64::from_str_radix(delay, 16).ok().filter(|&d| d > 0);
    }
    None
}
