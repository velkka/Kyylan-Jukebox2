#!/bin/sh
# Installs the deb on this machine (a CI runner, which runs systemd and has no sound card),
# checks the service works, and removes and purges it again.
#
#   packaging/linux/smoke-test.sh <kyylan-jukebox.deb> [v0.2.15 Electron .deb to upgrade from]
set -eu

deb=$(realpath "$1")
electron=${2:+$(realpath "$2")}
repo=$(cd "$(dirname "$0")/../.." && pwd)

wait_for() {
    url=$1
    for _ in $(seq 60); do
        if curl -fsS "$url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    echo "no answer from $url" >&2
    sudo journalctl -u kyylan-jukebox --no-pager | tail -50 >&2
    return 1
}

step() { printf '\n== %s\n' "$*"; }

# `! command` never fails a `set -e` script, so checks that something is gone use this.
fails() {
    if "$@"; then
        echo "expected to fail: $*" >&2
        exit 1
    fi
}

if [ -n "$electron" ]; then
    step "v0.2.15, the Electron app"
    sudo apt-get install -y "$electron"
    test -x "/opt/Kyylan Jukebox/kyylan-jukebox"
    test -e /usr/bin/kyylan-jukebox
fi

step "Install"
sudo apt-get install -y "$deb"
systemctl is-enabled kyylan-jukebox
wait_for http://127.0.0.1:8080/api/config
systemctl is-active kyylan-jukebox
test "$(readlink /usr/bin/kyylan-jukebox)" = /usr/lib/kyylan-jukebox/kyylan-jukebox
kyylan-jukebox --version
if [ -n "$electron" ]; then
    test ! -e "/opt/Kyylan Jukebox"
fi
id kyylan-jukebox | grep -q '(audio)'

step "Set up at install: a generated password, and no setup page"
sudo test "$(sudo stat -c %U:%a /var/lib/kyylan-jukebox/config.json)" = kyylan-jukebox:640
password=$(sudo sed -n 's/.*"adminPassword": "\(.*\)".*/\1/p' /etc/kyylan-jukebox/config.json)
test -n "$password"
curl -fsS -H 'content-type: application/json' -d "{\"password\":\"$password\"}" \
    http://127.0.0.1:8080/api/login
curl -fsS http://127.0.0.1:8080/ | grep -q '<div id="root"'
sudo -u kyylan-jukebox env KYYLAN_DATA_DIR=/var/lib/kyylan-jukebox kyylan-jukebox --check-config

step "Stopping cleanly"
sudo systemctl stop kyylan-jukebox
sudo journalctl -u kyylan-jukebox --no-pager | grep -q 'stopped cleanly'
sudo systemctl start kyylan-jukebox
wait_for http://127.0.0.1:8080/api/config

step "A config it won't start with isn't restarted over and over"
sudo cp /var/lib/kyylan-jukebox/config.json /tmp/config.json
echo '{"configured": true}' | sudo tee /var/lib/kyylan-jukebox/config.json >/dev/null
sudo systemctl restart kyylan-jukebox || true
sleep 8
test "$(systemctl show -p NRestarts --value kyylan-jukebox)" = 0
systemctl is-failed kyylan-jukebox
sudo cp /tmp/config.json /var/lib/kyylan-jukebox/config.json
sudo systemctl reset-failed kyylan-jukebox
sudo systemctl start kyylan-jukebox
wait_for http://127.0.0.1:8080/api/config

step "Import a v0.2.15 data directory"
old="$HOME/.config/kyylan-jukebox"
mkdir -p "$old"
cp "$repo/crates/jukebox-core/tests/fixtures/electron-v0.2.15/config.json" \
    "$repo/crates/jukebox-core/tests/fixtures/electron-v0.2.15/jukebox.db" "$old/"
sudo kyylan-jukebox import "$old"
# The imported settings use port 8094.
wait_for http://127.0.0.1:8094/api/config
test "$(curl -fsS http://127.0.0.1:8094/api/tracks | grep -o '"id":' | wc -l)" = 4
sudo test "$(sudo stat -c %U /var/lib/kyylan-jukebox/jukebox.db)" = kyylan-jukebox

step "Remove keeps the data"
sudo apt-get remove -y kyylan-jukebox
fails systemctl is-active kyylan-jukebox
test ! -e /usr/bin/kyylan-jukebox
sudo test -f /var/lib/kyylan-jukebox/jukebox.db

step "Purge deletes it"
sudo apt-get purge -y kyylan-jukebox
test ! -e /var/lib/kyylan-jukebox
test ! -e /etc/kyylan-jukebox
fails getent passwd kyylan-jukebox

step "Passed"
