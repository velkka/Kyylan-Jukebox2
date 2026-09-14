//! The handful of JavaScript and Node.js behaviours the library code leaned on implicitly.
//! Each one decides a value that ends up in the database or a response, so each is
//! reproduced exactly rather than approximated with the nearest Rust idiom.

/// `String.prototype.trim()`. JavaScript's whitespace differs from Rust's `char::is_whitespace`
/// in two places: it includes U+FEFF (the byte-order mark) and excludes U+0085 (NEL).
pub fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == '\u{feff}' || (c != '\u{85}' && c.is_whitespace()))
}

/// `parseInt(s, 10)`, or `None` where JavaScript gives `NaN`: leading whitespace, an optional
/// sign, then as many decimal digits as there are.
pub fn parse_int(s: &str) -> Option<i64> {
    let s = trim(s);
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let end = digits
        .bytes()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(digits.len());
    let value: i64 = digits[..end].parse().ok()?;
    Some(if negative { -value } else { value })
}

/// `String(n)` for a number that came out of SQLite: whole numbers without a decimal point,
/// everything else as the shortest representation that round-trips.
pub fn number_to_string(n: f64) -> String {
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_992.0;
    if n.is_finite() && n.fract() == 0.0 && n.abs() < MAX_SAFE_INTEGER {
        format!("{}", n as i64)
    } else {
        serde_json::to_string(&n).unwrap_or_else(|_| "NaN".into())
    }
}

/// Node's `path.join(dir, name)` on this platform, which normalizes the result. The library
/// stores every track under the path this produces, so it must match what Electron stored
/// character for character — a mismatch would make a rescan drop and re-add every track.
pub fn path_join(dir: &str, name: &str) -> String {
    if cfg!(windows) {
        normalize_win32(&format!("{dir}\\{name}"))
    } else {
        normalize_posix(&format!("{dir}/{name}"))
    }
}

/// `path.posix.normalize`.
pub fn normalize_posix(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let absolute = path.starts_with('/');
    let mut tail = normalize_segments(path, '/', !absolute);
    if tail.is_empty() && !absolute {
        tail.push('.');
    }
    if !tail.is_empty() && path.ends_with('/') {
        tail.push('/');
    }
    if absolute {
        format!("/{tail}")
    } else {
        tail
    }
}

/// `path.win32.normalize`, for the prefixes a library folder can have: a drive letter
/// (`C:\Music`), a UNC share (`\\nas\music`) or a rooted path (`\Music`).
pub fn normalize_win32(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let path = path.replace('/', "\\");
    let (device, rest, absolute) = split_win32_root(&path);
    let mut tail = normalize_segments(rest, '\\', !absolute);
    if tail.is_empty() && !absolute {
        tail.push('.');
    }
    if !tail.is_empty() && rest.ends_with('\\') {
        tail.push('\\');
    }
    if absolute {
        format!("{device}\\{tail}")
    } else {
        format!("{device}{tail}")
    }
}

fn split_win32_root(path: &str) -> (&str, &str, bool) {
    let bytes = path.as_bytes();
    if path.starts_with("\\\\") && !path.starts_with("\\\\\\") {
        // \\server\share: both parts are needed to make a device.
        let after = &path[2..];
        if let Some(server_end) = after.find('\\').filter(|&i| i > 0) {
            let share = &after[server_end + 1..];
            let share_end = share.find('\\').unwrap_or(share.len());
            if share_end > 0 {
                let device_len = 2 + server_end + 1 + share_end;
                return (&path[..device_len], &path[device_len..], true);
            }
        }
        return ("", path, true);
    }
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let rest = &path[2..];
        return (&path[..2], rest, rest.starts_with('\\'));
    }
    ("", path, path.starts_with('\\'))
}

/// Node's `normalizeString`: drops empty and `.` segments and resolves `..` lexically.
fn normalize_segments(path: &str, sep: char, allow_above_root: bool) -> String {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split(sep) {
        match segment {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|last| *last != "..") {
                    out.pop();
                } else if allow_above_root {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    out.join(&sep.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_matches_javascript() {
        assert_eq!(trim(" \t\u{feff}x\u{a0}\n"), "x");
        assert_eq!(trim("\u{85}x\u{85}"), "\u{85}x\u{85}");
        assert_eq!(trim(" \u{2003} "), "");
    }

    #[test]
    fn parse_int_matches_javascript() {
        assert_eq!(parse_int("3/12"), Some(3));
        assert_eq!(parse_int("07"), Some(7));
        assert_eq!(parse_int("  -4x"), Some(-4));
        assert_eq!(parse_int("+9"), Some(9));
        assert_eq!(parse_int("A1"), None);
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_int("-"), None);
        assert_eq!(parse_int("2001-05-02"), Some(2001));
    }

    #[test]
    fn numbers_print_like_javascript() {
        assert_eq!(number_to_string(215.0), "215");
        assert_eq!(number_to_string(245.33333333333334), "245.33333333333334");
        assert_eq!(number_to_string(1.0710204081632653), "1.0710204081632653");
        assert_eq!(number_to_string(0.5), "0.5");
    }

    #[test]
    fn posix_normalize_matches_node() {
        // Expected values from Node's path.posix.normalize / path.posix.join.
        assert_eq!(
            normalize_posix("/Users/me/MUSIC//music"),
            "/Users/me/MUSIC/music"
        );
        assert_eq!(normalize_posix("/a/b/../c/./d"), "/a/c/d");
        assert_eq!(normalize_posix("/../a"), "/a");
        assert_eq!(normalize_posix("a/../../b"), "../b");
        assert_eq!(normalize_posix("//srv/music/"), "/srv/music/");
        assert_eq!(normalize_posix(""), ".");
        assert_eq!(normalize_posix("./"), "./");
        assert_eq!(normalize_posix("/"), "/");
    }

    #[test]
    fn win32_normalize_matches_node() {
        // Expected values from Node's path.win32.normalize.
        assert_eq!(normalize_win32("C:/Music//Albums/"), "C:\\Music\\Albums\\");
        assert_eq!(
            normalize_win32("C:\\Music\\..\\Other\\x.mp3"),
            "C:\\Other\\x.mp3"
        );
        assert_eq!(normalize_win32("C:"), "C:.");
        assert_eq!(normalize_win32("C:\\"), "C:\\");
        assert_eq!(
            normalize_win32("\\\\nas\\music\\a\\..\\b"),
            "\\\\nas\\music\\b"
        );
        assert_eq!(normalize_win32("\\\\nas\\music"), "\\\\nas\\music\\");
        assert_eq!(normalize_win32("\\Music\\.\\x"), "\\Music\\x");
        assert_eq!(normalize_win32("music\\..\\..\\x"), "..\\x");
    }
}
