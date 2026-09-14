//! The server over a real socket: the client address reaches the routes, `/ws` upgrades and
//! pushes updates, and the web UI and Express-style 404s are served. The parity harness
//! drives the router in-process; this covers what only a listening server exercises.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use jukebox_core::config::ConfigStore;
use jukebox_core::player::SilentPlayer;
use jukebox_server::net::{Network, NoFolderPicker};
use jukebox_server::{serve, App, Options, SetupAccess};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct NoNetwork;

impl Network for NoNetwork {
    fn lan_addresses(&self) -> Vec<String> {
        Vec::new()
    }
    fn hostname(&self, ip: &str) -> String {
        ip.into()
    }
}

async fn start() -> (tempfile::TempDir, Arc<App>, u16) {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Arc::new(
        App::new(Options {
            config: Arc::new(ConfigStore::open(dir.path().join("config.json")).unwrap()),
            database: dir.path().join("jukebox.db"),
            player: Arc::new(SilentPlayer::new()),
            network: Arc::new(NoNetwork),
            folder_picker: Arc::new(NoFolderPicker),
            running_port: port,
            version: "test".into(),
            setup: SetupAccess::HostOnly,
        })
        .unwrap(),
    );
    let serving = app.clone();
    tokio::spawn(async move { serve(&serving, listener).await });
    (dir, app, port)
}

/// A raw HTTP/1.1 exchange: the status line, the headers as text, and the body.
async fn request(port: u16, method: &str, path: &str) -> (String, String, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream
        .write_all(
            format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(raw[..split].to_vec()).unwrap();
    let (status, headers) = head.split_once("\r\n").unwrap();
    (
        status.to_string(),
        headers.to_lowercase(),
        raw[split + 4..].to_vec(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_listening_server_answers_like_electron() {
    let (_dir, _app, port) = start().await;

    let (status, headers, body) = request(port, "GET", "/api/auth").await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(headers.contains("content-type: application/json; charset=utf-8"));
    let auth: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        auth["isLocal"], true,
        "the connection's address reaches the routes"
    );

    let (status, headers, body) = request(port, "HEAD", "/api/nope").await;
    assert_eq!(status, "HTTP/1.1 404 Not Found");
    // "Cannot HEAD /api/nope": the length of the page a GET would get, without the page.
    assert!(headers.contains("content-length: 148"), "{headers}");
    assert!(body.is_empty());

    let (status, _, body) = request(port, "GET", "/").await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(!body.is_empty());

    let (status, _, body) = request(port, "GET", "/ws").await;
    assert_eq!(
        status, "HTTP/1.1 404 Not Found",
        "a plain GET of /ws isn't an upgrade"
    );
    assert!(String::from_utf8_lossy(&body).contains("Cannot GET /ws"));
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn next(socket: &mut Socket) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("a message within 5 s")
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_clients_get_the_queue_and_its_changes() {
    let (_dir, app, port) = start().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();

    let first = next(&mut socket).await;
    assert_eq!(
        first["type"], "queue",
        "the queue as it stands, on connecting"
    );
    assert_eq!(first["payload"]["nowPlaying"]["entry"], Value::Null);

    // Nothing queued and no filler: advancing pauses the player and rebroadcasts.
    let engine = app.engine().clone();
    tokio::task::spawn_blocking(move || engine.advance().unwrap())
        .await
        .unwrap();
    let progress = next(&mut socket).await;
    assert_eq!(progress["type"], "progress");
    assert_eq!(progress["payload"]["playing"], false);
    let queue = next(&mut socket).await;
    assert_eq!(queue["type"], "queue");
}

/// First-run setup from a guest's browser would let them choose the admin password.
#[tokio::test]
async fn setup_can_be_limited_to_the_host() {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    for (access, from, status) in [
        (SetupAccess::HostOnly, "192.0.2.21", 403),
        (SetupAccess::HostOnly, "127.0.0.1", 200),
        (SetupAccess::Disabled, "127.0.0.1", 403),
        (SetupAccess::Anyone, "192.0.2.21", 200),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(Options {
            config: Arc::new(ConfigStore::open(dir.path().join("config.json")).unwrap()),
            database: dir.path().join("jukebox.db"),
            player: Arc::new(SilentPlayer::new()),
            network: Arc::new(NoNetwork),
            folder_picker: Arc::new(NoFolderPicker),
            running_port: 8080,
            version: "test".into(),
            setup: access,
        })
        .unwrap();
        let mut request = Request::post("/api/setup")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"adminPassword":"secret"}"#))
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(std::net::SocketAddr::new(
                from.parse().unwrap(),
                1,
            )));
        let response = app.router().oneshot(request).await.unwrap();
        let got = response.status().as_u16();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            got,
            status,
            "{access:?} from {from}: {}",
            String::from_utf8_lossy(&body)
        );
    }
}
