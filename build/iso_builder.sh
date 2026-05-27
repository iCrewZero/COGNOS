#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS ISO Builder
# Assembles a bootable hybrid ISO (UEFI + BIOS legacy) from the rootfs squashfs.
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR
readonly PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ─── Configuration ────────────────────────────────────────────────────────────
readonly OUTPUT_DIR="${SCRIPT_DIR}/output"
readonly WORK_DIR="${SCRIPT_DIR}/iso_work"
readonly ISO_ROOT="${WORK_DIR}/iso"
readonly SQUASHFS="${OUTPUT_DIR}/cognos-rootfs.squashfs"
readonly SQUASHFS_SHA="${OUTPUT_DIR}/cognos-rootfs.squashfs.sha256"
readonly ISO_LABEL="COGNOS_OS"
readonly ISO_OUTPUT="${OUTPUT_DIR}/cognos.iso"

# ─── Logging ──────────────────────────────────────────────────────────────────
log() { echo "[$(date '+%H:%M:%S')] [INFO] $*"; }
warn() { echo "[$(date '+%H:%M:%S')] [WARN] $*" >&2; }
die() { echo "[$(date '+%H:%M:%S')] [ERROR] $*" >&2; exit 1; }

# ─── Cleanup ──────────────────────────────────────────────────────────────────
cleanup() {
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        warn "ISO build failed (exit $exit_code). Work directory: $WORK_DIR"
    fi
}
trap cleanup EXIT ERR INT TERM

# ─── Prerequisites ────────────────────────────────────────────────────────────
check_prerequisites() {
    local missing=()
    for cmd in xorriso mtools sha256sum; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        die "Missing required tools: ${missing[*]}"
    fi

    if [[ ! -f "$SQUASHFS" ]]; then
        die "Squashfs image not found: $SQUASHFS (run rootfs_builder.sh first)"
    fi

    if [[ -f "$SQUASHFS_SHA" ]]; then
        log "Verifying squashfs integrity..."
        (cd "$OUTPUT_DIR" && sha256sum -c "cognos-rootfs.squashfs.sha256") || \
            die "Squashfs checksum verification FAILED — image may be corrupted"
        log "Squashfs integrity verified"
    else
        warn "No checksum file found for squashfs — skipping integrity check"
    fi
}

# ─── Create ISO Directory Structure ──────────────────────────────────────────
create_iso_structure() {
    log "Creating ISO directory structure..."

    rm -rf "$WORK_DIR"
    mkdir -p "$ISO_ROOT"/{live,boot/grub,EFI/BOOT,isolinux}

    # Copy squashfs
    cp "$SQUASHFS" "$ISO_ROOT/live/filesystem.squashfs"

    # Copy checksum
    if [[ -f "$SQUASHFS_SHA" ]]; then
        cp "$SQUASHFS_SHA" "$ISO_ROOT/live/filesystem.squashfs.sha256"
    fi

    log "ISO structure created"
}

# ─── Detect and Copy Kernel ───────────────────────────────────────────────────
setup_kernel() {
    log "Setting up kernel for ISO boot..."

    local vmlinuz initrd
    local rootfs_work="${SCRIPT_DIR}/rootfs_work/rootfs"

    # Try to find kernel in rootfs work directory or ESP
    vmlinuz=$(find "$rootfs_work/boot" -name 'vmlinuz-*' -print -quit 2>/dev/null || true)
    initrd=$(find "$rootfs_work/boot" -name 'initrd.img-*' -print -quit 2>/dev/null || true)

    if [[ -n "$vmlinuz" ]]; then
        cp "$vmlinuz" "$ISO_ROOT/live/vmlinuz"
        log "Kernel: $(basename "$vmlinuz")"
    else
        warn "No vmlinuz found — ISO will not be bootable without manual kernel addition"
        touch "$ISO_ROOT/live/vmlinuz.placeholder"
    fi

    if [[ -n "$initrd" ]]; then
        cp "$initrd" "$ISO_ROOT/live/initrd.img"
        log "Initrd: $(basename "$initrd")"
    else
        warn "No initrd found — ISO will not be bootable without manual initrd addition"
        touch "$ISO_ROOT/live/initrd.img.placeholder"
    fi
}

