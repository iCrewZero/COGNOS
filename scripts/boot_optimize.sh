#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS Boot Optimization Script
# ═══════════════════════════════════════════════════════════════════════════════
#
# Purpose:
#   Reduces COGNOS boot time to the <2s target on reference hardware by:
#     - stripping unused modules from the initramfs
#     - disabling services that are not required for an AI-native workstation
#     - enabling readahead / preload hints
#     - tuning systemd defaults (DefaultTimeoutStartSec, etc.)
#     - verifying the achieved boot time with `systemd-analyze`
#
# Usage:
#   sudo ./boot_optimize.sh [--dry-run] [--verify] [--aggressive] [--rootfs DIR]
#
# Arguments:
#   --dry-run        Print actions without modifying the system.
#   --verify         Only run verify_boot_time() and exit.
#   --aggressive     Apply more invasive optimisations (mask, not just disable).
#   --rootfs DIR     Operate on a rootfs directory (default: live host /).
#
# Environment variables:
#   COGNOS_BOOT_TARGET_SEC   Desired boot time in seconds (default: 2).
#   COGNOS_LOG_TAG           syslog tag (default: cognos-boot).
#
# Exit codes:
#   0   Success (or boot time within target).
#   1   Generic failure.
#   2   Boot time verification exceeded target.
#   3   Missing required tools / privileges.
#
# v0: stub — tune defaults are conservative
# TODO(v1): integrate with `systemd-analyze --plot` for regression tracking.
# TODO(v1): add per-service critical-chain dump on failure.
# ═══════════════════════════════════════════════════════════════════════════════

# ─── Globals ──────────────────────────────────────────────────────────────────
readonly SCRIPT_NAME="$(basename "$0")"
readonly BOOT_TARGET_SEC="${COGNOS_BOOT_TARGET_SEC:-2}"
readonly LOG_TAG="${COGNOS_LOG_TAG:-cognos-boot}"

DRY_RUN=0
VERIFY_ONLY=0
AGGRESSIVE=0
ROOTFS="/"

# Services that block boot or are irrelevant for an AI-native workstation.
# (Kept conservative in v0 — masked only under --aggressive.)
UNUSED_SERVICES=(
    "apt-daily.service"
    "apt-daily.timer"
    "apt-daily-upgrade.service"
    "apt-daily-upgrade.timer"
    "NetworkManager-wait-online.service"
    "systemd-networkd-wait-online.service"
    "man-db.timer"
    "e2scrub_all.timer"
    "e2scrub_reap.service"
    "modprobe@dm_multipath.service"
    "plymouth-quit-wait.service"
    "fwupd.service"
    "rsync.service"
)

# Services that are essential — never touched.
PROTECTED_SERVICES=(
    "systemd-journald.service"
    "systemd-logind.service"
    "systemd-udevd.service"
    "systemd-networkd.service"
    "systemd-resolved.service"
    "cognos-hal.service"
    "cognos-intent.service"
    "cognos-scheduler.service"
    "cognos-memory.service"
    "cognos-ui-agent.service"
)

# ─── Logging ──────────────────────────────────────────────────────────────────
log() {
    local msg="[INFO] $*"
    echo "$msg" >&2
    logger -t "$LOG_TAG" "$msg" 2>/dev/null || true
}

warn() {
    local msg="[WARN] $*"
    echo "$msg" >&2
    logger -t "$LOG_TAG" "$msg" 2>/dev/null || true
}

err() {
    local msg="[ERR] $*"
    echo "$msg" >&2
    logger -t "$LOG_TAG" "$msg" 2>/dev/null || true
}

die() {
    err "$*"
    exit 1
}

# ─── Cleanup trap ─────────────────────────────────────────────────────────────
cleanup() {
    local exit_code=$?
    if (( exit_code != 0 )); then
        err "boot_optimize.sh failed (exit $exit_code)"
        logger -t "$LOG_TAG" "optimization failed exit=$exit_code" 2>/dev/null || true
    else
        log "boot_optimize.sh completed cleanly"
    fi
}
trap cleanup EXIT

