#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS Root Filesystem Builder
# ═══════════════════════════════════════════════════════════════════════════════
#
# Purpose:
#   Builds the COGNOS root filesystem from a minimal Debian base via debootstrap,
#   installs COGNOS Rust binaries + Python agents, configures /etc/cognos and
#   /var/lib/cognos, then squashes the result into build/rootfs.squashfs for
#   use by the ISO builder.
#
# Usage:
#   sudo ./rootfs_builder.sh [--keep-work] [--skip-bootstrap]
#
# Arguments:
#   --keep-work        Do not delete the work directory on success.
#   --skip-bootstrap   Reuse an existing rootfs work dir (skip debootstrap).
#
# Environment variables:
#   BUILD_DIR        Build output root (default: <repo>/build).
#   ARCH             Target architecture (default: amd64).
#   SUITE            Debian suite (default: bookworm).
#   COGNOS_MIRROR    Debian mirror (default: http://deb.debian.org/debian).
#   COGNOS_HOSTNAME  Hostname written into the rootfs (default: cognos).
#
# Exit codes:
#   0   Success — squashfs produced.
#   1   Generic failure.
#   2   Required tool or input missing.
#   3   Must run as root.
#
# v0: stub — packages are minimal, no signed verification
# TODO(v1): verify Rust binary signatures before copying.
# TODO(v1): support multi-arch cross-build via qemu-user-static.
# ═══════════════════════════════════════════════════════════════════════════════

# ─── Paths & configuration ────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
readonly PROJECT_ROOT

readonly ARCH="${ARCH:-amd64}"
readonly SUITE="${SUITE:-bookworm}"
readonly MIRROR="${COGNOS_MIRROR:-http://deb.debian.org/debian}"
readonly HOSTNAME_VAL="${COGNOS_HOSTNAME:-cognos}"

BUILD_DIR="${BUILD_DIR:-$PROJECT_ROOT/build}"
readonly BUILD_DIR

readonly WORK_DIR="$BUILD_DIR/rootfs_work"
readonly ROOTFS_DIR="$WORK_DIR/rootfs"
readonly BASE_PACKAGES="$BUILD_DIR/rootfs/base_packages.txt"
readonly OUTPUT_SQUASHFS="$BUILD_DIR/rootfs.squashfs"
readonly OUTPUT_SHA256="$BUILD_DIR/rootfs.squashfs.sha256"

# COGNOS Rust binaries expected under target/release/.
readonly COGNOS_BINARIES=(
    "cognos-hal"
    "cognos-intent"
    "cognos-scheduler"
    "cognos-memory"
    "cognos-orchestrator"
    "cognos-ipc-server"
    # Owner: iCrewZero — added missing binaries referenced by systemd service files
    "cognos-ai-daemon"
    "cognos-ui-agent"
)

# Python agents directory.
readonly AGENTS_SRC="$PROJECT_ROOT/agents"

# systemd unit files to install (sourced from services/).
# Try systemd/ first (authoritative), fall back to services/ (for compatibility)
readonly SERVICES_SRC="${SYSTEMD_UNITS_DIR:-$PROJECT_ROOT/services}"

KEEP_WORK=0
SKIP_BOOTSTRAP=0

# Track mounts so cleanup can unmount them even on abrupt failure.
MOUNTS_ACTIVE=()

# ─── Logging ──────────────────────────────────────────────────────────────────
log()  { echo "[INFO] $*" >&2; }
warn() { echo "[WARN] $*" >&2; }
err()  { echo "[ERR]  $*" >&2; }
die()  { err "$*"; exit 1; }

