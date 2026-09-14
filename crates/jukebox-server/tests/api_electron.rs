//! The golden-response parity harness: the Rust server answers every request in the scripts
//! in tests/fixtures the way Electron's server did.
//!
//! Each `<name>-electron.json` was recorded by scripts/api-oracle.mjs, which runs Electron's
//! real server against the library fixtures. These tests replay the same script — the same
//! simulated clients, fixed network answers, silent player and restarts — and compare every
//! status, the headers that matter, every body, and every message each WebSocket client
//! received.

use std::collections::BTreeMap;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request};
use http_body_util::BodyExt;
use jukebox_core::config::ConfigStore;
use jukebox_core::player::SilentPlayer;
use jukebox_server::net::{Network, NoFolderPicker};
use jukebox_server::realtime::Subscription;
use jukebox_server::{App, Options, SetupAccess};
use regex::Regex;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use tower::ServiceExt;

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// The network answers api-net-stub.mjs gives Electron.
struct FixedNetwork;

impl Network for FixedNetwork {
    fn lan_addresses(&self) -> Vec<String> {
        vec!["192.0.2.10".into()]
    }

    fn hostname(&self, ip: &str) -> String {
        if ip == "127.0.0.1" {
            return "jukebox-host".into();
        }
        match ip
            .strip_prefix("192.0.2.")
            .and_then(|n| n.parse::<u8>().ok())
        {
            Some(n) if n < 50 => format!("device-{n}"),
            _ => ip.into(),
        }
    }
}

/// api-normalize.mjs, rule for rule.
fn normalize(value: Value, root: &str, key: Option<&str>) -> Value {
    static TIME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z").unwrap());
    static TOKEN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"kj_session=[0-9a-f]{64}").unwrap());
    static EXPIRES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Expires=[^;]+").unwrap());
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap());
    match value {
        Value::String(s) => {
            let mut s = TIME
                .replace_all(&s.replace(root, "/music"), "<time>")
                .into_owned();
            match key {
                Some("version") => s = "<version>".into(),
                Some("set-cookie") => {
                    s = TOKEN.replace(&s, "kj_session=<token>").into_owned();
                    s = EXPIRES.replace(&s, "Expires=<date>").into_owned();
                }
                Some("content-disposition") => s = DATE.replace(&s, "<date>").into_owned(),
                _ => {}
            }
            Value::String(s)
        }
        Value::Number(n) if key == Some("duration") => {
            // JSON.parse and JSON.stringify make no difference between 1 and 1.0.
            let rounded = (n.as_f64().unwrap() * 10.0).round() / 10.0;
            if rounded.fract() == 0.0 {
                json!(rounded as i64)
            } else {
                json!(rounded)
            }
        }
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|v| normalize(v, root, key)).collect())
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let v = normalize(v, root, Some(&k));
                    (k, v)
                })
                .collect(),
        ),
        other => other,
    }
}

/// Where the Rust build answers differently on purpose, its answer is mapped back to
/// Electron's before comparing: (Rust value, Electron value, why).
const DELIBERATE: &[(&str, &str, &str)] = &[(
    "AC/DC",
    "AC",
    "Electron's tag reader split an ID3v2.3 artist at every '/'; see jukebox-core's library tests",
)];

fn deliberate(value: Value) -> Value {
    match value {
        Value::String(s) => match DELIBERATE.iter().find(|d| d.0 == s) {
            Some(d) => json!(d.1),
            None => Value::String(s),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(deliberate).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, deliberate(v))).collect())
        }
        other => other,
    }
}

