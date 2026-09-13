//! Reads an install's data directory without changing anything: the config, the migration
//! state, and every row of every table through the typed readers. Phase 1's exit check.
//!
//!   cargo run -p jukebox-core --example inspect_data_dir [-- <data-dir>]
//!
//! With no argument it inspects this machine's real data directory — read-only.

use std::process::ExitCode;

use jukebox_core::config::AppConfig;
use jukebox_core::db;
use jukebox_core::paths::DataDir;
use jukebox_core::rows::read_every_table;

fn main() -> ExitCode {
    let dir = match std::env::args().nth(1) {
        Some(path) => DataDir::at(path),
        None => match DataDir::resolve() {
            Some(dir) => dir,
            None => {
                eprintln!("no data directory on this platform");
                return ExitCode::FAILURE;
            }
        },
    };
    println!("data directory  {}", dir.root().display());
    let mut ok = true;

    match AppConfig::read(&dir.config_path()) {
        Ok(c) => {
            println!(
                "config.json     ok — configured {}, port {}, {} library folder(s), admin password {}",
                c.configured,
                c.port,
                c.library_paths.len(),
                if c.admin_password.is_empty() { "unset" } else { "set" }
            );
            if !c.extra.is_empty() {
                println!(
                    "                keys from a newer version kept: {:?}",
                    c.extra.keys().collect::<Vec<_>>()
                );
            }
        }
        Err(e) => {
            println!("config.json     FAILED — {e}");
            ok = false;
        }
    }

    let conn = match db::open_read_only(&dir.database_path()) {
        Ok(conn) => conn,
        Err(e) => {
            println!("jukebox.db      FAILED to open — {e}");
            return ExitCode::FAILURE;
        }
    };
    match db::current_migration(&conn) {
        Ok(found) => {
            let known = db::latest_migration();
            let note = match found.cmp(&known) {
                std::cmp::Ordering::Equal => "up to date",
                std::cmp::Ordering::Less => "older; the app would migrate it on open",
                std::cmp::Ordering::Greater => "written by a newer version",
            };
            println!("migrations      {found} of {known} — {note}");
        }
        Err(e) => {
            println!("migrations      FAILED — {e}");
            ok = false;
        }
    }

    match read_every_table(&conn) {
        Ok(counts) => {
            for (table, typed) in counts {
                let sql: i64 = conn
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap_or(-1);
                let status = if typed as i64 == sql {
                    "ok"
                } else {
                    "MISMATCH"
                };
                ok &= typed as i64 == sql;
                println!("  {table:<14} {typed:>8} rows read  {status}");
            }
        }
        Err(e) => {
            println!("tables          FAILED — {e}");
            ok = false;
        }
    }

    println!(
        "{}",
        if ok {
            "result          everything read"
        } else {
            "result          FAILED"
        }
    );
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
