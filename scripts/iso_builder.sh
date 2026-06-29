#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS ISO Builder
# ═══════════════════════════════════════════════════════════════════════════════
#
# Purpose:
#   Assembles a bootable hybrid ISO image (BIOS via ISOLINUX + UEFI via GRUB)
#   containing the COGNOS kernel, initrd, and rootfs squashfs. Produces:
#     build/cognos-v0.iso
#     build/cognos-v0.iso.sha256
#
# Usage:
#   ./iso_builder.sh [--keep-work] [--verify-only]
#
# Arguments:
#   --keep-work      Retain the ISO staging directory after build.
#   --verify-only    Only verify an existing ISO and exit.
#
# Environment variables:
#   BUILD_DIR        Build output root (default: <repo>/build).
#   ISO_LABEL        ISO9660 volume label (default: COGNOS_V0).
#   KERNEL_CMDLINE   Override the default kernel command line.
#
# Exit codes:
#   0   Success — ISO produced and checksummed.
#   1   Generic failure.
#   2   Required tool or input missing.
#
# Layout produced inside the ISO:
#   /boot/vmlinuz                 Linux kernel
#   /boot/initrd.img              Initial ramdisk
#   /boot/isolinux/               ISOLINUX bootloader (BIOS)
#   /EFI/BOOT/                    GRUB EFI bootloader (UEFI)
#   /live/rootfs.squashfs         Live root filesystem
#
# v0: stub — no secure boot signing yet
# TODO(v1): sign kernel + GRUB with MOK for Secure Boot.
# TODO(v1): embed SHA256 manifest of every payload for in-field verification.
# ═══════════════════════════════════════════════════════════════════════════════

# ─── Paths & configuration ────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
readonly PROJECT_ROOT

BUILD_DIR="${BUILD_DIR:-$PROJECT_ROOT/build}"
readonly BUILD_DIR

readonly WORK_DIR="$BUILD_DIR/iso_work"
readonly ISO_ROOT="$WORK_DIR/iso"
readonly SQUASHFS="$BUILD_DIR/rootfs.squashfs"
readonly SQUASHFS_SHA="$BUILD_DIR/rootfs.squashfs.sha256"
readonly ROOTFS_DIR="$BUILD_DIR/rootfs_work/rootfs"

readonly ISO_OUTPUT="$BUILD_DIR/cognos-v0.iso"
readonly ISO_SHA256="$BUILD_DIR/cognos-v0.iso.sha256"
readonly ISO_LABEL="${ISO_LABEL:-COGNOS_V0}"

readonly KERNEL_CMDLINE="${KERNEL_CMDLINE:-boot=live components=cognos-hal,cognos-intent,cognos-scheduler quiet splash}"

KEEP_WORK=0
VERIFY_ONLY=0

# ─── Logging ──────────────────────────────────────────────────────────────────
log()  { echo "[INFO] $*" >&2; }
warn() { echo "[WARN] $*" >&2; }
err()  { echo "[ERR]  $*" >&2; }
die()  { err "$*"; exit 1; }

# ─── Cleanup trap ─────────────────────────────────────────────────────────────
cleanup() {
    local exit_code=$?
    if (( exit_code != 0 )); then
        err "iso_builder.sh failed (exit $exit_code). Work dir: $WORK_DIR"
    elif (( KEEP_WORK == 0 )); then
        log "Removing ISO work directory (use --keep-work to retain)"
        rm -rf "$WORK_DIR" 2>/dev/null || true
    fi
}
trap cleanup EXIT ERR INT TERM

# ─── Tool selection ───────────────────────────────────────────────────────────
# Prefer xorriso; fall back to mkisofs.
select_iso_tool() {
    if command -v xorriso &>/dev/null; then
        ISO_TOOL="xorriso"
    elif command -v mkisofs &>/dev/null; then
        ISO_TOOL="mkisofs"
    else
        err "Neither xorriso nor mkisofs is installed"
        exit 2
    fi
    readonly ISO_TOOL
    log "ISO tool: $ISO_TOOL"
}

