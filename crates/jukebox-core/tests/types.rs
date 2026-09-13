//! Every API type must turn the Electron server's JSON into Rust and back without losing,
//! adding or reshaping anything.
//!
//! The committed samples are synthetic but shaped like real responses. Set
//! `KYYLAN_API_CAPTURES` to a folder of responses captured from a running Electron build
//! (files named `<TypeName>.<anything>.json`) to hold the types to the real thing as well.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use jukebox_core::types::*;

fn round_trips<T: DeserializeOwned + Serialize>(input: &Value) {
    let parsed: T = serde_json::from_value(input.clone())
        .unwrap_or_else(|e| panic!("{} rejected {input}: {e}", std::any::type_name::<T>()));
    let back = serde_json::to_value(&parsed).unwrap();
    assert_eq!(
        &back,
        input,
        "{} changed the JSON",
        std::any::type_name::<T>()
    );
}

fn track(id: i64, duration: Value) -> Value {
    json!({
        "id": id, "title": "Going Under", "artist": "Evanescence", "album": "Fallen",
        "albumArtist": "Evanescence", "genre": "Rock, Goth Rock, Nu Metal", "duration": duration,
        "trackNo": 1, "discNo": 1, "year": 2003, "artHash": "ef490c28adaae5ba7b08c26af9605ce086f282a6"
    })
}

fn bare_track() -> Value {
    json!({
        "id": 964, "title": "Löyly", "artist": null, "album": null, "albumArtist": null, "genre": null,
        "duration": 9.769795918367347, "trackNo": null, "discNo": null, "year": null, "artHash": null
    })
}

fn entry(mine: bool) -> Value {
    json!({ "id": 101, "track": track(616, json!(215)), "addedByName": "oh7vm-mbp14", "mine": mine })
}

#[test]
fn setup_auth_and_settings() {
    round_trips::<PublicConfig>(
        &json!({ "configured": true, "port": 8090, "perUserQueueLimit": -3, "name": "Kyylan Jukebox", "version": "0.3.0" }),
    );
    round_trips::<SetupRequest>(&json!({ "adminPassword": "hunter2", "port": 8090 }));
    round_trips::<SetupRequest>(&json!({ "adminPassword": "hunter2" }));
    round_trips::<SetupResponse>(&json!({ "ok": true, "restartRequired": false, "port": 8090 }));
    round_trips::<LoginRequest>(&json!({ "password": "hunter2" }));
    round_trips::<AuthStatus>(&json!({ "isAdmin": true, "isLocal": false }));
    round_trips::<BrowseFolderResponse>(&json!({ "canceled": true }));
    round_trips::<BrowseFolderResponse>(
        &json!({ "canceled": false, "path": "/Users/example/Music" }),
    );
    round_trips::<AdminSettings>(&json!({
        "port": 8090, "perUserQueueLimit": 3, "downvoteSkipThreshold": -2,
        "sameSongCooldownMinutes": 60, "sameArtistCooldownMinutes": 0, "addRateLimitMinutes": 5
    }));
    round_trips::<AdminSettingsUpdate>(&json!({ "addRateLimitMinutes": 5 }));
    round_trips::<AdminSettingsUpdate>(&json!({}));
    round_trips::<SaveSettingsResponse>(&json!({ "restartRequired": true }));
    round_trips::<ErrorBody>(&json!({ "error": "Admin login required" }));
    round_trips::<HealthResponse>(
        &json!({ "name": "Kyylan Jukebox", "version": "0.3.0", "addresses": ["10.40.10.129", "100.114.224.66"], "port": 8090 }),
    );
}

