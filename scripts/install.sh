#!/bin/sh
# Installs (or updates) tha from a GitHub release, for this user only:
#
#   curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sh -s -- service
#   curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --key 'ssh-ed25519 AAAA...'
#
# Three modes:
#   standalone (default)  only the tha binary; metrics are sampled while the UI is open
#   service               also installs and starts the per-user background collector
#                         (launchd/systemd, no sudo), so History keeps 24 hours of metrics
#   agent                 for a server managed by Tha elsewhere: only tha-agent, owned by root
#                         in /usr/local/bin, with a persistent systemd service. Linux only, needs root.
#                         --key 'PUBLIC_KEY' also sets it up with a dedicated user, tha, or the
#                         one named by --user. --user root sets it up for an existing root key.
#                         Without these it only installs or updates; existing setup is kept.
#                         --allow-delete lets that server delete containers, images and volumes.
#                         Setup enables automatically verified Grype scans; --no-security opts out.
#
#   THA_MODE=service      same as passing `service`
#   THA_VERSION=v0.6.0    install that release instead of the latest
#   THA_BIN_DIR=...       where the command goes (default ~/.local/bin, or /usr/local/bin for agent)
#   MODULES=security      optional comma-separated modules (currently: security)
#   THA_INSTALL_LOG_DIR=...  debug log directory (default ~/.local/state/tha/install)
#
# Updating stops a running collector before the binary is replaced and starts it
# again afterwards, in either mode. Configuration and history are left alone.
set -eu

REPO=achainarong/tha
requested=${THA_MODE:-}
case ${1:-} in
    '' | -*) ;;
    *) requested=$1; shift ;;
esac
MODE=${requested:-standalone}
VERSION=${THA_VERSION:-latest}
MODULES=${MODULES:-}

say() { printf 'tha: %s\n' "$*"; }
die() { printf 'tha: %s\n' "$*" >&2; exit 1; }

if [ "$MODE" = agent ]; then
    # Checked first: the forced SSH command runs this binary, so only root may be able to
    # replace it, and sudo's HOME must not end up with root-owned files in a user's directory.
    [ "$(id -u)" = 0 ] || die "agent mode installs a root-owned binary; run it with: | sudo sh -s -- agent"
    BIN_DIR=${THA_BIN_DIR:-/usr/local/bin}
    log_dir=${THA_INSTALL_LOG_DIR:-/var/log/tha-agent}
    agent_key=
    agent_user=
    agent_delete=
    agent_scanner_digest=
    agent_security_interval=
    agent_no_security=
    while [ $# -gt 0 ]; do
        case $1 in
            --key) [ $# -ge 2 ] || die "--key needs the public key line"; agent_key=$2; shift 2 ;;
            --user) [ $# -ge 2 ] || die "--user needs a name"; agent_user=$2; shift 2 ;;
            --allow-delete) agent_delete=1; shift ;;
            --scanner-digest) [ $# -ge 2 ] || die "--scanner-digest needs sha256:DIGEST"; agent_scanner_digest=$2; shift 2 ;;
            --security-interval) [ $# -ge 2 ] || die "--security-interval needs seconds"; agent_security_interval=$2; shift 2 ;;
            --no-security) agent_no_security=1; shift ;;
            *) die "unknown agent option '$1'; expected --key, --user, --allow-delete, --no-security or --security-interval" ;;
        esac
    done
    if [ "$agent_user" = root ]; then
        [ -z "$agent_key" ] || die "--user root keeps root's own keys, so --key does not apply"
    elif [ -n "$agent_user" ] && [ -z "$agent_key" ]; then
        die "--user $agent_user needs --key with the public key for that user"
    elif [ -z "$agent_key" ] && [ -n "$agent_delete" ]; then
        die "--allow-delete needs --key or --user root"
    fi
    if [ -n "$agent_scanner_digest$agent_security_interval$agent_no_security" ] && [ -z "$agent_key$agent_user" ]; then
        die "security options need --key or --user root"
    fi