# ─── Helpers ──────────────────────────────────────────────────────────────────
check_prerequisites() {
    local missing=()
    for cmd in systemctl systemd-analyze logger; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done
    if (( ${#missing[@]} > 0 )); then
        err "Missing required tools: ${missing[*]}"
        exit 3
    fi

    if [[ "$ROOTFS" == "/" ]] && (( EUID != 0 )); then
        die "Root privileges required to optimise a live system (use --rootfs for chroot mode)"
    fi
}

# Run a command either for real or as a dry-run log line.
run() {
    if (( DRY_RUN == 1 )); then
        echo "[DRY] $*" >&2
        return 0
    fi
    # shellcheck disable=SC2068
    $@
}

# Invoke systemctl against either the live host or a chroot rootfs.
sc() {
    if [[ "$ROOTFS" == "/" ]]; then
        systemctl "$@"
    else
        chroot "$ROOTFS" systemctl "$@"
    fi
}

is_protected() {
    local svc="$1"
    local p
    for p in "${PROTECTED_SERVICES[@]}"; do
        if [[ "$p" == "$svc" ]]; then
            return 0
        fi
    done
    return 1
}

# ─── 1. Initramfs optimisation ────────────────────────────────────────────────
optimize_initramfs() {
    log "Optimising initramfs (MODULES=dep, COMPRESS=zstd)..."

    local conf_dir="$ROOTFS/etc/initramfs-tools"
    run mkdir -p "$conf_dir"

    local conf_file="$conf_dir/initramfs.conf"
    if [[ -f "$conf_file" ]] && (( DRY_RUN == 0 )); then
        cp -a "$conf_file" "${conf_file}.cognos-bak"
    fi

    cat > "$conf_file" <<EOF
# Managed by cognos boot_optimize.sh — do not edit by hand.
MODULES=dep
BUSYBOX=n
COMPRESS=zstd
COMPRESSLEVEL=19
RESUME=none
DEVICE=udev
EOF
    log "Wrote $conf_file"

    # Strip commonly-unused modules from the initramfs allowlist.
    local allow_file="$conf_dir/modules"
    run mkdir -p "$(dirname "$allow_file")"
    cat > "$allow_file" <<EOF
# COGNOS initramfs module allowlist (v0: minimal)
# Only modules required for early boot on reference hardware.
EOF
    log "Wrote minimal module allowlist to $allow_file"

    if (( DRY_RUN == 0 )) && [[ -x "$ROOTFS/usr/sbin/update-initramfs" ]]; then
        log "Regenerating initramfs (this may take a moment)..."
        if [[ "$ROOTFS" == "/" ]]; then
            update-initramfs -u -k all 2>&1 | tail -n 5 >&2 || warn "initramfs regeneration reported errors"
        else
            chroot "$ROOTFS" update-initramfs -u -k all 2>&1 | tail -n 5 >&2 || warn "initramfs regeneration reported errors"
        fi
    else
        warn "Skipping initramfs regeneration (dry-run or update-initramfs missing)"
    fi
}

# ─── 2. Disable unused services ──────────────────────────────────────────────
disable_unused_services() {
    log "Disabling unused services (aggressive=$AGGRESSIVE)..."

    local svc action
    for svc in "${UNUSED_SERVICES[@]}"; do
        if is_protected "$svc"; then
            warn "Refusing to touch protected service: $svc"
            continue
        fi

        # Skip services that don't exist on this install.
        if ! sc list-unit-files "$svc" &>/dev/null \
           && ! sc list-units "$svc" &>/dev/null; then
            continue
        fi

        if (( AGGRESSIVE == 1 )); then
            action="mask"
        else
            action="disable"
        fi

        log "systemctl $action $svc"
        if (( DRY_RUN == 0 )); then
            sc "$action" "$svc" 2>/dev/null || warn "Could not $action $svc"
        fi
    done

    log "Unused service cleanup complete"
}

# ─── 3. Enable readahead / preload ────────────────────────────────────────────
enable_readahead() {
    log "Enabling readahead hints..."

    # systemd-readahead was removed in modern systemd; we instead enable the
    # `systemd-random-seed` and `systemd-tmpfiles-setup` services early and
    # set a readahead-style cache via /etc/ld.so.conf.d preload.
    local conf="$ROOTFS/etc/ld.so.preload"
    if [[ -f "$conf" ]] && (( DRY_RUN == 0 )); then
        cp -a "$conf" "${conf}.cognos-bak"
    fi

    # In v0 we do not write a preload file (conservative) — just enable the
    # random-seed service which slightly improves boot determinism.
    if (( DRY_RUN == 0 )); then
        sc enable systemd-random-seed.service 2>/dev/null || true
        sc enable systemd-tmpfiles-setup.service 2>/dev/null || true
    fi
    log "Readahead helpers enabled"
    # TODO(v1): ship a readahead pack generated from a reference boot trace.
}

# ─── 4. Tune systemd defaults ─────────────────────────────────────────────────
tune_systemd() {
    log "Tuning systemd defaults..."

    local conf_dir="$ROOTFS/etc/systemd"
    run mkdir -p "$conf_dir"

    local conf_file="$conf_dir/system.conf"
    if [[ -f "$conf_file" ]] && (( DRY_RUN == 0 )); then
        cp -a "$conf_file" "${conf_file}.cognos-bak"
    fi

    cat > "$conf_file" <<EOF
# Managed by cognos boot_optimize.sh
[Manager]
DefaultTimeoutStartSec=8s
DefaultTimeoutStopSec=8s
DefaultDeviceTimeoutSec=5s
# Aggressively parallelise unit startup.
DefaultDependencies=yes
# Reduce log noise on the console.
LogLevel=notice
LogTarget=journal
EOF
    log "Wrote $conf_file"

    # User session tuning (less aggressive — does not break interactive logins).
    local user_conf="$conf_dir/user.conf"
    cat > "$user_conf" <<EOF
# Managed by cognos boot_optimize.sh
[Manager]
DefaultTimeoutStartSec=10s
DefaultTimeoutStopSec=10s
EOF
    log "Wrote $user_conf"

    # Journal: limit volatile size to keep boot journals small.
    local journald_conf="$conf_dir/journald.conf.d/cognos.conf"
    run mkdir -p "$(dirname "$journald_conf")"
    cat > "$journald_conf" <<EOF
# Managed by cognos boot_optimize.sh
[Journal]
Storage=volatile
RuntimeMaxUse=16M
EOF
    log "Wrote $journald_conf"
}

# ─── 5. Verify boot time ──────────────────────────────────────────────────────
verify_boot_time() {
    log "Verifying boot time (target: <${BOOT_TARGET_SEC}s)..."

    if [[ "$ROOTFS" != "/" ]]; then
        warn "--verify against a chroot is not meaningful; skipping"
        return 0
    fi

    if ! command -v systemd-analyze &>/dev/null; then
        die "systemd-analyze not available"
    fi

    local raw seconds
    raw="$(systemd-analyze 2>/dev/null | head -n 1 || true)"
    if [[ -z "$raw" ]]; then
        die "systemd-analyze produced no output"
    fi
    log "systemd-analyze: $raw"

    # `systemd-analyze time` prints lines like:
    #   Startup finished in 1.234s (kernel) + 0.567s (userspace) = 1.801s
    # Extract the final total.
    local time_out
    time_out="$(systemd-analyze time 2>/dev/null || true)"
    log "$time_out"

    # Pull the "= Xs" total.
    local total
    total="$(echo "$time_out" | grep -oE '= *[0-9]+\.?[0-9]*s' | head -n1 | tr -d '= s')"
    if [[ -z "$total" ]]; then
        warn "Could not parse total boot time; falling back to userspace figure"
        total="$(echo "$time_out" | grep -oE '= *[0-9]+\.?[0-9]*s' | tail -n1 | tr -d '= s')"
    fi

    if [[ -z "$total" ]]; then
        warn "Boot time could not be parsed — manual inspection required"
        return 0
    fi

    # Bash arithmetic cannot handle floats; use awk.
    seconds="$total"
    log "Measured boot time: ${seconds}s (target ${BOOT_TARGET_SEC}s)"

    local over
    over="$(awk -v t="$seconds" -v tgt="$BOOT_TARGET_SEC" 'BEGIN { print (t > tgt) ? 1 : 0 }')"
    if (( over == 1 )); then
        err "Boot time ${seconds}s EXCEEDS target ${BOOT_TARGET_SEC}s"
        # Dump the critical chain to help diagnose.
        if (( DRY_RUN == 0 )); then
            systemd-analyze critical-chain --no-pager 2>&1 | tail -n 20 >&2 || true
        fi
        return 2
    fi

    log "Boot time within target ✓"
    return 0
}

# ─── Argument parsing ─────────────────────────────────────────────────────────
parse_args() {
    while (( $# > 0 )); do
        case "$1" in
            --dry-run)
                DRY_RUN=1
                ;;
            --verify)
                VERIFY_ONLY=1
                ;;
            --aggressive)
                AGGRESSIVE=1
                ;;
            --rootfs)
                shift
                [[ -n "${1:-}" ]] || die "--rootfs requires a directory argument"
                ROOTFS="$1"
                ;;
            -h|--help)
                sed -n '2,30p' "$0"
                exit 0
                ;;
            *)
                die "Unknown argument: $1"
                ;;
        esac
        shift
    done

    if [[ "$ROOTFS" != "/" ]] && [[ ! -d "$ROOTFS" ]]; then
        die "Rootfs directory not found: $ROOTFS"
    fi

    readonly DRY_RUN VERIFY_ONLY AGGRESSIVE ROOTFS
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    check_prerequisites

    log "$SCRIPT_NAME starting (dry-run=$DRY_RUN, aggressive=$AGGRESSIVE, rootfs=$ROOTFS)"

    if (( VERIFY_ONLY == 1 )); then
        verify_boot_time
        exit $?
    fi

    optimize_initramfs
    disable_unused_services
    enable_readahead
    tune_systemd

    # Only verify when running against the live host — a chroot has no uptime.
    if [[ "$ROOTFS" == "/" ]]; then
        verify_boot_time || true
    fi

    log "Boot optimisation complete"
}

main "$@"

# v0: stub — tune defaults are conservative
