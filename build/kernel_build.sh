#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS Kernel Build Script
# Compiles a hardened Linux kernel with COGNOS patches, produces .deb packages.
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR
readonly PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ─── Configuration ────────────────────────────────────────────────────────────
readonly KERNEL_VERSION="${COGNOS_KERNEL_VERSION:-6.12.10}"
readonly KERNEL_MAJOR="${KERNEL_VERSION%%.*}"
readonly KERNEL_URL="https://cdn.kernel.org/pub/linux/kernel/v${KERNEL_MAJOR}.x/linux-${KERNEL_VERSION}.tar.xz"
readonly KERNEL_SIG_URL="${KERNEL_URL}.sign"
readonly KERNEL_SHA256="${COGNOS_KERNEL_SHA256:-}"

readonly CACHE_DIR="${SCRIPT_DIR}/cache"
readonly SRC_DIR="${CACHE_DIR}/linux-src"
readonly OUTPUT_DIR="${SCRIPT_DIR}/output"
readonly DEFCONFIG="${PROJECT_ROOT}/kernel/config/cognos_defconfig"
readonly PATCHES_DIR="${PROJECT_ROOT}/kernel/config"

readonly LOCALVERSION="-cognos"
readonly JOBS="${COGNOS_BUILD_JOBS:-$(nproc)}"

# ─── Logging ──────────────────────────────────────────────────────────────────
log() { echo "[$(date '+%H:%M:%S')] [INFO] $*"; }
warn() { echo "[$(date '+%H:%M:%S')] [WARN] $*" >&2; }
die() { echo "[$(date '+%H:%M:%S')] [ERROR] $*" >&2; exit 1; }

# ─── Cleanup ──────────────────────────────────────────────────────────────────
cleanup() {
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        warn "Build failed (exit $exit_code). Partial artifacts may remain in $OUTPUT_DIR"
    fi
}
trap cleanup EXIT ERR INT TERM

# ─── Prerequisite Check ───────────────────────────────────────────────────────
check_prerequisites() {
    local missing=()
    for cmd in make gcc flex bison bc pahole cpio xz tar dpkg-deb; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        die "Missing required tools: ${missing[*]}"
    fi
    log "All build prerequisites satisfied"
}

# ─── Download & Verify Source ─────────────────────────────────────────────────
download_kernel() {
    mkdir -p "$CACHE_DIR"
    local tarball="${CACHE_DIR}/linux-${KERNEL_VERSION}.tar.xz"

    if [[ -f "$tarball" ]]; then
        log "Kernel tarball already cached: $tarball"
    else
        log "Downloading kernel ${KERNEL_VERSION}..."
        curl -fSL --retry 3 -o "$tarball" "$KERNEL_URL"
    fi

    if [[ -n "$KERNEL_SHA256" ]]; then
        log "Verifying SHA-256 checksum..."
        echo "${KERNEL_SHA256}  ${tarball}" | sha256sum -c - || \
            die "SHA-256 verification FAILED for kernel tarball"
        log "Checksum verified"
    else
        warn "COGNOS_KERNEL_SHA256 not set — skipping checksum verification"
        warn "Set COGNOS_KERNEL_SHA256 for reproducible builds"
    fi
}

# ─── Extract Source ───────────────────────────────────────────────────────────
extract_kernel() {
    local tarball="${CACHE_DIR}/linux-${KERNEL_VERSION}.tar.xz"

    if [[ -d "$SRC_DIR" ]]; then
        log "Removing stale source directory..."
        rm -rf "$SRC_DIR"
    fi

    log "Extracting kernel source..."
    mkdir -p "$SRC_DIR"
    tar -xf "$tarball" --strip-components=1 -C "$SRC_DIR"
    log "Extracted to $SRC_DIR"
}

# ─── Apply Patches ────────────────────────────────────────────────────────────
apply_patches() {
    local patch_count=0

    if [[ -f "${PATCHES_DIR}/preempt_rt.patch" ]] && [[ -s "${PATCHES_DIR}/preempt_rt.patch" ]]; then
        log "Applying PREEMPT_RT patch..."
        (cd "$SRC_DIR" && patch -p1 < "${PATCHES_DIR}/preempt_rt.patch")
        patch_count=$((patch_count + 1))
    fi

    for patch_file in "${PATCHES_DIR}"/*.patch; do
        [[ -f "$patch_file" ]] || continue
        [[ "$(basename "$patch_file")" == "preempt_rt.patch" ]] && continue
        [[ ! -s "$patch_file" ]] && continue

        log "Applying patch: $(basename "$patch_file")"
        (cd "$SRC_DIR" && patch -p1 < "$patch_file")
        patch_count=$((patch_count + 1))
    done

    log "Applied $patch_count patch(es)"
}

# ─── Configure Kernel ─────────────────────────────────────────────────────────
configure_kernel() {
    if [[ ! -f "$DEFCONFIG" ]]; then
        die "Defconfig not found: $DEFCONFIG"
    fi

    log "Copying COGNOS defconfig..."
    cp "$DEFCONFIG" "${SRC_DIR}/.config"

    log "Running olddefconfig to resolve new symbols..."
    local config_before config_after
    config_before=$(md5sum "${SRC_DIR}/.config" | awk '{print $1}')
    (cd "$SRC_DIR" && make olddefconfig)
    config_after=$(md5sum "${SRC_DIR}/.config" | awk '{print $1}')

    if [[ "$config_before" != "$config_after" ]]; then
        warn "olddefconfig modified .config — new symbols were resolved automatically"
        warn "Review changes with: diff kernel/config/cognos_defconfig $SRC_DIR/.config"
    fi
}

# ─── Build Kernel ─────────────────────────────────────────────────────────────
build_kernel() {
    log "Building kernel with ${JOBS} parallel jobs..."
    log "LOCALVERSION=${LOCALVERSION}"

    (cd "$SRC_DIR" && make -j"$JOBS" \
        LOCALVERSION="$LOCALVERSION" \
        KDEB_PKGVERSION="$(date +%Y%m%d)-1" \
        bindeb-pkg)

    log "Kernel build completed successfully"
}

# ─── Collect Artifacts ────────────────────────────────────────────────────────
collect_artifacts() {
    mkdir -p "$OUTPUT_DIR"

    log "Collecting .deb packages..."
    local deb_count=0
    for deb in "${CACHE_DIR}"/linux-*.deb; do
        [[ -f "$deb" ]] || continue
        mv "$deb" "$OUTPUT_DIR/"
        deb_count=$((deb_count + 1))
    done

    if [[ $deb_count -eq 0 ]]; then
        die "No .deb packages produced — build may have failed silently"
    fi

    log "Generating checksums..."
    (cd "$OUTPUT_DIR" && sha256sum linux-*.deb > SHA256SUMS)

    log "Collected $deb_count package(s) in $OUTPUT_DIR:"
    ls -lh "$OUTPUT_DIR"/linux-*.deb
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    log "═══════════════════════════════════════════════════════════"
    log " COGNOS Kernel Build — v${KERNEL_VERSION}${LOCALVERSION}"
    log "═══════════════════════════════════════════════════════════"

    check_prerequisites
    download_kernel
    extract_kernel
    apply_patches
    configure_kernel
    build_kernel
    collect_artifacts

    log "═══════════════════════════════════════════════════════════"
    log " BUILD SUCCESSFUL"
    log " Output: $OUTPUT_DIR"
    log "═══════════════════════════════════════════════════════════"
}

main "$@"
