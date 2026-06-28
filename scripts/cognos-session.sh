#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS User Session Starter
# ═══════════════════════════════════════════════════════════════════════════════
#
# Purpose:
#   Launches a COGNOS user session: starts the user-scoped systemd services
#   (intent, HAL, scheduler, memory, UI agent), brings up the Wayland
#   compositor (sway), and registers the session with loginctl. Invoked by
#   the display manager (e.g. greetd, GDM, SDDM) as the user's session.
#
# Usage:
#   cognos-session.sh [--no-compositor] [--debug]
#
#   Typically launched by the display manager with:
#     Exec=cognos-session.sh
#
# Arguments:
#   --no-compositor   Start services + shell only; do not exec sway
#                     (useful for embedded/headless testing).
#   --debug           Enable verbose logging to $XDG_STATE_HOME/cognos/session.log.
#
# Environment variables expected (set by the display manager):
#   XDG_RUNTIME_DIR    Required — user runtime dir (/run/user/<uid>).
#   XDG_SESSION_ID     loginctl session id (populated if absent).
#   DBUS_SESSION_BUS_ADDRESS  Session bus.
#
# Environment variables exported by this script:
#   XDG_SESSION_TYPE=wayland
#   XDG_CURRENT_DESKTOP=cognos
#   COGNOS_SESSION_ID=<uuid>
#   COGNOS_CONFIG_DIR=/etc/cognos
#   COGNOS_STATE_DIR=${XDG_STATE_HOME:-$HOME/.local/state}/cognos
#
# Exit codes:
#   0   Clean session shutdown.
#   1   Generic failure.
#   2   Missing runtime prerequisites (XDG_RUNTIME_DIR, dbus, etc.).
#
# v0: stub — minimal session, no idle-detection yet
# TODO(v1): add idle-detection + auto-lock via swayidle.
# TODO(v1): add per-session seccomp filter before exec'ing sway.
# ═══════════════════════════════════════════════════════════════════════════════

# ─── Globals ──────────────────────────────────────────────────────────────────
readonly SCRIPT_NAME="$(basename "$0")"

DEBUG=0
NO_COMPOSITOR=0
SESSION_ID=""
SESSION_REGISTERED=0
SWAY_PID=""
LOG_FILE=""

# User-scoped COGNOS services — started in this order.
COGNOS_USER_SERVICES=(
    "cognos-memory.service"
    "cognos-hal.service"
    "cognos-scheduler.service"
    "cognos-intent.service"
    "cognos-ui-agent.service"
)

# ─── Logging ──────────────────────────────────────────────────────────────────
_log() {
    local level="$1"; shift
    local msg="$*"
    echo "[$level] $msg" >&2
    if [[ -n "$LOG_FILE" ]] && [[ -w "$(dirname "$LOG_FILE")" ]]; then
        echo "[$(date '+%H:%M:%S')] [$level] $msg" >> "$LOG_FILE" 2>/dev/null || true
    fi
}

log()  { _log INFO "$@"; }
warn() { _log WARN "$@"; }
err()  { _log ERR  "$@"; }
die()  { err "$*"; exit 1; }

debug_log() {
    if (( DEBUG == 1 )); then
        _log DBG "$@"
    fi
}

