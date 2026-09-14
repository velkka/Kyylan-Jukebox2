# Releasing

The version lives in `Cargo.toml` (`[workspace.package] version`). `/api/config` reports it,
and a version tag must match it, or the release workflow stops. Each release is a commit
named `Release X.Y.Z`, which the changelog step uses to find the previous release.

## A release

```bash
# 1. Bump [workspace.package] version in Cargo.toml, then refresh the lockfile:
cargo update --workspace
git commit -am "Release 0.3.1"

# 2. Tag and push:
git tag v0.3.1
git push origin main v0.3.1
```

The workflow builds the MSI, the pkg and both debs. It installs and uninstalls each on its
runner, then publishes the release with a changelog of the commits since the last release.

## The v0.3.0 cutover

v0.2.x was the Electron app, built from the same repository. v0.3.0 replaces it, and the
Electron build stays buildable on a branch, so a v0.2.x release is one tag away.

1. **Keep Electron releasable.** Branch from the last Electron release, and push the branch:

   ```bash
   git branch electron-0.2 v0.2.15
   git push origin electron-0.2
   ```

   Its own `release.yml` is the Electron one, so tags on that branch build the Electron
   packages.

2. **Beta.** Install the packages from a green release workflow run on real machines, with a
   backup of each install's data taken first. The run's artifacts come down with
   `gh run download <run-id>`. Install over v0.2.x on:
   - a Windows PC
   - an Apple Silicon Mac
   - an Intel Mac

   Then install on a Linux machine and a Raspberry Pi, and import each one's data. Reboot
   every machine and check it comes back playing, then run a real session.

3. **Merge and release.** `main` hasn't moved since v0.2.15, so `rust-port` fast-forwards it:

   ```bash
   git checkout main
   git merge --ff-only rust-port
   git commit --allow-empty -m "Release 0.3.0"
   git tag v0.3.0
   git push origin main v0.3.0
   ```

## Rolling back to v0.2.x

Release from the Electron branch: bump `package.json` to the next 0.2.x version, commit it as
`Release 0.2.16`, tag `v0.2.16` on that branch and push the tag.

v0.3.0 uses the same database schema and settings file as v0.2.15, so v0.2.x opens what it
leaves. The output device may need choosing again. On each machine:

- **Windows.** Uninstall Kyylan Jukebox in Settings → Apps, then run the v0.2.x
  `setup.exe`. The data in `%APPDATA%\kyylan-jukebox` is used as it is.
- **macOS.** Run `sudo kyylan-jukebox uninstall`, then install the v0.2.x `.dmg`. The data in
  `~/Library/Application Support/kyylan-jukebox` is used as it is.
- **Linux.** The service kept its data in `/var/lib`. Stop it, copy the data back to the user
  who'll run the app, then install the older package:

  ```bash
  sudo systemctl stop kyylan-jukebox
  mkdir -p ~/.config/kyylan-jukebox
  sudo cp /var/lib/kyylan-jukebox/config.json /var/lib/kyylan-jukebox/jukebox.db ~/.config/kyylan-jukebox/
  sudo chown "$USER": ~/.config/kyylan-jukebox/*
  sudo apt remove kyylan-jukebox
  sudo apt install --allow-downgrades ./Kyylan-Jukebox-0.2.16-amd64.deb
  ```

  Stopping the service first leaves the database checkpointed, so `jukebox.db` alone is
  complete.