else
    [ $# -eq 0 ] || die "only agent mode takes options, not '$1'"
    BIN_DIR=${THA_BIN_DIR:-$HOME/.local/bin}
    log_dir=${THA_INSTALL_LOG_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/tha/install}
fi
mkdir -p "$log_dir" || die "couldn't create installer log directory: $log_dir"
log_file=$(mktemp "$log_dir/install-$(date -u +%Y%m%dT%H%M%SZ).XXXXXX") ||
    die "couldn't create installer log in $log_dir"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/tha-install.XXXXXX") || die "couldn't create installer workspace; log: $log_file"
stage=initialization
# A FIFO keeps subprocess output live (including module setup), without losing
# command failures to a pipeline or requiring bash/process substitution.
mkfifo "$tmp/output" "$tmp/errors" || die "couldn't create installer output pipes; log: $log_file"
exec 3>&1 4>&2
tee -a "$log_file" < "$tmp/output" &
logger_pid=$!
tee -a "$log_file" < "$tmp/errors" >&2 &
logger_err_pid=$!
exec > "$tmp/output" 2> "$tmp/errors"
finish() {
    status=$?
    trap - EXIT HUP INT TERM
    if [ "$status" = 0 ]; then
        say "installation complete; debug log: $log_file"
    else
        say "installation failed during $stage (exit $status); debug log: $log_file" >&2
    fi
    exec 1>&3 2>&4 3>&- 4>&-
    wait "$logger_pid" || printf 'tha: warning: could not finish writing debug log: %s\n' "$log_file" >&2
    wait "$logger_err_pid" || printf 'tha: warning: could not finish writing debug log: %s\n' "$log_file" >&2
    rm -rf "$tmp"
    exit "$status"
}
trap finish EXIT
trap 'exit 1' HUP INT TERM
debug() { printf '[%s] DEBUG %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >> "$log_file"; }
step() { stage=$*; debug "stage: $stage"; say "$stage"; }
run() {
    debug "running: $*"
    if "$@"; then
        debug "command completed (exit 0)"
    else
        result=$?
        debug "command failed (exit $result)"
        return "$result"
    fi
}
say "debug log: $log_file"
step "validating installation options"
debug "mode=$MODE version=$VERSION bin_dir=$BIN_DIR modules=$MODULES"

case $MODE in
    standalone | service) ;;
    agent) [ -z "$MODULES" ] || die "agent mode installs only tha-agent; MODULES do not apply" ;;
    *) die "mode must be standalone, service or agent, not '$MODE'" ;;
esac

# Validate before downloading or replacing anything. Repeated names are harmless.
case $MODULES in
    '') ;;
    ,* | *, | *,,*) die "MODULES must be a comma-separated list without empty names" ;;
esac
old_ifs=$IFS
IFS=,
set -f
for module in $MODULES; do
    case $module in
        security) ;;
        *) die "unknown module '$module'; available modules: security" ;;
    esac
done
set +f
IFS=$old_ifs

case ,$MODULES, in
    *,security,*)
        if [ "$MODE" = standalone ]; then
            if [ "$requested" = standalone ]; then
                die "security is a service module; use service mode with MODULES=security"
            fi
            MODE=service
            say "security requires service mode; installing the per-user service"
        fi
        ;;
esac

if [ "$VERSION" = latest ]; then
    base=https://github.com/$REPO/releases/latest/download
else
    case $VERSION in v*) ;; *) VERSION=v$VERSION ;; esac
    base=https://github.com/$REPO/releases/download/$VERSION
fi

step "detecting platform"
platform=$(uname -s)-$(uname -m)
debug "platform=$platform"
case $platform in
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
        run curl --proto '=https' --tlsv1.2 -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        run wget -q -O "$2" "$1"
    else
        die "needs curl or wget"
    fi || die "couldn't download $1"
}