# ─── UEFI Boot Configuration ─────────────────────────────────────────────────
setup_uefi_boot() {
    log "Configuring UEFI boot..."

    # Hardened kernel cmdline for live boot
    local cmdline="boot=live components"
    cmdline+=" lockdown=integrity"
    cmdline+=" init_on_alloc=1 init_on_free=1"
    cmdline+=" slab_nomerge"
    cmdline+=" vsyscall=none"
    cmdline+=" debugfs=off"
    cmdline+=" mitigations=auto,nosmt"
    cmdline+=" quiet loglevel=3"

    # GRUB config for UEFI
    cat > "$ISO_ROOT/boot/grub/grub.cfg" <<EOF
set timeout=5
set default=0

menuentry "COGNOS OS — Live" {
    linux /live/vmlinuz ${cmdline}
    initrd /live/initrd.img
}

menuentry "COGNOS OS — Install" {
    linux /live/vmlinuz ${cmdline} cognos.mode=install
    initrd /live/initrd.img
}

menuentry "COGNOS OS — Recovery" {
    linux /live/vmlinuz boot=live components lockdown=integrity systemd.unit=rescue.target
    initrd /live/initrd.img
}
EOF

    # Create EFI boot image
    local efi_img="${WORK_DIR}/efi.img"
    local efi_size=4  # MB

    dd if=/dev/zero of="$efi_img" bs=1M count=$efi_size 2>/dev/null
    mkfs.vfat -F 12 "$efi_img" >/dev/null
    mmd -i "$efi_img" ::EFI
    mmd -i "$efi_img" ::EFI/BOOT

    # Copy GRUB EFI binary if available
    local grub_efi="/usr/lib/grub/x86_64-efi/monolithic/grubx64.efi"
    if [[ -f "$grub_efi" ]]; then
        mcopy -i "$efi_img" "$grub_efi" ::EFI/BOOT/BOOTX64.EFI
    elif [[ -f "/usr/share/grub/x86_64-efi/grubx64.efi" ]]; then
        mcopy -i "$efi_img" "/usr/share/grub/x86_64-efi/grubx64.efi" ::EFI/BOOT/BOOTX64.EFI
    else
        warn "GRUB EFI binary not found — creating placeholder"
        warn "Install grub-efi-amd64-bin for UEFI boot support"
    fi

    log "UEFI boot configured"
}

# ─── BIOS Boot Configuration (Legacy) ────────────────────────────────────────
setup_bios_boot() {
    log "Configuring BIOS (legacy) boot via isolinux..."

    # Isolinux config
    cat > "$ISO_ROOT/isolinux/isolinux.cfg" <<EOF
UI vesamenu.c32
PROMPT 0
TIMEOUT 50

MENU TITLE COGNOS OS Boot Menu

LABEL cognos-live
    MENU LABEL COGNOS OS — Live
    KERNEL /live/vmlinuz
    APPEND initrd=/live/initrd.img boot=live components lockdown=integrity quiet

LABEL cognos-install
    MENU LABEL COGNOS OS — Install
    KERNEL /live/vmlinuz
    APPEND initrd=/live/initrd.img boot=live components lockdown=integrity cognos.mode=install quiet

LABEL cognos-recovery
    MENU LABEL COGNOS OS — Recovery
    KERNEL /live/vmlinuz
    APPEND initrd=/live/initrd.img boot=live components lockdown=integrity systemd.unit=rescue.target
EOF

    # Copy isolinux binaries if available
    local isolinux_dir="/usr/lib/ISOLINUX"
    local syslinux_dir="/usr/lib/syslinux/modules/bios"

    if [[ -f "$isolinux_dir/isolinux.bin" ]]; then
        cp "$isolinux_dir/isolinux.bin" "$ISO_ROOT/isolinux/"
        cp "$isolinux_dir/isohdpfx.bin" "$WORK_DIR/" 2>/dev/null || true
    else
        warn "isolinux.bin not found — install isolinux package for BIOS boot"
    fi

    if [[ -d "$syslinux_dir" ]]; then
        cp "$syslinux_dir/ldlinux.c32" "$ISO_ROOT/isolinux/" 2>/dev/null || true
        cp "$syslinux_dir/vesamenu.c32" "$ISO_ROOT/isolinux/" 2>/dev/null || true
        cp "$syslinux_dir/libcom32.c32" "$ISO_ROOT/isolinux/" 2>/dev/null || true
        cp "$syslinux_dir/libutil.c32" "$ISO_ROOT/isolinux/" 2>/dev/null || true
    fi

    log "BIOS boot configured"
}

