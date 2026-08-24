# Kyylan Jukebox

A LAN party jukebox. The host machine runs the app and plays music through its own
speakers; guests open a URL in their phone/laptop browser to browse the library and
queue songs. Admins log in to manage the queue, pick the audio output, and configure
the library.

- **Native app** for Windows & macOS (Electron)
- **Browser UI** served over the LAN — no install for guests
- **Host plays audio locally**; output device is selectable
- Guests are identified by IP (no login); admins log in with a password

## Architecture

```
Host machine (Electron app)
├─ Main process
│  ├─ Express + WebSocket server on 0.0.0.0:<port>   ← LAN guests connect here
│  ├─ SQLite: library, queue, settings
│  └─ Library scanner (music-metadata)
└─ Hidden "player" window (Chromium)
   └─ <audio> → host output device (setSinkId)
```

The React web UI (`src/renderer/web`) is served to guests **and** loaded in the host
window, so the host console is just an admin-logged-in guest. A separate hidden
renderer (`src/renderer/player`) is the actual audio sink.

## Requirements

- Node.js 20+ (developed on 22)

## Develop

```bash
npm install
npm run dev
```

`npm run dev` launches Electron with hot-reload. The host window loads the Vite dev
server; API/WebSocket calls are proxied to the embedded Express server on port 8080.

## Build & package

```bash
npm run build          # regenerate icons + compile main/preload/renderers into out/
npm run package:mac    # -> release/Kyylan-Jukebox-<ver>-arm64.dmg
npm run package:win    # -> release/…-setup.exe (installer) + …-portable.exe (x64)
npm run package:linux  # -> release/…-x86_64.AppImage + …-amd64.deb  (run on Linux)
```

Windows installers build **from macOS** — electron-builder downloads a bundled
Wine/NSIS toolchain automatically, no manual Wine install needed. Two Windows
artifacts are produced: an NSIS **installer** and a **portable** single-exe (handy
for a party host — run it without installing). Use `package:win:arm64` for ARM PCs.

### Automated releases

Pushing a version tag builds all three platforms on GitHub-hosted runners (macOS,
Windows and Linux each build natively) and publishes a GitHub Release with the
`.dmg`, Windows `.exe`s and Linux `.AppImage`/`.deb` attached
(`.github/workflows/release.yml`):

```bash
npm version 0.3.0            # bump + commit + tag v0.3.0
git push && git push --tags # → CI builds macOS + Windows and drafts the release
```

> Native-module note: packaging rebuilds `better-sqlite3` for the target platform.
> After a Windows build, run `npm run rebuild` before `npm run dev` again on macOS.

Icons are generated from `build/logo-source.png` by `scripts/gen-icons.cjs`
(run automatically by `build`). Replace that file and rebuild to change the logo.

## Running a party

1. Launch the app on the machine wired to the speakers; complete first-run setup.
2. In the browser window, log in as admin → **Manage** → add your music folder(s)
   and pick the audio output.
3. Tell guests the URL shown on the home screen (or copy it from the tray menu —
   right-click the menubar eyes). They browse and queue songs; you manage the queue.

Closing the window keeps the jukebox running in the menubar (tray). Quit from there.

## macOS note (dev)

The unsigned dev Electron runtime is blocked by Gatekeeper/XProtect on recent macOS
("… is malware"). The `postinstall` step clears the quarantine and applies an ad-hoc
signature automatically. If a launch is ever blocked, re-run it manually:

```bash
npm run fix-electron
```

Packaged `.dmg`/`.exe` builds are currently **unsigned** (no signing certificate is
installed). For personal/LAN use, ad-hoc sign the built app once so it runs locally:

```bash
codesign --force --deep --sign - "release/mac-arm64/Kyylan Jukebox.app"
```

### Signing the final builds (macOS)

The build is already signing-ready — hardened-runtime entitlements
(`build/entitlements.mac.plist`) and the electron-builder settings are in place.
Once you have an Apple **Developer ID Application** certificate in your keychain:

```bash
# Notarization credentials (set these yourself; never commit them):
export APPLE_ID="you@example.com"
export APPLE_APP_SPECIFIC_PASSWORD="xxxx-xxxx-xxxx-xxxx"   # appleid.apple.com → App-Specific Passwords
export APPLE_TEAM_ID="XXXXXXXXXX"

npm run package:mac
```

electron-builder auto-discovers the Developer ID cert, signs with the hardened
runtime, and notarizes when those three env vars are present. No config changes
needed. (Certificate + Apple Developer account setup: https://electron.build/code-signing)

### Signing the final builds (Windows)

Unsigned `.exe` builds trigger a SmartScreen warning on first run. To sign, point
electron-builder at your code-signing certificate and rebuild:

```bash
export WIN_CSC_LINK="/path/to/certificate.pfx"
export WIN_CSC_KEY_PASSWORD="your-pfx-password"
npm run package:win
```

## Configuration

Settings live in a plaintext `config.json` in the OS user-data dir
(`~/Library/Application Support/kyylan-jukebox/` on macOS). Includes the server port,
admin password, library paths, per-user queue limit, and selected output device.

> The admin password is stored in plaintext by design (LAN party convenience). Don't
> reuse a sensitive password.
