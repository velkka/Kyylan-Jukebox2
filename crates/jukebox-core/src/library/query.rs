//! Browsing and searching the library: the queries behind `/api/tracks`, `/api/artists`,
//! `/api/albums` and `/api/art`. The filters, grouping and ordering are library.ts's
//! unchanged, and the logic around them — clamping, trimming, the A–Z index — reproduces
//! what the TypeScript did to its inputs.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use rusqlite::types::ToSql;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

use super::js;
use crate::types::{
    AlbumSummary, AlbumsResponse, ArtistSummary, ArtistsResponse, Track, TracksQuery,
    TracksResponse,
};

/// `/api/artists`' query parameters.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ArtistsQuery {
    pub search: Option<String>,
    pub letter: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// `/api/albums`' query parameters.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AlbumsQuery {
    pub search: Option<String>,
    pub artist: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

const TRACK_COLUMNS: &str =
    "t.id, t.title, t.artist, t.album, t.album_artist, t.genre, t.duration, t.track_no, t.disc_no, t.year, t.art_hash";

fn track(row: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        album: row.get(3)?,
        album_artist: row.get(4)?,
        genre: row.get(5)?,
        duration: row.get(6)?,
        track_no: row.get(7)?,
        disc_no: row.get(8)?,
        year: row.get(9)?,
        art_hash: row.get(10)?,
    })
}

/// A trimmed, non-empty string, or nothing — `query.x?.trim()` followed by `if (x)`.
fn present(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(js::trim).filter(|s| !s.is_empty())
}

fn clamp(value: Option<i64>, max: i64, default: i64) -> i64 {
    value.map_or(default, |v| v.clamp(1, max))
}

/// Named parameters built up alongside a query's SQL.
#[derive(Default)]
struct Params(Vec<(String, Box<dyn ToSql>)>);

impl Params {
    fn set(&mut self, name: &str, value: impl ToSql + 'static) {
        self.0.push((name.to_string(), Box::new(value)));
    }

    fn get(&self) -> Vec<(&str, &dyn ToSql)> {
        self.0
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_ref()))
            .collect()
    }
}

fn count(conn: &Connection, sql: &str, params: &Params) -> rusqlite::Result<i64> {
    conn.prepare(sql)?
        .query_row(params.get().as_slice(), |r| r.get(0))
}

