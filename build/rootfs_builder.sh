#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS Root Filesystem Builder
# Builds a minimal, hardened Debian-based rootfs and produces a squashfs image.
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR
readonly PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ─── Configuration ────────────────────────────────────────────────────────────
readonly SUITE="${COGNOS_SUITE:-bookworm}"
readonly ARCH="${COGNOS_ARCH:-amd64}"
readonly MIRROR="${COGNOS_MIRROR:-http://deb.debian.org/debian}"
readonly WORK_DIR="${SCRIPT_DIR}/rootfs_work"
readonly ROOTFS_DIR="${WORK_DIR}/rootfs"
readonly OUTPUT_DIR="${SCRIPT_DIR}/output"
readonly BASE_PACKAGES="${SCRIPT_DIR}/rootfs/base_packages.txt"
readonly KERNEL_DEBS_DIR="${OUTPUT_DIR}"
readonly HOSTNAME="cognos"

export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(date +%s)}"

# ─── Logging ──────────────────────────────────────────────────────────────────
log() { echo "[$(date '+%H:%M:%S')] [INFO] $*"; }
warn() { echo "[$(date '+%H:%M:%S')] [WARN] $*" >&2; }
die() { echo "[$(date '+%H:%M:%S')] [ERROR] $*" >&2; exit 1; }

# ─── Cleanup — CRITICAL for host safety ───────────────────────────────────────
MOUNTS_ACTIVE=()

umount_all() {
    log "Unmounting bind mounts..."
    for mnt in "${MOUNTS_ACTIVE[@]:-}"; do
        if mountpoint -q "$mnt" 2>/dev/null; then
            umount -lf "$mnt" 2>/dev/null || true
        fi
    done
}

