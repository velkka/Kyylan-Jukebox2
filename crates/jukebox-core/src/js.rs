//! The JavaScript and Node.js behaviours the Electron code leaned on implicitly: trimming,
//! number parsing and printing, `Number()` and `String()` applied to request bodies, dates,
//! and path joining. Each one decides a value that ends up in the database or a response, so
//! each is reproduced exactly rather than approximated with the nearest Rust idiom.

use serde_json::Value;

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
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if n.fract() == 0.0 && n.abs() < MAX_SAFE_INTEGER {
        format!("{}", n as i64)
    } else {
        serde_json::to_string(&n).unwrap_or_else(|_| "NaN".into())
    }
}

/// `Number(value)` for a value from a parsed JSON body, with `None` standing for a key that
/// isn't there (`undefined`, which is `NaN`).
pub fn to_number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => string_to_number(s),
        // An array becomes its elements joined with commas first: [] is 0, [5] is 5.
        Some(array @ Value::Array(_)) => string_to_number(&to_string(array)),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// `Number(string)`: surrounding whitespace ignored, empty is 0, `0x`/`0o`/`0b` prefixes
/// and `Infinity` accepted, and anything else that isn't a complete decimal literal is `NaN`.
pub fn string_to_number(s: &str) -> f64 {
    let s = trim(s);
    if s.is_empty() {
        return 0.0;
    }
    let radix = |digits: &str, radix: u32| {
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return f64::NAN;
        }
        digits.chars().fold(0.0, |n, c| {
            n * f64::from(radix) + f64::from(c.to_digit(radix).unwrap())
        })
    };
    match s.get(..2) {
        Some("0x" | "0X") => return radix(&s[2..], 16),
        Some("0o" | "0O") => return radix(&s[2..], 8),
        Some("0b" | "0B") => return radix(&s[2..], 2),
        _ => {}
    }
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    if unsigned == "Infinity" {
        return if s.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // [digits][.digits][e[+-]digits], with at least one digit before the exponent.
    let bytes = unsigned.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - start
    };
    let mut mantissa = digits(&mut i);
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        mantissa += digits(&mut i);
    }
    if mantissa == 0 {
        return f64::NAN;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return f64::NAN;
        }
    }
    if i != bytes.len() {
        return f64::NAN;
    }
    s.parse().unwrap_or(f64::NAN)
}

/// `String(value)` for a value from a parsed JSON body.
pub fn to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                v => to_string(v),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `Number.isInteger(n)`.
pub fn is_integer(n: f64) -> bool {
    n.is_finite() && n.trunc() == n
}

/// `Date.now()`.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// `new Date(ms).toISOString()`.
pub fn iso_from_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `new Date(iso).getTime()` for the timestamps this app writes, or `None` for one it can't
/// read (`NaN`).
pub fn ms_from_iso(iso: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|t| t.timestamp_millis())
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
    fn number_conversions_match_javascript() {
        // Expected values from Node's Number() and String().
        let nan = f64::NAN;
        for (input, want) in [
            ("12", 12.0),
            (" 12 ", 12.0),
            ("", 0.0),
            ("  ", 0.0),
            ("0x1F", 31.0),
            ("0o17", 15.0),
            ("0b101", 5.0),
            ("-0x10", nan),
            ("1e3", 1000.0),
            ("1.", 1.0),
            ("  .5", 0.5),
            ("e5", nan),
            ("1e", nan),
            ("+7", 7.0),
            ("-7.25", -7.25),
            ("-Infinity", f64::NEG_INFINITY),
            ("inf", nan),
            ("NaN", nan),
            ("1_000", nan),
            ("12abc", nan),
            ("0x", nan),
            ("\u{663}", nan),
        ] {
            let got = string_to_number(input);
            assert!(
                got == want || (got.is_nan() && want.is_nan()),
                "{input:?}: {got}"
            );
        }
        use serde_json::json;
        for (value, number, string) in [
            (json!(null), 0.0, "null"),
            (json!(true), 1.0, "true"),
            (json!([]), 0.0, ""),
            (json!([5]), 5.0, "5"),
            (json!([1, 2]), nan, "1,2"),
            (json!([null]), 0.0, ""),
            (json!({}), nan, "[object Object]"),
            (json!(2.5), 2.5, "2.5"),
            (json!(3.0), 3.0, "3"),
        ] {
            let got = to_number(Some(&value));
            assert!(
                got == number || (got.is_nan() && number.is_nan()),
                "{value}: {got}"
            );
            assert_eq!(to_string(&value), string);
        }
        assert!(to_number(None).is_nan());
        assert_eq!(number_to_string(f64::NAN), "NaN");
        assert_eq!(number_to_string(f64::INFINITY), "Infinity");
        assert!(is_integer(5.0) && !is_integer(5.5) && !is_integer(f64::INFINITY));
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