/// A free-text search as an FTS5 prefix match on every word: `daft pu` → `"daft"* "pu"*`.
pub fn fts_query(search: &str) -> String {
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}]+").unwrap());
    WORD.find_iter(search)
        .map(|m| format!("\"{}\"*", m.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn tracks(conn: &Connection, query: &TracksQuery) -> rusqlite::Result<TracksResponse> {
    let limit = clamp(query.limit, 500, 100);
    let offset = query.offset.map_or(0, |o| o.max(0));
    let empty = |total| TracksResponse {
        tracks: Vec::new(),
        total,
        limit,
        offset,
    };

    // Full-text search takes precedence.
    if let Some(search) = present(&query.search) {
        let fts = fts_query(search);
        if fts.is_empty() {
            return Ok(empty(0));
        }
        let total = conn.query_row(
            "SELECT COUNT(*) AS c FROM tracks_fts WHERE tracks_fts MATCH ?",
            [&fts],
            |r| r.get(0),
        )?;
        let tracks = conn
            .prepare(&format!(
                "SELECT {TRACK_COLUMNS} FROM tracks t
         JOIN tracks_fts f ON f.rowid = t.id
         WHERE tracks_fts MATCH ?
         ORDER BY rank
         LIMIT ? OFFSET ?"
            ))?
            .query_map((&fts, limit, offset), track)?
            .collect::<Result<_, _>>()?;
        return Ok(TracksResponse {
            tracks,
            total,
            limit,
            offset,
        });
    }

    // Filtered browse: by album (with album artist) or by artist.
    let mut filter = String::new();
    let mut params = Params::default();
    let mut order =
        "artist COLLATE NOCASE, album COLLATE NOCASE, disc_no, track_no, title COLLATE NOCASE";
    if let Some(album) = present(&query.album) {
        filter.push_str("album = @album COLLATE NOCASE");
        params.set("@album", album.to_string());
        if let Some(album_artist) = present(&query.album_artist) {
            filter.push_str(" AND COALESCE(album_artist, artist) = @aa COLLATE NOCASE");
            params.set("@aa", album_artist.to_string());
        }
        order = "disc_no, track_no, title COLLATE NOCASE";
    } else if let Some(artist) = present(&query.artist) {
        filter
            .push_str("(artist = @artist COLLATE NOCASE OR album_artist = @artist COLLATE NOCASE)");
        params.set("@artist", artist.to_string());
        if query.no_album == Some(true) {
            filter.push_str(" AND (album IS NULL OR album = '')");
            order = "title COLLATE NOCASE";
        } else {
            order = "album COLLATE NOCASE, disc_no, track_no, title COLLATE NOCASE";
        }
    }

    let where_sql = if filter.is_empty() {
        String::new()
    } else {
        format!("WHERE {filter}")
    };
    let total = count(
        conn,
        &format!("SELECT COUNT(*) AS c FROM tracks {where_sql}"),
        &params,
    )?;
    params.set("@limit", limit);
    params.set("@offset", offset);
    let tracks = conn
        .prepare(&format!(
            "SELECT {TRACK_COLUMNS} FROM tracks t {where_sql} ORDER BY {order} LIMIT @limit OFFSET @offset"
        ))?
        .query_map(params.get().as_slice(), track)?
        .collect::<Result<_, _>>()?;
    Ok(TracksResponse {
        tracks,
        total,
        limit,
        offset,
    })
}

/// The A–Z index bucket for an artist's first character: its letter with accents folded
/// away (Ä → A), or `#` for digits, symbols and other scripts. SQLite's `upper()` only
/// knows ASCII, so this happens here rather than in SQL.
pub fn initial_bucket(initial: &str) -> String {
    let folded: String = initial
        .nfd()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .collect::<String>()
        .to_uppercase();
    if folded.len() == 1 && folded.as_bytes()[0].is_ascii_uppercase() {
        folded
    } else {
        "#".into()
    }
}

/// Each bucket with the distinct first characters that fall in it.
fn initial_buckets(conn: &Connection) -> rusqlite::Result<BTreeMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT substr(artist, 1, 1) AS c FROM tracks WHERE artist IS NOT NULL AND artist <> ''",
    )?;
    let mut buckets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for initial in stmt.query_map([], |r| r.get::<_, Option<String>>(0))? {
        let Some(initial) = initial? else { continue };
        if initial.is_empty() {
            continue;
        }
        buckets
            .entry(initial_bucket(&initial))
            .or_default()
            .push(initial);
    }
    Ok(buckets)
}