# ─── Cleanup trap (CRITICAL: must unmount bind mounts) ────────────────────────
umount_all() {
    local mnt
    # Unmount in reverse order.
    for (( i=${#MOUNTS_ACTIVE[@]}-1; i>=0; i-- )); do
        mnt="${MOUNTS_ACTIVE[$i]}"
        if mountpoint -q "$mnt" 2>/dev/null; then
            warn "Unmounting $mnt"
            umount -lf "$mnt" 2>/dev/null || true
        fi
    done
    MOUNTS_ACTIVE=()
}

cleanup() {
    local exit_code=$?
    umount_all
    if (( exit_code != 0 )); then
        err "rootfs_builder.sh failed (exit $exit_code). Work dir preserved: $WORK_DIR"
    elif (( KEEP_WORK == 0 )); then
        log "Cleaning work directory (use --keep-work to retain)"
        rm -rf "$WORK_DIR" 2>/dev/null || true
    fi
}
trap cleanup EXIT ERR INT TERM

# ─── Prerequisites ────────────────────────────────────────────────────────────
check_prerequisites() {
    local missing=()
    for cmd in debootstrap chroot mksquashfs sha256sum; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done
    if (( ${#missing[@]} > 0 )); then
        err "Missing required tools: ${missing[*]}"
        exit 2
    fi

    if (( EUID != 0 )); then
        err "This script must run as root (required for chroot/mount)."
        exit 3
    fi

    if [[ ! -f "$BASE_PACKAGES" ]]; then
        err "Base packages list not found: $BASE_PACKAGES"
        exit 2
    fi
}

# ─── Bind-mount helpers ───────────────────────────────────────────────────────
bind_mount() {
    local src="$1"
    local dst="$2"
    local type="${3:-none}"

    mkdir -p "$dst"
    if [[ "$type" == "none" ]]; then
        mount --bind "$src" "$dst"
    else
        mount -t "$type" "$src" "$dst"
    fi
    MOUNTS_ACTIVE+=("$dst")
}

# ─── 1. prepare_chroot ────────────────────────────────────────────────────────
prepare_chroot() {
    log "Preparing chroot at $ROOTFS_DIR"

    if (( SKIP_BOOTSTRAP == 1 )) && [[ -d "$ROOTFS_DIR" ]]; then
        log "Skipping debootstrap (--skip-bootstrap); reusing $ROOTFS_DIR"
    else
        if [[ -d "$ROOTFS_DIR" ]]; then
            log "Removing stale rootfs work dir"
            rm -rf "$ROOTFS_DIR"
        fi
        mkdir -p "$ROOTFS_DIR"

        log "Running debootstrap (suite=$SUITE arch=$ARCH)"
        debootstrap \
            --variant=minbase \
            --arch="$ARCH" \
            --no-install-recommends \
            --merged-usr \
            "$SUITE" \
            "$ROOTFS_DIR" \
            "$MIRROR"
    fi

    # Bind-mount pseudo-filesystems so chroot commands work.
    log "Mounting pseudo-filesystems into chroot"
    bind_mount /proc                 "$ROOTFS_DIR/proc"
    bind_mount /sys                  "$ROOTFS_DIR/sys"
    bind_mount /dev                  "$ROOTFS_DIR/dev"
    mount -t devpts devpts "$ROOTFS_DIR/dev/pts"; MOUNTS_ACTIVE+=("$ROOTFS_DIR/dev/pts")
    mount -t tmpfs  tmpfs  "$ROOTFS_DIR/tmp";      MOUNTS_ACTIVE+=("$ROOTFS_DIR/tmp")

    # /etc/resolv.conf for apt-get inside the chroot.
    if [[ -f /etc/resolv.conf ]]; then
        cp /etc/resolv.conf "$ROOTFS_DIR/etc/resolv.conf"
    fi
}

# ─── 2. install_base_packages ─────────────────────────────────────────────────
install_base_packages() {
    log "Installing base packages from $BASE_PACKAGES"

    local packages
    packages="$(grep -vE '^\s*(#|$)' "$BASE_PACKAGES" | tr '\n' ' ')"
    if [[ -z "$packages" ]]; then
        warn "No packages found in base_packages.txt"
        return 0
    fi

    chroot "$ROOTFS_DIR" bash -c "
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y --no-install-recommends $packages
        apt-get clean
        rm -rf /var/lib/apt/lists/*
    "
    log "Base packages installed"
}

# ─── 3. install_cognos_components ─────────────────────────────────────────────
install_cognos_components() {
    log "Installing COGNOS components"

    local release_dir="$PROJECT_ROOT/target/release"

    # 3a. Rust binaries → /usr/local/bin/
    install -d -m 0755 "$ROOTFS_DIR/usr/lib/cognos"
    local bin
    for bin in "${COGNOS_BINARIES[@]}"; do
        local src="$release_dir/$bin"
        if [[ ! -x "$src" ]]; then
            warn "Missing Rust binary: $src — skipping (expected if not yet built)"
            continue
        fi
        install -m 0755 "$src" "$ROOTFS_DIR/usr/lib/cognos/$bin"
        log "  installed binary: $bin"
    done

    # 3b. Python agents → /opt/cognos/agents/
    local agents_dst="$ROOTFS_DIR/opt/cognos/agents"
    if [[ -d "$AGENTS_SRC" ]]; then
        install -d -m 0755 "$agents_dst"
        cp -a "$AGENTS_SRC/." "$agents_dst/"
        log "  installed agents: $AGENTS_SRC → $agents_dst"
    else
        warn "Agents source dir not found: $AGENTS_SRC"
    fi

    # 3c. Python venv for agents.
    if [[ -d "$agents_dst" ]] && chroot "$ROOTFS_DIR" bash -c "command -v python3 &>/dev/null"; then
        log "Creating Python venv at /opt/cognos/venv"
        chroot "$ROOTFS_DIR" bash -c "
            python3 -m venv /opt/cognos/venv
            /opt/cognos/venv/bin/pip install --quiet --upgrade pip 2>/dev/null || true
            if [[ -f /opt/cognos/agents/requirements.txt ]]; then
                /opt/cognos/venv/bin/pip install --quiet -r /opt/cognos/agents/requirements.txt 2>/dev/null || true
            fi
        " || warn "venv creation failed (continuing)"
    fi

    # 3d. systemd units.
    if [[ -d "$SERVICES_SRC" ]]; then
        install -d -m 0755 "$ROOTFS_DIR/etc/systemd/system"
        local svc
        for svc in "$SERVICES_SRC"/*.service; do
            [[ -f "$svc" ]] || continue
            install -m 0644 "$svc" "$ROOTFS_DIR/etc/systemd/system/"
            log "  installed unit: $(basename "$svc")"
        done
        chroot "$ROOTFS_DIR" systemctl daemon-reload 2>/dev/null || true
    else
        warn "Services source dir not found: $SERVICES_SRC"
    fi

    # 3e. Config + state directories.
    install -d -m 0755 "$ROOTFS_DIR/etc/cognos"
    install -d -m 0755 "$ROOTFS_DIR/etc/cognos/sway.config.d"
    install -d -m 0750 "$ROOTFS_DIR/var/lib/cognos"
    install -d -m 0755 "$ROOTFS_DIR/var/log/cognos"
    install -d -m 0755 "$ROOTFS_DIR/var/run/cognos"

    # 3e-extra. Install sway config.
    if [[ -f "$PROJECT_ROOT/sway.config" ]]; then
        install -m 0644 "$PROJECT_ROOT/sway.config" "$ROOTFS_DIR/etc/cognos/sway.config"
        log "  installed sway.config"
    fi
    if [[ -d "$PROJECT_ROOT/configs" ]]; then
        for cfg in "$PROJECT_ROOT/configs"/*.conf; do
            [[ -f "$cfg" ]] || continue
            install -m 0644 "$cfg" "$ROOTFS_DIR/etc/cognos/sway.config.d/"
            log "  installed sway override: $(basename "$cfg")"
        done
    fi

    cat > "$ROOTFS_DIR/etc/cognos/version" <<EOF
COGNOS_OS_VERSION=v0
COGNOS_BUILD_DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)
COGNOS_SUITE=$SUITE
COGNOS_ARCH=$ARCH
EOF

    log "COGNOS components installed"
}

# ─── 3f. Install session script for sway ─────────────────────────────────────
# Owner: iCrewZero — the session script was referenced but never copied into the rootfs.
# This installs cognos-session.sh so the display manager can find it at /usr/local/bin.
install_session_script() {
    log "Installing cognos-session.sh"
    mkdir -p "$ROOTFS_DIR/usr/local/bin"
    cp "$PROJECT_ROOT/scripts/cognos-session.sh" "$ROOTFS_DIR/usr/local/bin/cognos-session.sh"
    chmod 755 "$ROOTFS_DIR/usr/local/bin/cognos-session.sh"
    log "  installed cognos-session.sh → /usr/local/bin/cognos-session.sh"
}

# Install security infrastructure: nftables, AppArmor, cgroup slices.
# Owner: iCrewZero
install_security_configs() {
    log "Installing security configs..."

    # nftables ruleset
    mkdir -p "$ROOTFS_DIR/etc/cognos/nftables"
    if [ -f "$PROJECT_ROOT/security/nftables/ai-isolation.nft" ]; then
        cp "$PROJECT_ROOT/security/nftables/ai-isolation.nft" \
           "$ROOTFS_DIR/etc/cognos/nftables/ai-isolation.nft"
        log "  installed nftables ruleset"
    fi

    # AppArmor profiles
    mkdir -p "$ROOTFS_DIR/etc/apparmor.d"
    for profile in "$PROJECT_ROOT"/security/apparmor/*; do
        if [ -f "$profile" ]; then
            basename=$(basename "$profile")
            cp "$profile" "$ROOTFS_DIR/etc/apparmor.d/$basename"
            log "  installed AppArmor profile: $basename"
        fi
    done

    # cgroup slice
    if [ -f "$PROJECT_ROOT/security/cgroups/cognos.slice" ]; then
        mkdir -p "$ROOTFS_DIR/etc/systemd/system"
        cp "$PROJECT_ROOT/security/cgroups/cognos.slice" \
           "$ROOTFS_DIR/etc/systemd/system/cognos.slice"
        log "  installed cognos.slice"
    fi
}

# ─── 4. install_kernel ────────────────────────────────────────────────────────
install_kernel() {
    log "Installing kernel"

    local kernel_deb
    kernel_deb="$(find "$BUILD_DIR" -maxdepth 2 -name 'linux-image-*.deb' -print -quit 2>/dev/null || true)"
    if [[ -z "$kernel_deb" ]]; then
        warn "No linux-image-*.deb found in $BUILD_DIR — installing distro kernel"
        chroot "$ROOTFS_DIR" bash -c "
            export DEBIAN_FRONTEND=noninteractive
            apt-get install -y --no-install-recommends linux-image-amd64 2>/dev/null || \
                warn 'could not install distro kernel'
        " || true
        return 0
    fi

    log "  kernel deb: $(basename "$kernel_deb")"
    cp "$kernel_deb" "$ROOTFS_DIR/tmp/"
    chroot "$ROOTFS_DIR" bash -c "
        export DEBIAN_FRONTEND=noninteractive
        dpkg -i /tmp/$(basename "$kernel_deb") 2>/dev/null || \
            apt-get install -y -f --no-install-recommends
    "
    rm -f "$ROOTFS_DIR/tmp/$(basename "$kernel_deb")"
}

# ─── 5. setup_fstab ───────────────────────────────────────────────────────────
setup_fstab() {
    log "Writing /etc/fstab"
    cat > "$ROOTFS_DIR/etc/fstab" <<EOF
# COGNOS OS — filesystem table (managed by rootfs_builder.sh)
# <device>     <mount>      <type>   <options>                      <dump> <pass>
/dev/root      /            ext4     defaults,noatime,errors=ro     0      1
tmpfs          /tmp         tmpfs    defaults,noexec,nosuid,nodev   0      0
tmpfs          /var/tmp     tmpfs    defaults,noexec,nosuid,nodev   0      0
proc           /proc        proc     defaults                       0      0
sysfs          /sys         sysfs    defaults                       0      0
EOF

    # Hostname.
    echo "$HOSTNAME_VAL" > "$ROOTFS_DIR/etc/hostname"
    cat > "$ROOTFS_DIR/etc/hosts" <<EOF
127.0.0.1   localhost
127.0.1.1   $HOSTNAME_VAL
::1         localhost ip6-localhost ip6-loopback
EOF
}

# ─── 6. setup_initramfs ───────────────────────────────────────────────────────
setup_initramfs() {
    log "Configuring initramfs"
    install -d -m 0755 "$ROOTFS_DIR/etc/initramfs-tools"

    cat > "$ROOTFS_DIR/etc/initramfs-tools/initramfs.conf" <<EOF
# COGNOS initramfs configuration
MODULES=dep
BUSYBOX=n
COMPRESS=zstd
COMPRESSLEVEL=19
RESUME=none
DEVICE=udev
EOF

    # Live-boot hooks (so the squashfs can be mounted as the rootfs).
    install -d -m 0755 "$ROOTFS_DIR/etc/initramfs-tools/scripts/live-bottom"
    cat > "$ROOTFS_DIR/etc/initramfs-tools/conf.d/cognos-live.conf" <<EOF
export COGNOS_LIVE=1
EOF

    if [[ -x "$ROOTFS_DIR/usr/sbin/update-initramfs" ]]; then
        log "Regenerating initramfs"
        chroot "$ROOTFS_DIR" update-initramfs -u -k all 2>&1 | tail -n 3 >&2 \
            || warn "initramfs regeneration reported errors"
    else
        warn "update-initramfs not available yet — will run on first boot"
    fi
}

# ─── 7. create_squashfs ───────────────────────────────────────────────────────
create_squashfs() {
    log "Creating squashfs image: $OUTPUT_SQUASHFS"

    # Make sure nothing is mounted before we squash.
    umount_all

    install -d -m 0755 "$(dirname "$OUTPUT_SQUASHFS")"
    if [[ -f "$OUTPUT_SQUASHFS" ]]; then
        rm -f "$OUTPUT_SQUASHFS"
    fi

    mksquashfs "$ROOTFS_DIR" "$OUTPUT_SQUASHFS" \
        -comp zstd \
        -Xcompression-level 19 \
        -no-xattrs \
        -noappend \
        -all-root \
        -quiet

    local size
    size="$(du -sh "$OUTPUT_SQUASHFS" | awk '{print $1}')"
    log "Squashfs image written ($size)"

    log "Computing SHA-256"
    (cd "$(dirname "$OUTPUT_SQUASHFS")" \
        && sha256sum "$(basename "$OUTPUT_SQUASHFS")" > "$OUTPUT_SHA256")
    log "Checksum: $(cat "$OUTPUT_SHA256")"
}

# ─── Argument parsing ─────────────────────────────────────────────────────────
parse_args() {
    while (( $# > 0 )); do
        case "$1" in
            --keep-work)     KEEP_WORK=1 ;;
            --skip-bootstrap) SKIP_BOOTSTRAP=1 ;;
            -h|--help)
                sed -n '2,35p' "$0"
                exit 0
                ;;
            *) die "Unknown argument: $1" ;;
        esac
        shift
    done
    readonly KEEP_WORK SKIP_BOOTSTRAP
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    check_prerequisites

    log "═══════════════════════════════════════════════════════════"
    log " COGNOS Rootfs Builder"
    log " Suite: $SUITE | Arch: $ARCH | Mirror: $MIRROR"
    log "═══════════════════════════════════════════════════════════"

    prepare_chroot
    install_base_packages
    install_cognos_components
    # Owner: iCrewZero — install session script and security configs into rootfs
    install_session_script
    install_security_configs
    install_kernel
    setup_fstab
    setup_initramfs
    create_squashfs

    log "═══════════════════════════════════════════════════════════"
    log " ROOTFS BUILD SUCCESSFUL"
    log " Output: $OUTPUT_SQUASHFS"
    log "═══════════════════════════════════════════════════════════"
}

main "$@"

# v0: stub — packages are minimal, no signed verification
