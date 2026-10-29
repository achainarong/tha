#!/bin/sh
# Installs (or updates) tha from a GitHub release, for this user only:
#
#   curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sh -s -- service
#
# Two modes:
#   standalone (default)  only the tha binary; metrics are sampled while the UI is open
#   service               also installs and starts the per-user background collector
#                         (launchd/systemd, no sudo), so History keeps 24 hours of metrics
#
#   THA_MODE=service      same as passing `service`
#   THA_VERSION=v0.6.0    install that release instead of the latest
#   THA_BIN_DIR=...       where the tha command goes (default ~/.local/bin)
#
# Updating stops a running collector before the binary is replaced and starts it
# again afterwards, in either mode. Configuration and history are left alone.
set -eu

REPO=achainarong/tha
MODE=${1:-${THA_MODE:-standalone}}
VERSION=${THA_VERSION:-latest}
BIN_DIR=${THA_BIN_DIR:-$HOME/.local/bin}

say() { printf 'tha: %s\n' "$*"; }
die() { printf 'tha: %s\n' "$*" >&2; exit 1; }

case $MODE in
    standalone | service) ;;
    *) die "mode must be standalone or service, not '$MODE'" ;;
esac

if [ "$VERSION" = latest ]; then
    base=https://github.com/$REPO/releases/latest/download
else
    case $VERSION in v*) ;; *) VERSION=v$VERSION ;; esac
    base=https://github.com/$REPO/releases/download/$VERSION
fi

case $(uname -s)-$(uname -m) in
    Linux-x86_64 | Linux-amd64) asset=tha-linux-amd64.tar.gz ;;
    Linux-aarch64 | Linux-arm64) asset=tha-linux-arm64.tar.gz ;;
    Darwin-arm64 | Darwin-x86_64)
        # The hardware, not this shell: under Rosetta, uname -m says x86_64.
        [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ] ||
            die "the macOS release is for Apple Silicon only; build from source on Intel (see the README)"
        asset=tha-macos-arm64.tar.gz
        ;;
    *) die "no release for $(uname -s) $(uname -m); supported: Linux amd64/arm64, macOS Apple Silicon" ;;
esac

# fetch URL FILE
fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        die "needs curl or wget"
    fi || die "couldn't download $1"
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/tha-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' HUP INT TERM

say "downloading $asset ($VERSION)"
fetch "$base/$asset" "$tmp/$asset"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"

awk -v asset="$asset" '$2 == asset { print }' "$tmp/SHA256SUMS" > "$tmp/CHECKSUM"
[ "$(wc -l < "$tmp/CHECKSUM" | tr -d '[:space:]')" = 1 ] ||
    die "missing or duplicate checksum for $asset; nothing installed"
(
    cd "$tmp"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c CHECKSUM >/dev/null
    else
        shasum -a 256 -c CHECKSUM >/dev/null
    fi
) || die "checksum mismatch for $asset; nothing installed"
tar -xzf "$tmp/$asset" -C "$tmp" tha || die "couldn't unpack $asset; nothing installed"

bin=$BIN_DIR/tha
running=0
# Never replace the executable under a running collector.
if [ -x "$bin" ] && "$bin" service status 2>/dev/null | grep -q ': running$'; then
    say "stopping the background collector for the update"
    "$bin" service stop
    running=1
fi
mkdir -p "$BIN_DIR"
install -m 755 "$tmp/tha" "$bin"
say "installed $("$bin" --version) to $bin"

if [ "$MODE" = service ]; then
    "$bin" service install
    "$bin" service start
    say "background collector running; check it with: tha service status"
elif [ "$running" = 1 ]; then
    "$bin" service start
    say "background collector restarted; remove it with: tha service uninstall"
fi

case :$PATH: in
    *:"$BIN_DIR":*) ;;
    *) say "$BIN_DIR isn't on your PATH; add it to use the tha command in a shell" ;;
esac
