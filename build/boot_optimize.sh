#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS Boot Optimization & Security Configuration
# Configures systemd-boot with hardened kernel cmdline parameters.
# Runs INSIDE or AGAINST a rootfs directory.
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR

# ─── Configuration ────────────────────────────────────────────────────────────
readonly ROOTFS="${1:-${SCRIPT_DIR}/rootfs_work/rootfs}"
readonly ESP_DIR="${ROOTFS}/boot/efi"
readonly LOADER_DIR="${ESP_DIR}/loader"
readonly ENTRIES_DIR="${LOADER_DIR}/entries"

# Hardened kernel command line — Zero Trust parameters
readonly CMDLINE_SECURITY=(
    "lockdown=integrity"
    "init_on_alloc=1"
    "init_on_free=1"
    "slab_nomerge"
    "randomize_kstack_offset=on"
    "vsyscall=none"
    "debugfs=off"
    "mitigations=auto,nosmt"
    "page_alloc.shuffle=1"
    "pti=on"
    "kfence.sample_interval=100"
    "module.sig_enforce=1"
)

readonly CMDLINE_BOOT=(
    "quiet"
    "loglevel=3"
    "systemd.show_status=auto"
    "rd.systemd.show_status=auto"
    "vt.global_cursor_default=0"
)

readonly CMDLINE_ROOT=(
    "root=/dev/sda2"
    "rootfstype=ext4"
    "ro"
)

# ─── Logging ──────────────────────────────────────────────────────────────────
log() { echo "[$(date '+%H:%M:%S')] [INFO] $*"; }
warn() { echo "[$(date '+%H:%M:%S')] [WARN] $*" >&2; }
die() { echo "[$(date '+%H:%M:%S')] [ERROR] $*" >&2; exit 1; }

# ─── Cleanup ──────────────────────────────────────────────────────────────────
cleanup() {
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        warn "Boot configuration failed (exit $exit_code)"
    fi
}
trap cleanup EXIT ERR INT TERM

# ─── Prerequisites ────────────────────────────────────────────────────────────
check_prerequisites() {
    if [[ ! -d "$ROOTFS" ]]; then
        die "Rootfs directory not found: $ROOTFS"
    fi

    if [[ ! -d "$ROOTFS/boot" ]]; then
        die "No /boot directory in rootfs"
    fi

    log "Target rootfs: $ROOTFS"
}

# ─── Detect Kernel ────────────────────────────────────────────────────────────
detect_kernel() {
    local vmlinuz
    vmlinuz=$(find "$ROOTFS/boot" -name 'vmlinuz-*' -print -quit 2>/dev/null || true)

    if [[ -z "$vmlinuz" ]]; then
        warn "No vmlinuz found in $ROOTFS/boot — using placeholder"
        KERNEL_VERSION="6.12.10-cognos"
    else
        KERNEL_VERSION=$(basename "$vmlinuz" | sed 's/vmlinuz-//')
        log "Detected kernel version: $KERNEL_VERSION"
    fi
}

# ─── Configure systemd-boot ──────────────────────────────────────────────────
setup_systemd_boot() {
    log "Setting up systemd-boot..."

    mkdir -p "$LOADER_DIR"
    mkdir -p "$ENTRIES_DIR"
    mkdir -p "${ESP_DIR}/EFI/Linux"

    # loader.conf — main bootloader configuration
    cat > "$LOADER_DIR/loader.conf" <<EOF
default cognos.conf
timeout 3
console-mode max
editor no
EOF

    # Build full command line
    local cmdline=""
    cmdline="${CMDLINE_ROOT[*]} ${CMDLINE_SECURITY[*]} ${CMDLINE_BOOT[*]}"

    # Boot entry for COGNOS
    cat > "$ENTRIES_DIR/cognos.conf" <<EOF
title   COGNOS OS
linux   /vmlinuz-${KERNEL_VERSION}
initrd  /initrd.img-${KERNEL_VERSION}
options ${cmdline}
EOF

    # Fallback entry (recovery mode — still hardened, but verbose)
    cat > "$ENTRIES_DIR/cognos-recovery.conf" <<EOF
title   COGNOS OS (Recovery)
linux   /vmlinuz-${KERNEL_VERSION}
initrd  /initrd.img-${KERNEL_VERSION}
options root=/dev/sda2 rootfstype=ext4 ro lockdown=integrity module.sig_enforce=1 systemd.unit=rescue.target
EOF

    log "systemd-boot entries created"
}

