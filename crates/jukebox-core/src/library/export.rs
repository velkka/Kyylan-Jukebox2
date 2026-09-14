//! The admin panel's CSV download of the whole library.

use rusqlite::types::ValueRef;
use rusqlite::Connection;

use super::js;

pub const CSV_COLUMNS: &[&str] = &[
    "id",
    "title",
    "artist",
    "album",
    "albumArtist",
    "genre",
    "year",
    "trackNo",
    "discNo",
    "duration",
    "path",
    "addedAt",
];

/// The export file's contents, as `/library/export.csv` sent them: a byte-order mark so
/// Excel reads accents as UTF-8, a header row, then one RFC 4180 row per track, every line
/// ending in CRLF.
pub fn library_csv(conn: &Connection) -> rusqlite::Result<String> {
    let mut stmt = conn.prepare(
        "SELECT id, title, artist, album, album_artist AS albumArtist, genre, year,
              track_no AS trackNo, disc_no AS discNo, duration, path, added_at AS addedAt
         FROM tracks
        ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, disc_no, track_no, title COLLATE NOCASE",
    )?;
    let mut out = String::from("\u{feff}");
    out.push_str(&CSV_COLUMNS.join(","));
    out.push_str("\r\n");
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let cells = (0..CSV_COLUMNS.len())
            .map(|i| Ok(cell(row.get_ref(i)?)))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    Ok(out)
}

/// `csvCell(String(value))`: empty for NULL, quoted when it holds a quote, comma or line
/// break, with quotes doubled.
fn cell(value: ValueRef<'_>) -> String {
    let text = match value {
        ValueRef::Null => return String::new(),
        ValueRef::Integer(n) => n.to_string(),
        ValueRef::Real(n) => js::number_to_string(n),
        ValueRef::Text(t) | ValueRef::Blob(t) => String::from_utf8_lossy(t).into_owned(),
    };
    if text.contains(['"', ',', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}