# ─── Build ISO Image ─────────────────────────────────────────────────────────
build_iso() {
    log "Building ISO image..."

    mkdir -p "$OUTPUT_DIR"
    local efi_img="${WORK_DIR}/efi.img"
    local isohdpfx="${WORK_DIR}/isohdpfx.bin"

    local xorriso_args=(
        -as mkisofs
        -iso-level 3
        -full-iso9660-filenames
        -volid "$ISO_LABEL"
        -output "$ISO_OUTPUT"
    )

    # UEFI support
    if [[ -f "$efi_img" ]]; then
        xorriso_args+=(
            -eltorito-alt-boot
            -e "$(basename "$efi_img")"
            -no-emul-boot
            -isohybrid-gpt-basdat
        )
        cp "$efi_img" "$ISO_ROOT/"
    fi

    # BIOS support
    if [[ -f "$ISO_ROOT/isolinux/isolinux.bin" ]]; then
        xorriso_args+=(
            -eltorito-boot isolinux/isolinux.bin
            -eltorito-catalog isolinux/boot.cat
            -no-emul-boot
            -boot-load-size 4
            -boot-info-table
        )
        if [[ -f "$isohdpfx" ]]; then
            xorriso_args+=(-isohybrid-mbr "$isohdpfx")
        fi
    fi

    xorriso_args+=("$ISO_ROOT")

    xorriso "${xorriso_args[@]}"

    if [[ ! -f "$ISO_OUTPUT" ]]; then
        die "xorriso did not produce output file"
    fi

    local iso_size
    iso_size=$(du -sh "$ISO_OUTPUT" | awk '{print $1}')
    log "ISO image created: $ISO_OUTPUT ($iso_size)"
}

# ─── Generate Checksums & Sign ────────────────────────────────────────────────
finalize() {
    log "Generating checksums..."
    (cd "$OUTPUT_DIR" && sha256sum "cognos.iso" > "cognos.iso.sha256")
    log "SHA-256: $(cat "$OUTPUT_DIR/cognos.iso.sha256")"

    # GPG signing (optional — only if key is available)
    if command -v gpg &>/dev/null && gpg --list-secret-keys 2>/dev/null | grep -q sec; then
        log "Signing ISO with GPG..."
        gpg --detach-sign --armor "$ISO_OUTPUT"
        log "GPG signature: ${ISO_OUTPUT}.asc"
    else
        warn "No GPG secret key available — ISO not signed"
        warn "For production releases, sign with: gpg --detach-sign --armor $ISO_OUTPUT"
    fi
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    log "═══════════════════════════════════════════════════════════"
    log " COGNOS ISO Builder"
    log "═══════════════════════════════════════════════════════════"

    check_prerequisites
    create_iso_structure
    setup_kernel
    setup_uefi_boot
    setup_bios_boot
    build_iso
    finalize

    log "═══════════════════════════════════════════════════════════"
    log " ISO BUILD SUCCESSFUL"
    log " Output: $ISO_OUTPUT"
    log "═══════════════════════════════════════════════════════════"
}

main "$@"