# ─── Copy Kernel/Initrd to ESP ────────────────────────────────────────────────
copy_boot_files() {
    log "Copying kernel and initramfs to ESP..."

    if [[ -f "$ROOTFS/boot/vmlinuz-${KERNEL_VERSION}" ]]; then
        cp "$ROOTFS/boot/vmlinuz-${KERNEL_VERSION}" "$ESP_DIR/"
    else
        warn "vmlinuz-${KERNEL_VERSION} not found — ESP will be incomplete"
    fi

    if [[ -f "$ROOTFS/boot/initrd.img-${KERNEL_VERSION}" ]]; then
        cp "$ROOTFS/boot/initrd.img-${KERNEL_VERSION}" "$ESP_DIR/"
    else
        warn "initrd.img-${KERNEL_VERSION} not found — ESP will be incomplete"
    fi
}

# ─── Optimize Boot Services ───────────────────────────────────────────────────
optimize_services() {
    log "Optimizing systemd boot targets..."

    # Disable unnecessary services for faster boot
    local disable_services=(
        "apt-daily.timer"
        "apt-daily-upgrade.timer"
        "man-db.timer"
        "e2scrub_all.timer"
    )

    for svc in "${disable_services[@]}"; do
        if [[ -f "$ROOTFS/lib/systemd/system/$svc" ]]; then
            chroot "$ROOTFS" systemctl disable "$svc" 2>/dev/null || true
        fi
    done

    # Enable critical services
    local enable_services=(
        "systemd-networkd.service"
        "systemd-resolved.service"
        "apparmor.service"
    )

    for svc in "${enable_services[@]}"; do
        chroot "$ROOTFS" systemctl enable "$svc" 2>/dev/null || true
    done

    log "Service optimization complete"
}

# ─── Initramfs Hardening ──────────────────────────────────────────────────────
harden_initramfs() {
    log "Configuring initramfs security..."

    mkdir -p "$ROOTFS/etc/initramfs-tools"

    # Minimal initramfs — only include required modules
    cat > "$ROOTFS/etc/initramfs-tools/initramfs.conf" <<EOF
MODULES=dep
BUSYBOX=n
COMPRESS=zstd
COMPRESSLEVEL=19
RESUME=none
EOF

    # Regenerate initramfs if tools are available
    if [[ -x "$ROOTFS/usr/sbin/update-initramfs" ]]; then
        log "Regenerating initramfs..."
        chroot "$ROOTFS" update-initramfs -u -k all 2>/dev/null || \
            warn "initramfs regeneration failed (may need kernel installed first)"
    fi
}

# ─── Secure Boot Preparation ──────────────────────────────────────────────────
prepare_secure_boot() {
    log "Preparing Secure Boot infrastructure..."

    mkdir -p "$ROOTFS/etc/cognos/keys"

    # Create a placeholder for MOK key generation instructions
    cat > "$ROOTFS/etc/cognos/keys/README" <<EOF
COGNOS Secure Boot Keys
========================

To enable Secure Boot for this installation:

1. Generate a Machine Owner Key (MOK):
   openssl req -new -x509 -newkey rsa:2048 -keyout MOK.key -out MOK.crt \
     -nodes -days 3650 -subj "/CN=COGNOS Secure Boot/"

2. Convert to DER format:
   openssl x509 -in MOK.crt -out MOK.der -outform DER

3. Enroll the key:
   mokutil --import MOK.der

4. Sign the kernel:
   sbsign --key MOK.key --cert MOK.crt --output /boot/efi/vmlinuz-signed vmlinuz

5. Reboot and complete MOK enrollment at the UEFI prompt.

Note: In production, these keys should be managed via a Hardware Security Module (HSM)
or a secure key management service, never stored on the filesystem in plaintext.
EOF

    log "Secure Boot preparation complete (keys must be generated separately)"
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    log "═══════════════════════════════════════════════════════════"
    log " COGNOS Boot Optimization"
    log "═══════════════════════════════════════════════════════════"

    check_prerequisites
    detect_kernel
    setup_systemd_boot
    copy_boot_files
    optimize_services
    harden_initramfs
    prepare_secure_boot

    log "═══════════════════════════════════════════════════════════"
    log " BOOT CONFIGURATION COMPLETE"
    log " Kernel cmdline: ${CMDLINE_ROOT[*]} ${CMDLINE_SECURITY[*]} ${CMDLINE_BOOT[*]}"
    log "═══════════════════════════════════════════════════════════"
}

main "$@"