#[test]
fn library() {
    round_trips::<Track>(&track(616, json!(215)));
    round_trips::<Track>(&track(617, json!(237.17333333333335)));
    round_trips::<Track>(&bare_track());
    round_trips::<TracksQuery>(&json!({ "search": "say you", "limit": 60, "offset": 120 }));
    round_trips::<TracksQuery>(&json!({ "artist": "Faith No More", "noAlbum": true }));
    round_trips::<TracksResponse>(
        &json!({ "tracks": [track(616, json!(215)), bare_track()], "total": 813, "limit": 60, "offset": 0 }),
    );
    round_trips::<ArtistsResponse>(&json!({
        "artists": [{ "artist": "Evanescence", "trackCount": 71, "albumCount": 5 }], "total": 6, "letters": ["E", "F", "#"]
    }));
    round_trips::<AlbumsResponse>(&json!({
        "albums": [
            { "album": "Fallen", "artist": "Evanescence", "artHash": "ef49", "trackCount": 12, "year": 2003 },
            { "album": "Introduce Yourself", "artist": "Faith No More", "artHash": null, "trackCount": 10, "year": null }
        ],
        "total": 2
    }));
    round_trips::<ScanStatus>(&json!({
        "scanning": false, "processed": 356, "added": 356, "updated": 0, "removed": 346, "total": 356,
        "startedAt": "2026-09-12T17:36:40.001Z", "finishedAt": "2026-09-12T17:36:43.420Z", "error": null
    }));
    round_trips::<LibraryPathsResponse>(
        &json!({ "paths": [{ "path": "/Users/example/Music/", "trackCount": 356 }], "total": 356 }),
    );
}

#[test]
fn player() {
    round_trips::<DevicesResponse>(&json!({
        "devices": [{ "deviceId": "coreaudio:BuiltInSpeakerDevice", "label": "MacBook Pro Speakers" }], "selected": null
    }));
    round_trips::<OutputResponse>(&json!({ "selected": "coreaudio:BuiltInSpeakerDevice" }));
    round_trips::<PlaybackState>(
        &json!({ "trackId": 961, "playing": false, "position": 0, "duration": 0, "volume": 1 }),
    );
    round_trips::<PlaybackState>(
        &json!({ "trackId": null, "playing": true, "position": 12.345, "duration": 248.53333333333333, "volume": 0.35 }),
    );
}

#[test]
fn queue_and_standby() {
    let now_playing = json!({
        "entry": entry(true), "position": 31.5, "duration": 215, "playing": true, "isStandby": false,
        "downvotes": 1, "downvoteThreshold": 2, "downvotedByMe": true
    });
    round_trips::<NowPlaying>(&now_playing);
    round_trips::<QueueState>(
        &json!({ "nowPlaying": now_playing, "queue": [entry(false)], "perUserLimit": 3, "myQueueCount": 1 }),
    );
    round_trips::<QueueState>(&json!({
        "nowPlaying": { "entry": null, "position": 0, "duration": 0, "playing": false, "isStandby": false,
                        "downvotes": 0, "downvoteThreshold": 0, "downvotedByMe": false },
        "queue": [], "perUserLimit": 3, "myQueueCount": 0
    }));
    round_trips::<EnqueueRequest>(&json!({ "trackId": 616 }));
    round_trips::<StandbyState>(
        &json!({ "enabled": true, "shuffle": false, "random": true, "entries": [{ "id": 4, "track": bare_track() }] }),
    );
}

#[test]
fn stats_and_bans() {
    let history = json!([
        { "id": 5, "trackId": 618, "title": "Everybody's Fool", "artist": "Evanescence", "artHash": "ef49",
          "requestedByName": "oh7vm-mbp14", "source": "guest", "playedAt": "2026-09-12T07:16:18.961Z" },
        { "id": 6, "trackId": 961, "title": "Goodbye Baby", "artist": null, "artHash": null,
          "requestedByName": null, "source": "random", "playedAt": "2026-09-12T07:17:00.000Z" }
    ]);
    let top =
        json!([{ "trackId": 616, "title": "Going Under", "artist": "Evanescence", "count": 2 }]);
    // Admin view, then the redacted guest view of the same guest.
    round_trips::<StatsResponse>(&json!({
        "history": history, "topPlayed": top, "topDownvoted": [],
        "users": [{ "id": "10.40.10.129", "ip": "10.40.10.129", "name": "oh7vm-mbp14", "requests": 4, "downvotes": 1,
                    "banned": true, "bannedUntil": "2026-09-12T08:00:00.000Z" }],
        "totals": { "plays": 5, "requests": 4, "downvotes": 1 }
    }));
    round_trips::<StatsResponse>(&json!({
        "history": [], "topPlayed": [], "topDownvoted": top,
        "users": [{ "id": "4b84b15b", "ip": null, "name": "oh7vm-mbp14", "requests": 4, "downvotes": 1,
                    "banned": false, "bannedUntil": null }],
        "totals": { "plays": 0, "requests": 4, "downvotes": 1 }
    }));
    round_trips::<BansResponse>(&json!({ "bans": [
        { "ip": "127.0.0.1", "name": "oh7vm-mbp14", "bannedAt": "2026-09-12T07:28:06.878Z", "expiresAt": "2026-09-12T07:43:06.878Z" },
        { "ip": "10.0.0.9", "name": null, "bannedAt": "2026-09-12T07:28:06.928Z", "expiresAt": null }
    ]}));
    round_trips::<BanRequest>(&json!({ "ip": "10.0.0.9", "minutes": 15 }));
    round_trips::<BanRequest>(&json!({ "ip": "10.0.0.9" }));
}