# ─── Cleanup trap ─────────────────────────────────────────────────────────────
cleanup_on_exit() {
    local exit_code=$?
    log "$SCRIPT_NAME shutting down (exit=$exit_code)"

    # Kill sway if we started it and it's still alive.
    if [[ -n "$SWAY_PID" ]] && kill -0 "$SWAY_PID" 2>/dev/null; then
        log "Stopping sway (pid=$SWAY_PID)"
        kill -TERM "$SWAY_PID" 2>/dev/null || true
        # Give it a moment, then SIGKILL if needed.
        for _ in 1 2 3 4 5; do
            kill -0 "$SWAY_PID" 2>/dev/null || break
            sleep 0.2
        done
        kill -KILL "$SWAY_PID" 2>/dev/null || true
    fi

    # Stop COGNOS user services (reverse order).
    local i
    for (( i=${#COGNOS_USER_SERVICES[@]}-1; i>=0; i-- )); do
        local svc="${COGNOS_USER_SERVICES[$i]}"
        if sudo systemctl is-active --quiet "$svc" 2>/dev/null; then
            log "Stopping $svc"
            sudo systemctl stop "$svc" 2>/dev/null || warn "Could not stop $svc"
        fi
    done

    # Terminate the session we registered.
    if (( SESSION_REGISTERED == 1 )) && [[ -n "$SESSION_ID" ]]; then
        log "Terminating loginctl session $SESSION_ID"
        loginctl terminate-session "$SESSION_ID" 2>/dev/null || true
    fi

    log "Session cleanup complete"
}
trap cleanup_on_exit EXIT
trap 'warn "Received SIGINT"; exit 130' INT
trap 'warn "Received SIGTERM"; exit 143' TERM

# ─── Prerequisites ────────────────────────────────────────────────────────────
check_prerequisites() {
    local missing=()

    for cmd in systemctl loginctl sway; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done

    if (( ${#missing[@]} > 0 )); then
        err "Missing required tools: ${missing[*]}"
        exit 2
    fi

    if [[ -z "${XDG_RUNTIME_DIR:-}" ]]; then
        err "XDG_RUNTIME_DIR is not set — refusing to start session"
        exit 2
    fi

    if [[ ! -d "$XDG_RUNTIME_DIR" ]]; then
        err "XDG_RUNTIME_DIR does not exist: $XDG_RUNTIME_DIR"
        exit 2
    fi

    # dbus session bus must be available for `sudo systemctl`.
    if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
        warn "DBUS_SESSION_BUS_ADDRESS not set; attempting to start a session bus"
        if command -v dbus-launch &>/dev/null; then
            # shellcheck disable=SC2046
            eval "$(dbus-launch --sh-syntax)"
            log "Started session bus: $DBUS_SESSION_BUS_ADDRESS"
        else
            err "dbus-launch not available — cannot start user services"
            exit 2
        fi
    fi
}

# ─── 1. setup_environment ─────────────────────────────────────────────────────
setup_environment() {
    log "Setting up COGNOS session environment"

    # Session type / desktop.
    export XDG_SESSION_TYPE="wayland"
    export XDG_CURRENT_DESKTOP="cognos"

    # COGNOS config + state dirs.
    export COGNOS_CONFIG_DIR="${COGNOS_CONFIG_DIR:-/etc/cognos}"
    local state_root="${XDG_STATE_HOME:-$HOME/.local/state}"
    export COGNOS_STATE_DIR="$state_root/cognos"
    install -d -m 0750 "$COGNOS_STATE_DIR"

    # Per-session log file.
    LOG_FILE="$COGNOS_STATE_DIR/session.log"
    install -d -m 0750 "$(dirname "$LOG_FILE")"

    # Unique session id.
    if [[ -z "${COGNOS_SESSION_ID:-}" ]]; then
        if command -v uuidgen &>/dev/null; then
            COGNOS_SESSION_ID="$(uuidgen)"
        else
            COGNOS_SESSION_ID="cognos-$$-$(date +%s)"
        fi
    fi
    export COGNOS_SESSION_ID
    readonly COGNOS_SESSION_ID
    log "COGNOS_SESSION_ID=$COGNOS_SESSION_ID"

    # Wayland-specific env that sway + clients expect.
    export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}"
    export _JAVA_AWT_WM_NONREPARENTING=1
    export QT_QPA_PLATFORM="${QT_QPA_PLATFORM:-wayland}"
    export GDK_BACKEND="${GDK_BACKEND:-wayland}"
    export SDL_VIDEODRIVER="${SDL_VIDEODRIVER:-wayland}"
    export MOZ_ENABLE_WAYLAND=1

    debug_log "env: XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR"
    debug_log "env: WAYLAND_DISPLAY=$WAYLAND_DISPLAY"
    debug_log "env: COGNOS_CONFIG_DIR=$COGNOS_CONFIG_DIR"
}

# ─── 2. start_cognos_services ─────────────────────────────────────────────────
start_cognos_services() {
    log "Starting COGNOS user services"

    # Make sure the user manager is available.
    if ! sudo systemctl is-system-running &>/dev/null \
       && ! sudo systemctl daemon-reload 2>/dev/null; then
        warn "user systemd manager not responsive; attempting start via systemd-tty..."
        /usr/lib/systemd/systemd --user &
        sleep 1
    fi

    sudo systemctl daemon-reload 2>/dev/null || true

    local svc
    for svc in "${COGNOS_USER_SERVICES[@]}"; do
        if ! sudo systemctl --quiet is-enabled "$svc" 2>/dev/null \
           && ! sudo systemctl --quiet is-active "$svc" 2>/dev/null; then
            warn "Unit $svc not installed or inactive — skipping"
            continue
        fi
        log "  starting $svc"
        if ! sudo systemctl start "$svc"; then
            err "Failed to start $svc"
            sudo systemctl status "$svc" --no-pager 2>&1 | tail -n 20 >&2 || true
            # Continue — a missing service should not abort the whole session.
        fi
    done

    log "COGNOS user services started"
}

# ─── 3. start_wayland ─────────────────────────────────────────────────────────
start_wayland() {
    if (( NO_COMPOSITOR == 1 )); then
        log "Skipping compositor (--no-compositor)"
        return 0
    fi

    log "Starting sway (Wayland compositor)"

    # sway reads $COGNOS_CONFIG_DIR/sway.config if no user config exists.
    local sway_args=(
        -d
    )
    if [[ -f "$COGNOS_CONFIG_DIR/sway.config" ]]; then
        sway_args+=(-c "$COGNOS_CONFIG_DIR/sway.config")
    elif [[ -f "$HOME/.config/sway/config" ]]; then
        sway_args+=(-c "$HOME/.config/sway/config")
    else
        warn "No sway config found — using sway defaults"
    fi

    # Launch sway in the background so we can register the session + start the
    # shell on the same Wayland socket.
    sway "${sway_args[@]}" &
    SWAY_PID=$!
    log "sway started (pid=$SWAY_PID)"

    # Wait for the Wayland socket to appear.
    local sock="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"
    local waited=0
    while [[ ! -S "$sock" ]]; do
        if ! kill -0 "$SWAY_PID" 2>/dev/null; then
            err "sway exited before opening the Wayland socket"
            exit 1
        fi
        (( waited >= 50 )) && {
            err "Timed out waiting for Wayland socket: $sock"
            exit 1
        }
        sleep 0.1
        (( waited++ ))
    done
    log "Wayland socket ready: $sock"
}

# ─── 4. start_shell ───────────────────────────────────────────────────────────
start_shell() {
    log "Starting COGNOS shell"

    # Prefer the Rust shell binary if present.
    local shell_bin=""
    for p in /usr/local/bin/cognos-shell /usr/bin/cognos-shell; do
        if [[ -x "$p" ]]; then
            shell_bin="$p"
            break
        fi
    done

    if [[ -z "$shell_bin" ]]; then
        warn "cognos-shell not installed — starting foot terminal as a stand-in"
        if command -v foot &>/dev/null; then
            foot &
            log "foot started (pid=$!)"
        else
            warn "foot not available — no shell launched"
        fi
        return 0
    fi

    # The shell connects to the Wayland display and runs until the compositor exits.
    if ! "$shell_bin"; then
        err "cognos-shell exited with an error"
    fi
}

# ─── 5. register_session ──────────────────────────────────────────────────────
register_session() {
    log "Registering session with loginctl"

    # If the display manager didn't provide one, ask loginctl.
    if [[ -z "${XDG_SESSION_ID:-}" ]]; then
        XDG_SESSION_ID="$(loginctl list-sessions --no-legend 2>/dev/null \
            | awk -v uid="$EUID" '$2 == uid {print $1; exit}')"
        if [[ -z "$XDG_SESSION_ID" ]]; then
            warn "Could not determine loginctl session id — skipping registration"
            return 0
        fi
    fi
    SESSION_ID="$XDG_SESSION_ID"
    export XDG_SESSION_ID

    # Mark the session's type + class so other services can find it.
    loginctl show-session "$SESSION_ID" --property=Type --value 2>/dev/null \
        | grep -q wayland \
        || loginctl set-session "$SESSION_ID" Type=wayland 2>/dev/null \
            || warn "Could not set session Type=wayland"

    loginctl set-session "$SESSION_ID" Class=user 2>/dev/null \
        || warn "Could not set session Class=user"

    SESSION_REGISTERED=1
    log "Session registered: id=$SESSION_ID type=wayland class=user"
}

# ─── Argument parsing ─────────────────────────────────────────────────────────
parse_args() {
    while (( $# > 0 )); do
        case "$1" in
            --no-compositor) NO_COMPOSITOR=1 ;;
            --debug)         DEBUG=1 ;;
            -h|--help)
                sed -n '2,40p' "$0"
                exit 0
                ;;
            *) die "Unknown argument: $1" ;;
        esac
        shift
    done
    readonly DEBUG NO_COMPOSITOR
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    check_prerequisites
    setup_environment

    log "═══════════════════════════════════════════════════════════"
    log " COGNOS session starting"
    log " session_id=$COGNOS_SESSION_ID"
    log "═══════════════════════════════════════════════════════════"

    start_cognos_services
    start_wayland
    register_session

    if (( NO_COMPOSITOR == 0 )); then
        start_shell

        # Block until sway exits — this is the session lifetime.
        if [[ -n "$SWAY_PID" ]]; then
            log "Waiting for sway (pid=$SWAY_PID) to exit"
            wait "$SWAY_PID" 2>/dev/null || true
            log "sway exited"
        fi
    else
        log "No compositor — keeping session alive until signal"
        # Sleep forever until a signal arrives (trap will fire).
        while true; do
            sleep 3600
        done
    fi

    log "Session main loop ended"
    exit 0
}

main "$@"

# v0: stub — minimal session, no idle-detection yet
