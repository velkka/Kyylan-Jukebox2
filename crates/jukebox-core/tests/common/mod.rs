//! Helpers shared by the integration tests.

use rusqlite::Connection;

/// Schema of a database Electron built up release by release, rows excluded.
pub const ELECTRON_SCHEMA: &str = include_str!("../fixtures/electron-schema.tsv");

/// `sqlite_master` in the format the reference was dumped in: one object per line,
/// tab-separated, with tabs and newlines inside the SQL escaped as `\t` and `\n`.
pub fn schema_tsv(conn: &Connection) -> String {
    let mut stmt = conn
        .prepare(
            "SELECT type, name, tbl_name, coalesce(sql, '') FROM sqlite_master ORDER BY type, name",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            let sql: String = r.get(3)?;
            Ok(format!(
                "{}\t{}\t{}\t{}\n",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                sql.replace('\t', "\\t").replace('\n', "\\n")
            ))
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}
