#!/bin/sh
# Installs the pkg on this Mac (a CI runner, logged in, with no sound card), checks the
# jukebox runs from its LaunchAgent on both architectures, and uninstalls it.
#
#   packaging/macos/smoke-test.sh <Kyylan-Jukebox.pkg>
set -eu

pkg=$1
app="/Applications/Kyylan Jukebox.app"
program="$app/Contents/MacOS/kyylan-jukebox"
label=org.kyylan.jukebox

wait_for() {
    for _ in $(seq 60); do
        if curl -fsS "$1" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    echo "no answer from $1" >&2
    launchctl print "gui/$(id -u)/$label" >&2 || true
    tail -50 "$HOME"/Library/Logs/kyylan-jukebox/*.log >&2 || true
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

step "The package"
if pkgutil --payload-files "$pkg" | grep '/\._'; then
    echo "extended attributes leaked into the payload" >&2
    exit 1
fi

step "Install"
sudo installer -pkg "$pkg" -target /
pkgutil --pkg-info "$label"
test "$(stat -f %Su:%Lp /Library/LaunchAgents/$label.plist)" = root:644
codesign --verify --strict "$app"
lipo -archs "$program" | grep -q arm64
lipo -archs "$program" | grep -q x86_64
"$program" --version
# The Intel half, under Rosetta.
arch -x86_64 "$program" --version
test "$(readlink /usr/local/bin/kyylan-jukebox)" = "$program"

step "Started by launchd for the logged-in user"
echo "console user: $(stat -f %Su /dev/console)"
wait_for http://127.0.0.1:8080/api/config
launchctl print "gui/$(id -u)/$label" | grep -q 'state = running'
curl -fsS http://127.0.0.1:8080/ | grep -q '<div id="root"'

step "A crash is restarted"
pid=$(pgrep -f "$program")
kill -KILL "$pid"
sleep 3
wait_for http://127.0.0.1:8080/api/config
test "$(pgrep -f "$program")" != "$pid"

step "Installing again replaces it"
sudo installer -pkg "$pkg" -target /
wait_for http://127.0.0.1:8080/api/config

step "Uninstall keeps the data"
sudo kyylan-jukebox uninstall
sleep 2
grep -q 'stopped cleanly' "$HOME"/Library/Logs/kyylan-jukebox/*.log
fails pgrep -f "$program"
test ! -e "$app"
test ! -e /Library/LaunchAgents/$label.plist
test ! -e /usr/local/bin/kyylan-jukebox
fails pkgutil --pkg-info "$label" 2>/dev/null
test -f "$HOME/Library/Application Support/kyylan-jukebox/jukebox.db"

step "Passed"