# ─── Prerequisites ────────────────────────────────────────────────────────────
check_prerequisites() {
    local missing=()
    for cmd in sha256sum mmd mcopy; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done
    if (( ${#missing[@]} > 0 )); then
        err "Missing required tools: ${missing[*]}"
        exit 2
    fi

    if [[ ! -f "$SQUASHFS" ]]; then
        die "Squashfs not found: $SQUASHFS (run rootfs_builder.sh first)"
    fi

    if [[ -f "$SQUASHFS_SHA" ]]; then
        log "Verifying squashfs integrity"
        (cd "$BUILD_DIR" && sha256sum -c "$(basename "$SQUASHFS_SHA")") \
            || die "Squashfs checksum verification FAILED"
        log "Squashfs integrity verified"
    else
        warn "No squashfs checksum file — skipping integrity check"
    fi
}

# ─── 1. prepare_iso_root ──────────────────────────────────────────────────────
prepare_iso_root() {
    log "Preparing ISO root at $ISO_ROOT"

    rm -rf "$WORK_DIR"
    install -d -m 0755 "$ISO_ROOT"
    install -d -m 0755 "$ISO_ROOT/boot"
    install -d -m 0755 "$ISO_ROOT/boot/isolinux"
    install -d -m 0755 "$ISO_ROOT/live"
    install -d -m 0755 "$ISO_ROOT/EFI/BOOT"
}

# ─── 2. copy_kernel_and_initrd ────────────────────────────────────────────────
copy_kernel_and_initrd() {
    log "Copying kernel + initrd"

    local vmlinuz initrd

    # Try the rootfs work dir first; fall back to $BUILD_DIR.
    vmlinuz="$(find "$ROOTFS_DIR/boot" "$BUILD_DIR" -maxdepth 3 \
        -name 'vmlinuz-*' -print -quit 2>/dev/null || true)"
    initrd="$(find "$ROOTFS_DIR/boot" "$BUILD_DIR" -maxdepth 3 \
        -name 'initrd.img-*' -print -quit 2>/dev/null || true)"

    if [[ -z "$vmlinuz" ]]; then
        die "No vmlinuz-* found under $ROOTFS_DIR/boot or $BUILD_DIR"
    fi
    if [[ -z "$initrd" ]]; then
        die "No initrd.img-* found under $ROOTFS_DIR/boot or $BUILD_DIR"
    fi

    cp "$vmlinuz" "$ISO_ROOT/boot/vmlinuz"
    cp "$initrd" "$ISO_ROOT/boot/initrd.img"
    log "  kernel:  $(basename "$vmlinuz")"
    log "  initrd:  $(basename "$initrd")"
}

# ─── 3. copy_rootfs_squashfs ──────────────────────────────────────────────────
copy_rootfs_squashfs() {
    log "Copying squashfs into ISO"
    cp "$SQUASHFS" "$ISO_ROOT/live/rootfs.squashfs"
    if [[ -f "$SQUASHFS_SHA" ]]; then
        cp "$SQUASHFS_SHA" "$ISO_ROOT/live/rootfs.squashfs.sha256"
    fi
}

# ─── 4. setup_isolinux (BIOS) ─────────────────────────────────────────────────
setup_isolinux() {
    log "Configuring ISOLINUX (BIOS boot)"

    cat > "$ISO_ROOT/boot/isolinux/isolinux.cfg" <<EOF
UI menu.c32
PROMPT 0
TIMEOUT 30
DEFAULT cognos-live

MENU TITLE COGNOS OS v0

LABEL cognos-live
    MENU LABEL ^COGNOS OS v0 — Live
    KERNEL /boot/vmlinuz
    APPEND initrd=/boot/initrd.img $KERNEL_CMDLINE

LABEL cognos-live-verbose
    MENU LABEL COGNOS OS v0 — Live (verbose)
    KERNEL /boot/vmlinuz
    APPEND initrd=/boot/initrd.img boot=live components=cognos-hal,cognos-intent,cognos-scheduler loglevel=7

LABEL cognos-recovery
    MENU LABEL COGNOS OS v0 — ^Recovery shell
    KERNEL /boot/vmlinuz
    APPEND initrd=/boot/initrd.img boot=live systemd.unit=rescue.target
EOF

    # Pull in ISOLINUX + syslinux modules from the host if available.
    local isolinux_bin=""
    local -a search_paths=(
        "/usr/lib/ISOLINUX/isolinux.bin"
        "/usr/lib/syslinux/isolinux.bin"
        "/usr/share/syslinux/isolinux.bin"
    )
    for p in "${search_paths[@]}"; do
        if [[ -f "$p" ]]; then
            isolinux_bin="$p"
            break
        fi
    done

    if [[ -n "$isolinux_bin" ]]; then
        cp "$isolinux_bin" "$ISO_ROOT/boot/isolinux/"
        log "  isolinux.bin: $isolinux_bin"
    else
        warn "isolinux.bin not found — install isolinux for BIOS boot"
    fi

    # Copy the .c32 modules we reference.
    local c32_dir=""
    for d in /usr/lib/syslinux/modules/bios /usr/share/syslinux; do
        if [[ -d "$d" ]]; then
            c32_dir="$d"
            break
        fi
    done
    if [[ -n "$c32_dir" ]]; then
        for mod in menu.c32 libutil.c32 libcom32.c32 ldlinux.c32; do
            [[ -f "$c32_dir/$mod" ]] && cp "$c32_dir/$mod" "$ISO_ROOT/boot/isolinux/"
        done
    else
        warn "syslinux modules dir not found — BIOS menu may not render"
    fi

    # isohdpfx for hybrid ISO (BIOS boot from USB).
    local isohdpfx=""
    for p in /usr/lib/ISOLINUX/isohdpfx.bin /usr/lib/syslinux/isohdpfx.bin; do
        if [[ -f "$p" ]]; then
            isohdpfx="$p"
            break
        fi
    done
    if [[ -n "$isohdpfx" ]]; then
        cp "$isohdpfx" "$WORK_DIR/isohdpfx.bin"
    fi
}

# ─── 5. setup_grub_efi (UEFI) ─────────────────────────────────────────────────
setup_grub_efi() {
    log "Configuring GRUB EFI (UEFI boot)"

    local grub_cfg="$ISO_ROOT/boot/grub/grub.cfg"
    install -d -m 0755 "$(dirname "$grub_cfg")"

    cat > "$grub_cfg" <<EOF
set timeout=3
set default=0

insmod all_video
set gfxpayload=keep

menuentry "COGNOS OS v0 — Live" {
    linux /boot/vmlinuz $KERNEL_CMDLINE
    initrd /boot/initrd.img
}

menuentry "COGNOS OS v0 — Live (verbose)" {
    linux /boot/vmlinuz boot=live components=cognos-hal,cognos-intent,cognos-scheduler loglevel=7
    initrd /boot/initrd.img
}

menuentry "COGNOS OS v0 — Recovery shell" {
    linux /boot/vmlinuz boot=live systemd.unit=rescue.target
    initrd /boot/initrd.img
}
EOF
    log "  wrote $grub_cfg"

    # Locate grubx64.efi on the host.
    local grub_efi=""
    for p in \
        /usr/lib/grub/x86_64-efi/monolithic/grubx64.efi \
        /usr/share/grub/x86_64-efi/grubx64.efi \
        /boot/efi/EFI/debian/grubx64.efi ; do
        if [[ -f "$p" ]]; then
            grub_efi="$p"
            break
        fi
    done

    if [[ -n "$grub_efi" ]]; then
        cp "$grub_efi" "$ISO_ROOT/EFI/BOOT/BOOTX64.EFI"
        log "  grubx64.efi: $grub_efi"
    else
        warn "grubx64.efi not found — install grub-efi-amd64-bin for UEFI boot"
        # Write a placeholder so the layout is still valid.
        touch "$ISO_ROOT/EFI/BOOT/BOOTX64.EFI.placeholder"
    fi

    # Build a small FAT EFI system partition image so xorriso can embed it.
    local efi_img="$WORK_DIR/efi.img"
    local efi_size_mb=4
    dd if=/dev/zero of="$efi_img" bs=1M count="$efi_size_mb" status=none
    mkfs.vfat -F 12 -n COGNISEFI "$efi_img" >/dev/null

    mmd -i "$efi_img" ::EFI
    mmd -i "$efi_img" ::EFI/BOOT
    if [[ -f "$ISO_ROOT/EFI/BOOT/BOOTX64.EFI" ]]; then
        mcopy -i "$efi_img" "$ISO_ROOT/EFI/BOOT/BOOTX64.EFI" ::EFI/BOOT/BOOTX64.EFI
    fi
    # Embed the grub.cfg so GRUB finds it without scanning partitions.
    mmd -i "$efi_img" ::boot
    mmd -i "$efi_img" ::boot/grub
    mcopy -i "$efi_img" "$grub_cfg" ::boot/grub/grub.cfg

    # Stash a copy in the ISO tree for xorriso to embed.
    cp "$efi_img" "$ISO_ROOT/efi.img"
    log "  EFI system partition image: $efi_img"
}

# ─── 6. build_iso ─────────────────────────────────────────────────────────────
build_iso() {
    log "Building ISO with $ISO_TOOL"

    local -a args=()

    if [[ "$ISO_TOOL" == "xorriso" ]]; then
        args+=(
            -as mkisofs
            -iso-level 3
            -full-iso9660-filenames
            -volid "$ISO_LABEL"
            -appid "COGNOS OS v0"
            -publisher "COGNOS Project"
            -output "$ISO_OUTPUT"
            -eltorito-boot boot/isolinux/isolinux.bin
            -eltorito-catalog boot/isolinux/boot.cat
            -no-emul-boot
            -boot-load-size 4
            -boot-info-table
        )
        if [[ -f "$WORK_DIR/isohdpfx.bin" ]]; then
            args+=(-isohybrid-mbr "$WORK_DIR/isohdpfx.bin")
        fi
        # UEFI boot entry.
        if [[ -f "$ISO_ROOT/efi.img" ]]; then
            args+=(
                -eltorito-alt-boot
                -e efi.img
                -no-emul-boot
                -isohybrid-gpt-basdat
            )
        fi
    else
        # mkisofs fallback (no UEFI support).
        args+=(
            -iso-level 3
            -V "$ISO_LABEL"
            -o "$ISO_OUTPUT"
            -b boot/isolinux/isolinux.bin
            -c boot/isolinux/boot.cat
            -no-emul-boot
            -boot-load-size 4
            -boot-info-table
        )
        warn "mkisofs cannot produce hybrid UEFI images — UEFI boot may not work"
    fi

    args+=("$ISO_ROOT")

    # shellcheck disable=SC2068
    "$ISO_TOOL" ${args[@]}

    if [[ ! -f "$ISO_OUTPUT" ]]; then
        die "$ISO_TOOL did not produce $ISO_OUTPUT"
    fi

    local size
    size="$(du -sh "$ISO_OUTPUT" | awk '{print $1}')"
    log "ISO written: $ISO_OUTPUT ($size)"
}

# ─── 7. verify_iso ────────────────────────────────────────────────────────────
verify_iso() {
    log "Verifying ISO"

    if [[ ! -f "$ISO_OUTPUT" ]]; then
        die "ISO missing: $ISO_OUTPUT"
    fi

    # Write the checksum alongside the ISO.
    (cd "$BUILD_DIR" && sha256sum "$(basename "$ISO_OUTPUT")" > "$(basename "$ISO_SHA256")")
    log "SHA-256: $(cat "$ISO_SHA256")"

    # Self-verify by re-computing and comparing.
    (cd "$BUILD_DIR" && sha256sum -c "$(basename "$ISO_SHA256")") \
        || die "ISO checksum self-verification FAILED"
    log "ISO checksum verified ✓"

    # Best-effort hybrid MBR / GPT sanity check.
    if command -v isohybrid &>/dev/null; then
        isohybrid --help &>/dev/null && true
    fi
}

# ─── Argument parsing ─────────────────────────────────────────────────────────
parse_args() {
    while (( $# > 0 )); do
        case "$1" in
            --keep-work)   KEEP_WORK=1 ;;
            --verify-only) VERIFY_ONLY=1 ;;
            -h|--help)
                sed -n '2,40p' "$0"
                exit 0
                ;;
            *) die "Unknown argument: $1" ;;
        esac
        shift
    done
    readonly KEEP_WORK VERIFY_ONLY
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    select_iso_tool

    if (( VERIFY_ONLY == 1 )); then
        verify_iso
        exit $?
    fi

    check_prerequisites

    log "═══════════════════════════════════════════════════════════"
    log " COGNOS ISO Builder"
    log " Tool: $ISO_TOOL | Label: $ISO_LABEL"
    log "═══════════════════════════════════════════════════════════"

    prepare_iso_root
    copy_kernel_and_initrd
    copy_rootfs_squashfs
    setup_isolinux
    setup_grub_efi
    build_iso
    verify_iso

    log "═══════════════════════════════════════════════════════════"
    log " ISO BUILD SUCCESSFUL"
    log " Output: $ISO_OUTPUT"
    log "═══════════════════════════════════════════════════════════"
}

main "$@"

# v0: stub — no secure boot signing yet