cleanup() {
    local exit_code=$?
    umount_all
    if [[ $exit_code -ne 0 ]]; then
        warn "Build failed (exit $exit_code). Work directory preserved for debugging: $WORK_DIR"
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
    if [[ ${#missing[@]} -gt 0 ]]; then
        die "Missing required tools: ${missing[*]}"
    fi

    if [[ $EUID -ne 0 ]]; then
        die "This script must run as root (required for chroot/mount operations)"
    fi

    if [[ ! -f "$BASE_PACKAGES" ]]; then
        die "Base packages list not found: $BASE_PACKAGES"
    fi
}

# ─── Bootstrap ────────────────────────────────────────────────────────────────
bootstrap_rootfs() {
    if [[ -d "$ROOTFS_DIR" ]]; then
        log "Removing stale rootfs work directory..."
        rm -rf "$ROOTFS_DIR"
    fi
    mkdir -p "$ROOTFS_DIR"

    log "Running debootstrap (suite=$SUITE, arch=$ARCH)..."
    debootstrap \
        --variant=minbase \
        --arch="$ARCH" \
        --no-install-recommends \
        --merged-usr \
        "$SUITE" \
        "$ROOTFS_DIR" \
        "$MIRROR"

    log "Bootstrap complete"
}

# ─── Mount Pseudo-filesystems ─────────────────────────────────────────────────
mount_pseudofs() {
    log "Mounting pseudo-filesystems..."
    mount --bind /proc "$ROOTFS_DIR/proc";  MOUNTS_ACTIVE+=("$ROOTFS_DIR/proc")
    mount --bind /sys "$ROOTFS_DIR/sys";    MOUNTS_ACTIVE+=("$ROOTFS_DIR/sys")
    mount --bind /dev "$ROOTFS_DIR/dev";    MOUNTS_ACTIVE+=("$ROOTFS_DIR/dev")
    mount -t devpts devpts "$ROOTFS_DIR/dev/pts"; MOUNTS_ACTIVE+=("$ROOTFS_DIR/dev/pts")
    mount -t tmpfs tmpfs "$ROOTFS_DIR/tmp"; MOUNTS_ACTIVE+=("$ROOTFS_DIR/tmp")
}

# ─── Install Packages ─────────────────────────────────────────────────────────
install_packages() {
    log "Installing packages from $BASE_PACKAGES..."

    local packages
    packages=$(grep -v '^\s*#' "$BASE_PACKAGES" | grep -v '^\s*$' | tr '\n' ' ')

    if [[ -z "$packages" ]]; then
        warn "No packages listed in base_packages.txt"
        return
    fi

    chroot "$ROOTFS_DIR" bash -c "
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y --no-install-recommends $packages
    "

    log "Package installation complete"
}

# ─── Install Kernel ───────────────────────────────────────────────────────────
install_kernel() {
    local kernel_deb
    kernel_deb=$(find "$KERNEL_DEBS_DIR" -name 'linux-image-*.deb' -print -quit 2>/dev/null || true)

    if [[ -z "$kernel_deb" ]]; then
        warn "No kernel .deb found in $KERNEL_DEBS_DIR — skipping kernel installation"
        return
    fi

    log "Installing kernel: $(basename "$kernel_deb")"
    cp "$kernel_deb" "$ROOTFS_DIR/tmp/"
    chroot "$ROOTFS_DIR" bash -c "dpkg -i /tmp/$(basename "$kernel_deb")"
    rm -f "$ROOTFS_DIR/tmp/$(basename "$kernel_deb")"

    local headers_deb
    headers_deb=$(find "$KERNEL_DEBS_DIR" -name 'linux-headers-*.deb' -print -quit 2>/dev/null || true)
    if [[ -n "$headers_deb" ]]; then
        log "Installing headers: $(basename "$headers_deb")"
        cp "$headers_deb" "$ROOTFS_DIR/tmp/"
        chroot "$ROOTFS_DIR" bash -c "dpkg -i /tmp/$(basename "$headers_deb")"
        rm -f "$ROOTFS_DIR/tmp/$(basename "$headers_deb")"
    fi
}

# ─── System Configuration ─────────────────────────────────────────────────────
configure_system() {
    log "Configuring system..."

    # Hostname
    echo "$HOSTNAME" > "$ROOTFS_DIR/etc/hostname"
    cat > "$ROOTFS_DIR/etc/hosts" <<EOF
127.0.0.1   localhost
127.0.1.1   $HOSTNAME
::1         localhost ip6-localhost ip6-loopback
EOF

    # Locale
    chroot "$ROOTFS_DIR" bash -c "
        export DEBIAN_FRONTEND=noninteractive
        if command -v locale-gen &>/dev/null; then
            echo 'en_US.UTF-8 UTF-8' > /etc/locale.gen
            locale-gen
        fi
        echo 'LANG=en_US.UTF-8' > /etc/default/locale
    "

    # Timezone
    chroot "$ROOTFS_DIR" bash -c "
        ln -sf /usr/share/zoneinfo/UTC /etc/localtime
        echo UTC > /etc/timezone
    "

    # Networking (systemd-networkd)
    mkdir -p "$ROOTFS_DIR/etc/systemd/network"
    cat > "$ROOTFS_DIR/etc/systemd/network/20-wired.network" <<EOF
[Match]
Name=en*
Name=eth*

[Network]
DHCP=yes
IPv6AcceptRA=yes
EOF

    chroot "$ROOTFS_DIR" bash -c "
        systemctl enable systemd-networkd 2>/dev/null || true
        systemctl enable systemd-resolved 2>/dev/null || true
    "

    # fstab
    cat > "$ROOTFS_DIR/etc/fstab" <<EOF
# COGNOS OS — filesystem table
# <device>    <mount>   <type>   <options>                    <dump> <pass>
/dev/root     /         ext4     defaults,noatime,errors=ro   0      1
/dev/sda1     /boot/efi vfat     defaults,umask=0077          0      2
tmpfs         /tmp      tmpfs    defaults,noexec,nosuid,nodev 0      0
EOF

    # Security: restrict /tmp, /var/tmp
    mkdir -p "$ROOTFS_DIR/etc/tmpfiles.d"
    cat > "$ROOTFS_DIR/etc/tmpfiles.d/cognos-security.conf" <<EOF
d /tmp 1777 root root 10d
d /var/tmp 1777 root root 30d
EOF

    # Disable root password login (force key-based or user escalation)
    chroot "$ROOTFS_DIR" bash -c "passwd -l root" 2>/dev/null || true

    # sysctl hardening
    mkdir -p "$ROOTFS_DIR/etc/sysctl.d"
    cat > "$ROOTFS_DIR/etc/sysctl.d/90-cognos-hardening.conf" <<EOF
kernel.kptr_restrict = 2
kernel.dmesg_restrict = 1
kernel.perf_event_paranoid = 3
kernel.yama.ptrace_scope = 2
kernel.unprivileged_bpf_disabled = 1
net.core.bpf_jit_harden = 2
kernel.kexec_load_disabled = 1
kernel.sysrq = 0
fs.protected_hardlinks = 1
fs.protected_symlinks = 1
fs.protected_fifos = 2
fs.protected_regular = 2
net.ipv4.conf.all.rp_filter = 1
net.ipv4.conf.default.rp_filter = 1
net.ipv4.conf.all.accept_redirects = 0
net.ipv4.conf.default.accept_redirects = 0
net.ipv6.conf.all.accept_redirects = 0
net.ipv4.conf.all.send_redirects = 0
net.ipv4.conf.all.accept_source_route = 0
net.ipv6.conf.all.accept_source_route = 0
net.ipv4.tcp_syncookies = 1
net.ipv4.tcp_timestamps = 0
net.ipv4.icmp_echo_ignore_broadcasts = 1
EOF

    log "System configuration complete"
}

# ─── Cleanup Inside Rootfs ────────────────────────────────────────────────────
cleanup_rootfs() {
    log "Cleaning up rootfs..."
    chroot "$ROOTFS_DIR" bash -c "
        apt-get clean
        rm -rf /var/lib/apt/lists/*
        rm -rf /var/cache/apt/archives/*.deb
        rm -rf /tmp/*
        rm -f /var/log/*.log
        rm -f /root/.bash_history
    "

    # Set proper permissions
    chmod 755 "$ROOTFS_DIR"
    chown root:root "$ROOTFS_DIR"
}

# ─── Produce Squashfs Image ──────────────────────────────────────────────────
produce_squashfs() {
    mkdir -p "$OUTPUT_DIR"
    local output_file="${OUTPUT_DIR}/cognos-rootfs.squashfs"

    if [[ -f "$output_file" ]]; then
        rm -f "$output_file"
    fi

    log "Creating squashfs image (zstd compression)..."
    mksquashfs "$ROOTFS_DIR" "$output_file" \
        -comp zstd \
        -Xcompression-level 19 \
        -no-xattrs \
        -noappend \
        -all-root \
        -quiet

    local size
    size=$(du -sh "$output_file" | awk '{print $1}')
    log "Squashfs image created: $output_file ($size)"

    log "Computing SHA-256..."
    (cd "$OUTPUT_DIR" && sha256sum "cognos-rootfs.squashfs" > "cognos-rootfs.squashfs.sha256")
    log "Checksum: $(cat "${OUTPUT_DIR}/cognos-rootfs.squashfs.sha256")"
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    log "═══════════════════════════════════════════════════════════"
    log " COGNOS Rootfs Builder"
    log " Suite: $SUITE | Arch: $ARCH | Mirror: $MIRROR"
    log "═══════════════════════════════════════════════════════════"

    check_prerequisites
    bootstrap_rootfs
    mount_pseudofs
    install_packages
    install_kernel
    configure_system
    cleanup_rootfs
    umount_all
    MOUNTS_ACTIVE=()
    produce_squashfs

    log "═══════════════════════════════════════════════════════════"
    log " ROOTFS BUILD SUCCESSFUL"
    log " Output: ${OUTPUT_DIR}/cognos-rootfs.squashfs"
    log "═══════════════════════════════════════════════════════════"
}

main "$@"
