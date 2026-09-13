//! Typed rows for every table, column for column as the migrations define them.
//!
//! Each reader selects its columns by name, so a database missing a column — or holding a
//! value of the wrong type — fails loudly instead of being half read.

use rusqlite::{Connection, Row};

pub trait TableRow: Sized {
    const TABLE: &'static str;
    const COLUMNS: &'static str;
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self>;
}

/// Reads a whole table in rowid order, handing each row to `each` without collecting them —
/// the `art` table alone can hold hundreds of megabytes of cover images. Returns the count.
pub fn scan<T: TableRow>(conn: &Connection, mut each: impl FnMut(T)) -> rusqlite::Result<u64> {
    let sql = format!("SELECT {} FROM {} ORDER BY rowid", T::COLUMNS, T::TABLE);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut n = 0;
    while let Some(row) = rows.next()? {
        each(T::from_row(row)?);
        n += 1;
    }
    Ok(n)
}

macro_rules! table_row {
    ($(#[$doc:meta])* $name:ident, $table:literal, { $($field:ident : $ty:ty),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name {
            $(pub $field: $ty),+
        }

        impl TableRow for $name {
            const TABLE: &'static str = $table;
            const COLUMNS: &'static str = stringify!($($field),+);
            fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
                Ok($name { $($field: row.get(stringify!($field))?),+ })
            }
        }
    };
}

table_row!(MigrationRow, "_migrations", { id: u32, applied_at: String });

table_row!(MetaRow, "meta", { key: String, value: Option<String> });

table_row!(TrackRow, "tracks", {
    id: i64,
    path: String,
    title: String,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    genre: Option<String>,
    duration: Option<f64>,
    track_no: Option<i64>,
    disc_no: Option<i64>,
    year: Option<i64>,
    art_hash: Option<String>,
    mtime_ms: i64,
    added_at: String,
    seen: i64,
});

table_row!(
    /// Cover art. `data` is the full image; use `scan` rather than collecting these.
    ArtRow, "art", { hash: String, mime: String, data: Vec<u8> });

table_row!(QueueRow, "queue", {
    id: i64,
    track_id: i64,
    added_by_ip: String,
    added_by_name: Option<String>,
    added_at: String,
    position: i64,
    status: String,
});

table_row!(StandbyRow, "standby", { id: i64, track_id: i64, position: i64, added_at: String });

table_row!(PlayHistoryRow, "play_history", {
    id: i64,
    track_id: i64,
    artist: Option<String>,
    played_at: String,
    title: Option<String>,
    requested_by_ip: Option<String>,
    requested_by_name: Option<String>,
    is_standby: i64,
    source: String,
});

table_row!(RequestLogRow, "request_log", {
    id: i64,
    track_id: i64,
    title: Option<String>,
    artist: Option<String>,
    requested_by_ip: String,
    requested_by_name: Option<String>,
    requested_at: String,
});

table_row!(DownvoteLogRow, "downvote_log", {
    id: i64,
    track_id: i64,
    title: Option<String>,
    artist: Option<String>,
    voter_ip: String,
    voter_name: Option<String>,
    voted_at: String,
});

table_row!(BanRow, "bans", { ip: String, name: Option<String>, banned_at: String, expires_at: Option<String> });

/// Row counts for every app table, read through the typed rows above rather than
/// `count(*)`, so a success means every row actually decoded.
pub fn read_every_table(conn: &Connection) -> rusqlite::Result<Vec<(&'static str, u64)>> {
    Ok(vec![
        (MigrationRow::TABLE, scan::<MigrationRow>(conn, drop)?),
        (MetaRow::TABLE, scan::<MetaRow>(conn, drop)?),
        (TrackRow::TABLE, scan::<TrackRow>(conn, drop)?),
        (ArtRow::TABLE, scan::<ArtRow>(conn, drop)?),
        (QueueRow::TABLE, scan::<QueueRow>(conn, drop)?),
        (StandbyRow::TABLE, scan::<StandbyRow>(conn, drop)?),
        (PlayHistoryRow::TABLE, scan::<PlayHistoryRow>(conn, drop)?),
        (RequestLogRow::TABLE, scan::<RequestLogRow>(conn, drop)?),
        (DownvoteLogRow::TABLE, scan::<DownvoteLogRow>(conn, drop)?),
        (BanRow::TABLE, scan::<BanRow>(conn, drop)?),
    ])
}