command_name=tha
if [ "$MODE" = agent ]; then
    case $asset in
        tha-linux-*) asset=tha-agent-${asset#tha-} ;;
        *) die "agent mode supports Linux servers only" ;;
    esac
    command_name=tha-agent
fi

step "downloading $asset ($VERSION)"
fetch "$base/$asset" "$tmp/$asset"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"

step "verifying release checksum"
awk -v asset="$asset" '$2 == asset { print }' "$tmp/SHA256SUMS" > "$tmp/CHECKSUM"
[ "$(wc -l < "$tmp/CHECKSUM" | tr -d '[:space:]')" = 1 ] ||
    die "missing or duplicate checksum for $asset; nothing installed"
(
    cd "$tmp"
    if command -v sha256sum >/dev/null 2>&1; then
        run sha256sum -c CHECKSUM
    else
        run shasum -a 256 -c CHECKSUM
    fi
) || die "checksum mismatch for $asset; nothing installed"
step "unpacking release"
run tar -xzf "$tmp/$asset" -C "$tmp" "$command_name" || die "couldn't unpack $asset; nothing installed"

bin=$BIN_DIR/$command_name
if [ "$MODE" = agent ]; then
    step "installing tha-agent to $BIN_DIR"
    if command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet tha-agent.service; then
        run systemctl stop tha-agent.service
    fi
    run mkdir -p "$BIN_DIR"
    run install -m 755 "$tmp/tha-agent" "$bin"
    say "installed $(run "$bin" --version) to $bin"
    if [ -z "$agent_key$agent_user" ]; then
        if [ -r /etc/tha/agent.toml ]; then
            run "$bin" install-service
            say "existing setup kept; agent service restarted"
            if ! grep -q '^service_uid' /etc/tha/agent.toml; then
                say "legacy policy: rerun setup with --key and --user to enable dedicated-user service access"
            fi
        else
            say "not ready yet: this server still needs setup. Run this again with one of:"
            say "  dedicated user (recommended): --key 'PUBLIC_KEY'"
            say "  existing root key:            --user root"
        fi
        exit 0
    fi
    step "setting up tha-agent"
    set -- setup --user "${agent_user:-tha}"
    [ -z "$agent_key" ] || set -- "$@" --key "$agent_key"
    [ -z "$agent_delete" ] || set -- "$@" --allow-delete
    [ -z "$agent_scanner_digest" ] || set -- "$@" --scanner-digest "$agent_scanner_digest"
    [ -z "$agent_security_interval" ] || set -- "$@" --security-interval "$agent_security_interval"
    [ -z "$agent_no_security" ] || set -- "$@" --no-security
    run "$bin" "$@" < /dev/null ||
        die "tha-agent was installed, but setup failed; fix the problem above and run this again"
    exit 0
fi
running=0
# Never replace the executable under a running collector.
debug "checking existing collector status at $bin"
if [ -x "$bin" ] && "$bin" service status 2>> "$log_file" | grep -q ': running$'; then
    step "stopping the background collector for the update"
    run "$bin" service stop
    running=1
fi
step "installing binary to $BIN_DIR"
run mkdir -p "$BIN_DIR"
run install -m 755 "$tmp/tha" "$bin"
installed_version=$(run "$bin" --version)
say "installed $installed_version to $bin"

if [ "$MODE" = service ]; then
    step "installing background collector service"
    run "$bin" service install
    step "starting background collector"
    run "$bin" service start
    say "background collector running; check it with: tha service status"
elif [ "$running" = 1 ]; then
    step "restarting background collector"
    run "$bin" service start
    say "background collector restarted; remove it with: tha service uninstall"
fi

case ,$MODULES, in
    *,security,*)
        step "installing security module (Grype image + vulnerability database; requires a running engine)"
        run "$bin" module install security ||
            die "tha was installed, but security setup failed; retry: tha module install security"
        ;;
esac

case :$PATH: in
    *:"$BIN_DIR":*) ;;
    *) say "$BIN_DIR isn't on your PATH; add it to use the tha command in a shell" ;;
esac