pub fn artists(conn: &Connection, query: &ArtistsQuery) -> rusqlite::Result<ArtistsResponse> {
    let limit = clamp(query.limit, 1000, 200);
    let offset = query.offset.map_or(0, |o| o.max(0));
    let buckets = initial_buckets(conn)?;
    // A…Z, then # last.
    let mut letters: Vec<String> = buckets.keys().filter(|k| *k != "#").cloned().collect();
    if buckets.contains_key("#") {
        letters.push("#".into());
    }

    let mut filter = "artist IS NOT NULL AND artist <> ''".to_string();
    let mut params = Params::default();
    if let Some(search) = present(&query.search) {
        filter.push_str(" AND artist LIKE @like");
        params.set("@like", format!("%{search}%"));
    }

    let letter = query.letter.as_deref().map(|l| js::trim(l).to_uppercase());
    if let Some(letter) = letter.filter(|l| !l.is_empty()) {
        // Match on the exact initials in this bucket: its case and accent variants.
        let initials = buckets.get(&letter).cloned().unwrap_or_default();
        if initials.is_empty() {
            return Ok(ArtistsResponse {
                artists: Vec::new(),
                total: 0,
                letters,
            });
        }
        let names: Vec<String> = initials
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let name = format!("@c{i}");
                params.set(&name, c);
                name
            })
            .collect();
        filter.push_str(&format!(
            " AND substr(artist, 1, 1) IN ({})",
            names.join(", ")
        ));
    }

    let total = count(
        conn,
        &format!("SELECT COUNT(*) AS c FROM (SELECT 1 FROM tracks WHERE {filter} GROUP BY artist COLLATE NOCASE)"),
        &params,
    )?;
    params.set("@limit", limit);
    params.set("@offset", offset);
    let artists = conn
        .prepare(&format!(
            "SELECT artist, COUNT(*) AS trackCount, COUNT(DISTINCT album) AS albumCount
       FROM tracks WHERE {filter}
       GROUP BY artist COLLATE NOCASE
       ORDER BY artist COLLATE NOCASE
       LIMIT @limit OFFSET @offset"
        ))?
        .query_map(params.get().as_slice(), |r| {
            Ok(ArtistSummary {
                artist: r.get(0)?,
                track_count: r.get(1)?,
                album_count: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(ArtistsResponse {
        artists,
        total,
        letters,
    })
}

pub fn albums(conn: &Connection, query: &AlbumsQuery) -> rusqlite::Result<AlbumsResponse> {
    let limit = clamp(query.limit, 1000, 200);
    let offset = query.offset.map_or(0, |o| o.max(0));

    let mut filter = "album IS NOT NULL AND album <> ''".to_string();
    let mut params = Params::default();
    if let Some(search) = present(&query.search) {
        filter.push_str(" AND (album LIKE @like OR COALESCE(album_artist, artist) LIKE @like)");
        params.set("@like", format!("%{search}%"));
    }
    if let Some(artist) = present(&query.artist) {
        filter.push_str(
            " AND (artist = @artist COLLATE NOCASE OR album_artist = @artist COLLATE NOCASE)",
        );
        params.set("@artist", artist.to_string());
    }

    let total = count(
        conn,
        &format!(
            "SELECT COUNT(*) AS c FROM (
           SELECT 1 FROM tracks WHERE {filter}
           GROUP BY album COLLATE NOCASE, COALESCE(album_artist, artist) COLLATE NOCASE)"
        ),
        &params,
    )?;
    params.set("@limit", limit);
    params.set("@offset", offset);
    let albums = conn
        .prepare(&format!(
            "SELECT album,
              COALESCE(album_artist, artist) AS artist,
              COUNT(*) AS trackCount,
              MAX(art_hash) AS artHash,
              MAX(year) AS year
       FROM tracks WHERE {filter}
       GROUP BY album COLLATE NOCASE, COALESCE(album_artist, artist) COLLATE NOCASE
       ORDER BY year, album COLLATE NOCASE
       LIMIT @limit OFFSET @offset"
        ))?
        .query_map(params.get().as_slice(), |r| {
            Ok(AlbumSummary {
                album: r.get(0)?,
                artist: r.get(1)?,
                track_count: r.get(2)?,
                art_hash: r.get(3)?,
                year: r.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(AlbumsResponse { albums, total })
}

pub fn track_by_id(conn: &Connection, id: i64) -> rusqlite::Result<Option<Track>> {
    conn.query_row(
        &format!("SELECT {TRACK_COLUMNS} FROM tracks t WHERE id = ?"),
        [id],
        track,
    )
    .optional()
}

/// A track's file, for streaming and playback. Never sent to clients.
pub fn track_path(conn: &Connection, id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT path FROM tracks WHERE id = ?", [id], |r| r.get(0))
        .optional()
}

/// A stored cover image: its MIME type and bytes.
pub fn art(conn: &Connection, hash: &str) -> rusqlite::Result<Option<(String, Vec<u8>)>> {
    conn.query_row("SELECT mime, data FROM art WHERE hash = ?", [hash], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .optional()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_query_quotes_each_word_as_a_prefix() {
        assert_eq!(fts_query("daft  pu"), r#""daft"* "pu"*"#);
        assert_eq!(
            fts_query("AC/DC — Back in Black!"),
            r#""AC"* "DC"* "Back"* "in"* "Black"*"#
        );
        assert_eq!(fts_query("Äänitys 2000"), r#""Äänitys"* "2000"*"#);
        assert_eq!(
            fts_query(r#"" OR *"#),
            r#""OR"*"#,
            "FTS syntax can't get through"
        );
        assert_eq!(fts_query("-- !!"), "");
    }

    #[test]
    fn initials_bucket_like_library_ts() {
        for (initial, bucket) in [
            ("a", "A"),
            ("Ä", "A"),
            ("é", "E"),
            ("Ø", "#"), // a letter of its own, not O with an accent
            ("ß", "#"), // upper-cases to "SS"
            ("1", "#"),
            ("日", "#"),
            ("z", "Z"),
        ] {
            assert_eq!(initial_bucket(initial), bucket, "{initial}");
        }
    }
}
