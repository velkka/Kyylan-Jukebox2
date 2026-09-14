# Installing Kyylan Jukebox

Each release has an installer per platform. None is code-signed yet, so each OS warns
the first time.

## Windows: `Kyylan-Jukebox-<version>-x64.msi`

- **Install.** Run the MSI; SmartScreen warns first (More info → Run anyway). It installs
  for all users into Program Files and starts the jukebox straight away.
- **Starting.** A scheduled task starts it whenever someone signs in, in that person's
  session, where the sound devices are. It lives in the tray; click the icon to open the
  console.
- **Upgrading from v0.2.x.** The installer removes the old app installed by the person
  running it. The library, settings and stats stay where they were.
- **Data.** `%APPDATA%\kyylan-jukebox`. Logs are in its `logs` folder.
- **Uninstall.** Use Settings → Apps. The data stays.
- **After a power cut.** The PC waits at the sign-in screen, and the jukebox isn't running
  until someone signs in. To have it come back by itself, set up automatic sign-in
  (Sysinternals Autologon).

## macOS: `Kyylan-Jukebox-<version>.pkg`

- **Install.** Right-click the pkg → Open, since Gatekeeper blocks unsigned installers on a
  double-click. It runs natively on Apple Silicon and Intel.
- **Starting.** A LaunchAgent starts the jukebox for whoever logs in, including straight
  after installing. It's a menu-bar app with no Dock icon. Quit in its menu stops it until
  the next log in.
- **Upgrading from v0.2.x.** The pkg replaces the old app. The library, settings and stats
  stay.
- **Data.** `~/Library/Application Support/kyylan-jukebox`.
- **Logs.** `~/Library/Logs/kyylan-jukebox`.
- **Uninstall.** Run `sudo kyylan-jukebox uninstall`. The data stays.
- **After a power cut.** The Mac waits at the login window. Automatic login (System
  Settings → Users & Groups) brings the jukebox back by itself; FileVault turns it off.

## Linux: `Kyylan-Jukebox-<version>-amd64.deb` / `-arm64.deb`

```sh
sudo apt install ./Kyylan-Jukebox-<version>-amd64.deb
```

Installing prints the console address and a generated admin password. The password is also
`adminPassword` in `/etc/kyylan-jukebox/config.json`.

The jukebox is a system service. It starts at boot with nobody logged in, runs as the user
`kyylan-jukebox`, and plays straight to the sound card.

- **Status.** `systemctl status kyylan-jukebox`
- **Logs.** `journalctl -u kyylan-jukebox`
- **Data.** `/var/lib/kyylan-jukebox`

### Music the service can read

The service is a different user from you, so your home folder and desktop-mounted USB drives
(`/media/<you>/…`) are usually closed to it. The admin panel says so when you add such a
folder. Instead, put the music somewhere it can read:

- a folder like `/srv/music`, readable by everyone, or
- a drive mounted through `/etc/fstab`, or
- a folder shared through a group:
  `sudo usermod -aG <group> kyylan-jukebox && sudo systemctl restart kyylan-jukebox`.

A library folder that isn't there when the jukebox starts, such as a drive not yet mounted,
keeps its tracks. They come back when the folder does.

### Upgrading from v0.2.x

apt replaces the old app in place. Its data was in your home folder, so bring it across once:

```sh
sudo kyylan-jukebox import ~/.config/kyylan-jukebox
```

The import stops the service, copies the settings and the database, and starts the service
again. It also warns about library folders the service can't read.

### Editing the settings by hand

Stop the service, edit `/etc/kyylan-jukebox/config.json`, check the file, then start it again:

```sh
sudo systemctl stop kyylan-jukebox
sudo kyylan-jukebox --check-config --data-dir /var/lib/kyylan-jukebox
sudo systemctl start kyylan-jukebox
```

- **Output devices.** `kyylan-jukebox --list-devices` shows the names and ids to use in
  `outputDeviceId`.
- **Admin password.** The service won't start without `adminPassword`.

### On a desktop

Dedicated machines work best: Raspberry Pi OS Lite, or Ubuntu Server. On a desktop, the
logged-in user's PipeWire holds the sound card, and a system service plays nothing through
it. The fix is to give the jukebox its own output, such as a USB DAC. Then add a WirePlumber
rule so PipeWire leaves that card alone, and set `outputDeviceId` to the card.

### Removing

- `sudo apt remove kyylan-jukebox` stops and removes it, and keeps the data.
- `sudo apt purge kyylan-jukebox` also deletes `/var/lib/kyylan-jukebox` and the service
  user.
