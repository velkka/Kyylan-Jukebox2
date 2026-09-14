# Kyylan Jukebox

A LAN party jukebox. The host machine plays music through its own speakers, and guests open
a URL in their phone or laptop browser to browse the library and queue songs. Admins log in
to manage the queue, pick the audio output and set up the library.

- **One small program** for Windows, macOS and Linux, written in Rust.
- **Browser UI** served over the LAN, so guests install nothing.
- **Plays through the host's own audio output**, which is selectable.
- Guests are identified by IP with no login; admins log in with a password.

Installing, upgrading from v0.2.x and removing it: [packaging/INSTALL.md](packaging/INSTALL.md).

## Architecture

```
kyylan-jukebox
├─ Server (axum): HTTP API + WebSocket on 0.0.0.0:<port>   ← LAN guests connect here
│  └─ the React web UI, embedded in the program
├─ Engine: queue, standby playlist, bans, stats — SQLite (rusqlite)
├─ Library scanner (lofty)
├─ Player: symphonia and libopus → resampling → cpal (CoreAudio, WASAPI, ALSA)
└─ Windows and macOS: tray / menu-bar icon (tray-icon); Linux: none, it's a system service
```

| Crate | What's in it |
|---|---|
| `crates/jukebox-core` | Config, database, library, and the queue engine |
| `crates/jukebox-server` | The API, live updates and the embedded UI |
| `crates/jukebox-audio` | Decoding and output |
| `crates/kyylan-jukebox` | The program: command line, tray, logging, shutdown, `import` and `uninstall` |

The web UI (`src/renderer/web`) is shared with the Electron build that v0.2.x shipped. That
build's code is still in `src/main`. The Rust server's tests are held to responses recorded
from it.

## Requirements

- Rust (stable, 1.85 or newer)
- Node.js 20 or newer, for the web UI
- Linux: `libasound2-dev`

## Develop

```bash
npm ci
npm run build                     # icons, and the web UI into out/renderer
cargo run -p kyylan-jukebox -- --data-dir /tmp/jukebox-dev
```

Open http://127.0.0.1:8080/ for first-run setup.

- **Data.** `--data-dir` keeps development data, and its logs, apart from your real install.
- **UI changes.** A debug build reads the UI from `out/renderer` as it runs, so rebuilding the
  UI is enough.
- **Tests.** Run them with `cargo test --workspace`.
- **Other commands.** `--list-devices`, `--check-config` and the rest are in
  `cargo run -p kyylan-jukebox -- --help`.

Icons are generated from `build/logo-source.png` by `scripts/gen-icons.cjs`, which
`npm run build` runs.

## Releases

Pushing a version tag builds the installers on GitHub-hosted runners
(`.github/workflows/release.yml`):

- the Windows `.msi`
- the universal macOS `.pkg`
- Linux `.deb`s for amd64 and arm64

Each installer is installed, used and uninstalled on its runner, and then a GitHub Release
is published. The same run without publishing happens for every push that changes the
packaging, and on demand from the Actions tab. Releasing, and rolling back to v0.2.x, are
described in [RELEASING.md](RELEASING.md).

The installers aren't code-signed. The macOS app is ad-hoc signed, so it runs; Gatekeeper
and SmartScreen warn the first time.

## Running a party

1. Install on the machine wired to the speakers. On Windows and macOS the console opens for
   first-run setup. On Linux the install prints the address and a generated admin password.
2. Log in as admin → **Manage** → add your music folder(s) and pick the audio output.
3. Tell guests the URL shown on the home screen, or copy it from the tray menu. They browse
   and queue songs; you manage the queue.

On Windows and macOS the jukebox lives in the tray (the menu-bar eyes) and starts when you
sign in. Quit from the tray. On Linux it runs as the `kyylan-jukebox` service from boot.

## Configuration

Settings live in a plaintext `config.json` in the data directory:

- **Windows:** `%APPDATA%\kyylan-jukebox`
- **macOS:** `~/Library/Application Support/kyylan-jukebox`
- **Linux:** `/var/lib/kyylan-jukebox`, linked from `/etc/kyylan-jukebox`

It holds the server port, admin password, library folders, per-user queue limit, output
device and the rest of the admin panel's settings. Check a hand-edited file with
`kyylan-jukebox --check-config`.

> The admin password is stored in plaintext by design (LAN party convenience). Don't
> reuse a sensitive password.