#[test]
fn realtime_messages() {
    round_trips::<RealtimeMessage>(
        &json!({ "type": "progress", "payload": { "position": 42.1, "duration": 215, "playing": true, "trackId": 616 } }),
    );
    round_trips::<RealtimeMessage>(
        &json!({ "type": "progress", "payload": { "position": 0, "duration": 0, "playing": false, "trackId": null } }),
    );
    round_trips::<RealtimeMessage>(&json!({ "type": "queue", "payload": {
        "nowPlaying": { "entry": null, "position": 0, "duration": 0, "playing": false, "isStandby": true,
                        "downvotes": 0, "downvoteThreshold": -3, "downvotedByMe": false },
        "queue": [], "perUserLimit": 0, "myQueueCount": 0
    }}));
}

#[test]
fn numbers_are_written_the_way_javascript_writes_them() {
    let state = PlaybackState {
        track_id: Some(1),
        playing: true,
        position: 215.0,
        duration: f64::NAN,
        volume: -0.0,
    };
    assert_eq!(
        serde_json::to_string(&state).unwrap(),
        r#"{"trackId":1,"playing":true,"position":215,"duration":null,"volume":0}"#
    );
}

#[test]
fn optional_fields_are_left_out_rather_than_null() {
    let folder = BrowseFolderResponse {
        canceled: true,
        path: None,
    };
    assert_eq!(
        serde_json::to_string(&folder).unwrap(),
        r#"{"canceled":true}"#
    );
    let update = AdminSettingsUpdate {
        port: Some(8090),
        ..Default::default()
    };
    assert_eq!(serde_json::to_string(&update).unwrap(), r#"{"port":8090}"#);
}

/// Round-trips responses captured from a running Electron build, when provided.
#[test]
fn captured_electron_responses() {
    let Some(dir) = std::env::var_os("KYYLAN_API_CAPTURES") else {
        eprintln!("skipped: set KYYLAN_API_CAPTURES to a folder of captured responses");
        return;
    };
    let mut checked = 0;
    for file in std::fs::read_dir(dir).unwrap() {
        let path = file.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Some(type_name) = name.split('.').next() else {
            continue;
        };
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        match type_name {
            "HealthResponse" => round_trips::<HealthResponse>(&value),
            "PublicConfig" => round_trips::<PublicConfig>(&value),
            "AuthStatus" => round_trips::<AuthStatus>(&value),
            "TracksResponse" => round_trips::<TracksResponse>(&value),
            "ArtistsResponse" => round_trips::<ArtistsResponse>(&value),
            "AlbumsResponse" => round_trips::<AlbumsResponse>(&value),
            "QueueState" => round_trips::<QueueState>(&value),
            "StatsResponse" => round_trips::<StatsResponse>(&value),
            "PlaybackState" => round_trips::<PlaybackState>(&value),
            "AdminSettings" => round_trips::<AdminSettings>(&value),
            "LibraryPathsResponse" => round_trips::<LibraryPathsResponse>(&value),
            "ScanStatus" => round_trips::<ScanStatus>(&value),
            "StandbyState" => round_trips::<StandbyState>(&value),
            "BansResponse" => round_trips::<BansResponse>(&value),
            "DevicesResponse" => round_trips::<DevicesResponse>(&value),
            "ErrorBody" => round_trips::<ErrorBody>(&value),
            other => panic!("no mapping for captured type {other} ({name})"),
        }
        eprintln!("ok: {name}");
        checked += 1;
    }
    assert!(checked > 0, "the capture folder was empty");
}