/// The oracle's `describe`: status, the compared headers, and the body by type.
async fn describe(response: axum::response::Response) -> Value {
    const COMPARED: &[&str] = &[
        "content-type",
        "content-length",
        "content-range",
        "accept-ranges",
        "cache-control",
        "content-disposition",
        "set-cookie",
        "content-security-policy",
        "x-content-type-options",
    ];
    let status = response.status().as_u16();
    let head = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();

    let mut headers = Map::new();
    for name in COMPARED {
        let values: Vec<String> = head
            .get_all(*name)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        match (*name, values.as_slice()) {
            (_, []) => {}
            ("set-cookie", _) => {
                headers.insert(name.to_string(), json!(values));
            }
            (_, [one, ..]) => {
                headers.insert(name.to_string(), json!(one));
            }
        }
    }
    // On the wire, hyper sends the length of a body whose size it knows; in-process there's
    // no wire, so add it the same way.
    if !headers.contains_key("content-length") {
        headers.insert("content-length".into(), json!(bytes.len().to_string()));
    }

    let content_type = headers
        .get("content-type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let body = if content_type.starts_with("application/json") {
        json!({ "json": serde_json::from_slice::<Value>(&bytes).unwrap() })
    } else if content_type.starts_with("text/csv") {
        json!({ "csvLines": String::from_utf8_lossy(&bytes).split("\r\n").count() })
    } else if content_type.starts_with("text/") {
        json!({ "text": String::from_utf8_lossy(&bytes) })
    } else {
        let sha1: String = Sha1::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        json!({ "bytes": bytes.len(), "sha1": sha1 })
    };
    if body.get("json").is_some() || body.get("csvLines").is_some() {
        headers.remove("content-length");
    }
    json!({ "status": status, "headers": headers, "body": body })
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Progress is throttled by wall-clock time, so a message that only moved the position may
/// or may not be sent depending on how fast the steps ran. Messages that start, stop or
/// change the song are always sent, and those are what's compared.
fn significant(messages: Vec<Value>) -> Vec<Value> {
    let mut last: Option<(Value, Value)> = None;
    messages
        .into_iter()
        .filter(|m| {
            if m["type"] != "progress" {
                return true;
            }
            let key = (
                m["payload"]["playing"].clone(),
                m["payload"]["trackId"].clone(),
            );
            let keep = last.as_ref() != Some(&key);
            last = Some(key);
            keep
        })
        .collect()
}

fn fill(value: &Value, root: &str, port: u64) -> Value {
    match value {
        Value::String(s) => json!(s
            .replace("{root}", root)
            .replace("{port}", &port.to_string())),
        Value::Array(items) => Value::Array(items.iter().map(|v| fill(v, root, port)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), fill(v, root, port)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn start(data: &Path, port: u64) -> App {
    App::new(Options {
        config: Arc::new(ConfigStore::open(data.join("config.json")).unwrap()),
        database: data.join("jukebox.db"),
        player: Arc::new(SilentPlayer::new()),
        network: Arc::new(FixedNetwork),
        folder_picker: Arc::new(NoFolderPicker),
        running_port: port as u16,
        version: env!("CARGO_PKG_VERSION").into(),
        setup: SetupAccess::Anyone,
    })
    .unwrap()
}

async fn blocking(work: impl FnOnce() + Send + 'static) {
    tokio::task::spawn_blocking(work).await.unwrap();
}

/// Replays `<name>.json` against the Rust server and compares with `<name>-electron.json`.
async fn replay(name: &str) {
    let script = fixture(&format!("{name}.json"));
    let recorded = fixture(&format!("{name}-electron.json"));
    let port = script["port"].as_u64().unwrap();

    // The same starting point as the oracle: a data folder with a config, and the library.
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let music = dir.path().join("music");
    fs::create_dir(&data).unwrap();
    let library =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../jukebox-core/tests/fixtures/library");
    match script["setup"]["library"].as_array() {
        Some(files) => {
            fs::create_dir(&music).unwrap();
            for file in files {
                let file = file.as_str().unwrap();
                fs::copy(library.join(file), music.join(file)).unwrap();
            }
        }
        None => copy_dir(&library, &music),
    }
    let root = music.to_str().unwrap().to_string();
    let mut config = Map::new();
    config.insert("port".into(), json!(port));
    if let Some(extra) = script["setup"]["config"].as_object() {
        for (key, value) in extra {
            config.insert(key.clone(), fill(value, &root, port));
        }
    }
    fs::write(
        data.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    let mut app = start(&data, port);
    let mut router = app.router();
    let clients = script["clients"].as_object().unwrap();
    let mut subscriptions: BTreeMap<String, Subscription> = BTreeMap::new();
    let mut received: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut cookie: Option<String> = None;
    let mut responses = Vec::new();

    for step in script["steps"].as_array().unwrap() {
        match step["do"].as_str() {
            Some("ws-open") => {
                let client = step["client"].as_str().unwrap();
                let ip = clients[client].as_str().unwrap();
                subscriptions.insert(client.into(), app.hub().subscribe(ip));
                continue;
            }
            Some("wait-scan") => {
                while app.scanner().status().scanning {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                continue;
            }
            Some("track-ended") => {
                let engine = app.engine().clone();
                blocking(move || engine.track_ended().unwrap()).await;
                continue;
            }
            Some("player-ready") => {
                let engine = app.engine().clone();
                blocking(move || engine.maybe_start().unwrap()).await;
                continue;
            }
            Some("delete-file") => {
                fs::remove_file(music.join(step["file"].as_str().unwrap())).unwrap();
                continue;
            }
            Some("ws-close") => {
                for (client, mut subscription) in std::mem::take(&mut subscriptions) {
                    let list = received.entry(client).or_default();
                    while let Ok(text) = subscription.messages.try_recv() {
                        list.push(serde_json::from_str(&text).unwrap());
                    }
                }
                continue;
            }
            Some("restart") => {
                // Everything in memory goes; the data folder stays. The browser keeps its
                // cookie.
                drop(router);
                drop(app);
                app = start(&data, port);
                router = app.router();
                continue;
            }
            Some(other) => panic!("unknown action {other}"),
            None => {}
        }

        let ip = clients[step["ip"].as_str().unwrap()].as_str().unwrap();
        let path = fill(&step["path"], &root, port);
        let mut request = Request::builder()
            .method(step["method"].as_str().unwrap())
            .uri(path.as_str().unwrap());
        if let Some(extra) = step["headers"].as_object() {
            for (name, value) in extra {
                request = request.header(name, value.as_str().unwrap());
            }
        }
        if step["admin"] == true {
            if let Some(cookie) = &cookie {
                request = request.header(header::COOKIE, cookie);
            }
        }
        let body = if let Some(json) = step.get("json") {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&fill(json, &root, port)).unwrap())
        } else if let Some(text) = step["text"].as_str() {
            request = request.header(header::CONTENT_TYPE, step["contentType"].as_str().unwrap());
            Body::from(text.to_string())
        } else {
            Body::empty()
        };
        let mut request = request.body(body).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip.parse().unwrap(), 50000)));

        let response = router.clone().oneshot(request).await.unwrap();
        let is_login = step["id"].as_str().unwrap().starts_with("login");
        if is_login && response.status() == 200 {
            if let Some(set) = response.headers().get(header::SET_COOKIE) {
                cookie = Some(set.to_str().unwrap().split(';').next().unwrap().to_string());
            }
        }
        let mut described = describe(response).await;
        described
            .as_object_mut()
            .unwrap()
            .insert("id".into(), step["id"].clone());
        responses.push(described);
    }

    // Compare.
    let ours = deliberate(normalize(
        json!({ "responses": responses, "websocket": received }),
        &root,
        None,
    ));
    let mut problems = Vec::new();
    let expected = recorded["responses"].as_array().unwrap();
    let actual = ours["responses"].as_array().unwrap();
    assert_eq!(actual.len(), expected.len(), "number of responses");
    for (want, got) in expected.iter().zip(actual) {
        for part in ["status", "headers", "body"] {
            if want[part] != got[part] {
                problems.push(format!(
                    "{} {part}:\n    electron {}\n    rust     {}",
                    want["id"], want[part], got[part]
                ));
            }
        }
    }
    for (client, want) in recorded["websocket"].as_object().unwrap() {
        let want = significant(want.as_array().unwrap().clone());
        let got = significant(
            ours["websocket"][client]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        if want.len() != got.len() {
            problems.push(format!(
                "websocket {client}: {} messages, Electron sent {}",
                got.len(),
                want.len()
            ));
        }
        for (i, (w, g)) in want.iter().zip(&got).enumerate() {
            if w != g {
                problems.push(format!(
                    "websocket {client} message {i}:\n    electron {w}\n    rust     {g}"
                ));
                break;
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{name}: {} differences:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// Every route, its validation, and its errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_route_matches_electron() {
    replay("api-routes").await;
}

/// Longer queue flows: fills, votes, bans, pruning and restarts together.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_flows_match_electron() {
    replay("api-flows").await;
}
