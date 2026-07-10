#!/usr/bin/env bash
set -euo pipefail

# COGNOS kernel builder — downloads a pinned upstream kernel, verifies the
# defconfig + critical options, applies ANFS patches, and emits Debian packages.
#
# Reproducibility knobs:
# - version + source hash are pinned below
# - KBUILD_BUILD_TIMESTAMP is fixed unless overridden
# - olddefconfig diff is always shown (never silent)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
readonly PROJECT_ROOT

# ─── Pinned upstream source ──────────────────────────────────────────────────
readonly BASE_KERNEL_VERSION="${COGNOS_KERNEL_VERSION:-6.12.10}"
readonly BASE_KERNEL_SHA256="${COGNOS_KERNEL_SHA256:-abfecf8de3a4fe41de37ba5bb64946baa342f0e585024bd559e477b43b52d062}"
readonly KERNEL_MAJOR="${BASE_KERNEL_VERSION%%.*}"
readonly KERNEL_TARBALL="linux-${BASE_KERNEL_VERSION}.tar.xz"
readonly KERNEL_URL="https://cdn.kernel.org/pub/linux/kernel/v${KERNEL_MAJOR}.x/${KERNEL_TARBALL}"
readonly KERNEL_SOURCE_HASH="${BASE_KERNEL_SHA256}"

readonly LOCALVERSION="${COGNOS_LOCALVERSION:--cognos}"
readonly KBUILD_BUILD_TIMESTAMP="${KBUILD_BUILD_TIMESTAMP:-2025-01-17T00:00:00+0000}"
readonly KBUILD_BUILD_USER="${KBUILD_BUILD_USER:-cognos}"
readonly KBUILD_BUILD_HOST="${KBUILD_BUILD_HOST:-builder}"
readonly SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1737072000}"
readonly JOBS="${COGNOS_BUILD_JOBS:-$(nproc)}"

readonly BUILD_ROOT="${PROJECT_ROOT}/build"
readonly CACHE_DIR="${BUILD_ROOT}/cache/kernel"
readonly SRC_DIR="${CACHE_DIR}/linux-src"
readonly ARTIFACT_DIR="${BUILD_ROOT}/artifacts"
readonly EXTRACT_DIR="${CACHE_DIR}/extract"

readonly PATCH_DIR="${PROJECT_ROOT}/patches"
readonly DEFCONFIG_PRIMARY="${PROJECT_ROOT}/configs/cognos_defconfig"
readonly DEFCONFIG_FALLBACK="${PROJECT_ROOT}/kernel/config/cognos_defconfig"
readonly REQUIRED_OPTIONS_FILE="${PROJECT_ROOT}/configs/kernel_required_options.conf"
readonly CONFIG_DIFF_FILE="${ARTIFACT_DIR}/cognos_defconfig.olddefconfig.diff"
readonly CONFIG_SNAPSHOT="${ARTIFACT_DIR}/kernel.build.config"
readonly SOURCE_SHA_FILE="${ARTIFACT_DIR}/kernel-source.sha256"

log()  { echo "[INFO] $*" >&2; }
warn() { echo "[WARN] $*" >&2; }
die()  { echo "[ERR]  $*" >&2; exit 1; }

resolve_defconfig() {
    if [[ -f "${DEFCONFIG_PRIMARY}" ]]; then
        printf '%s\n' "${DEFCONFIG_PRIMARY}"
    elif [[ -f "${DEFCONFIG_FALLBACK}" ]]; then
        printf '%s\n' "${DEFCONFIG_FALLBACK}"
    else
        die "No cognos_defconfig found at ${DEFCONFIG_PRIMARY} or ${DEFCONFIG_FALLBACK}"
    fi
}

readonly DEFCONFIG="$(resolve_defconfig)"

check_prerequisites() {
    local missing=()
    for cmd in awk cmp curl diff dpkg-deb grep make patch rsync sha256sum tar xz; do
        if ! command -v "${cmd}" >/dev/null 2>&1; then
            missing+=("${cmd}")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        die "Missing required tools: ${missing[*]}"
    fi
    if [[ ! -f "${REQUIRED_OPTIONS_FILE}" ]]; then
        die "Missing critical options file: ${REQUIRED_OPTIONS_FILE}"
    fi
    if [[ ! -d "${PATCH_DIR}" ]]; then
        die "Missing patches directory: ${PATCH_DIR}"
    fi
}

download_kernel() {
    mkdir -p "${CACHE_DIR}" "${ARTIFACT_DIR}"
    local tarball="${CACHE_DIR}/${KERNEL_TARBALL}"

    if [[ ! -f "${tarball}" ]]; then
        log "Downloading ${KERNEL_TARBALL}"
        curl -fL --retry 3 -o "${tarball}" "${KERNEL_URL}"
    else
        log "Using cached tarball ${tarball}"
    fi

    echo "${KERNEL_SOURCE_HASH}  ${tarball}" | sha256sum -c - >/dev/null || \
        die "Kernel source checksum mismatch for ${tarball}"
    printf '%s  %s\n' "${KERNEL_SOURCE_HASH}" "${KERNEL_TARBALL}" > "${SOURCE_SHA_FILE}"
    log "Pinned source: ${BASE_KERNEL_VERSION} (${KERNEL_SOURCE_HASH})"
}

extract_kernel() {
    local tarball="${CACHE_DIR}/${KERNEL_TARBALL}"
    rm -rf "${EXTRACT_DIR}" "${SRC_DIR}"
    mkdir -p "${EXTRACT_DIR}" "${SRC_DIR}"
    tar -xf "${tarball}" -C "${EXTRACT_DIR}"
    local extracted="${EXTRACT_DIR}/linux-${BASE_KERNEL_VERSION}"
    [[ -d "${extracted}" ]] || die "Extracted source dir missing: ${extracted}"
    rsync -a --delete "${extracted}/" "${SRC_DIR}/"
}

verify_required_options_in_file() {
    local config_file="$1"
    local missing=()
    while IFS= read -r expected; do
        [[ -z "${expected}" || "${expected}" == \#* ]] && continue
        if ! grep -qx "${expected}" "${config_file}"; then
            missing+=("${expected}")
        fi
    done < "${REQUIRED_OPTIONS_FILE}"

    if [[ ${#missing[@]} -gt 0 ]]; then
        printf 'Missing critical kernel options in %s:\n' "${config_file}" >&2
        printf '  - %s\n' "${missing[@]}" >&2
        return 1
    fi
}

verify_defconfig_inputs() {
    log "Verifying defconfig critical options"
    verify_required_options_in_file "${DEFCONFIG}" || \
        die "cognos_defconfig is missing required options"
}

apply_patches() {
    shopt -s nullglob
    local patch_files=("${PATCH_DIR}"/*.patch)
    shopt -u nullglob
    if [[ ${#patch_files[@]} -eq 0 ]]; then
        die "No ANFS patches found in ${PATCH_DIR}"
    fi

    local patch_file
    for patch_file in "${patch_files[@]}"; do
        log "Applying patch $(basename "${patch_file}")"
        (cd "${SRC_DIR}" && patch -p1 < "${patch_file}")
    done
}

configure_kernel() {
    log "Applying defconfig ${DEFCONFIG}"
    cp "${DEFCONFIG}" "${SRC_DIR}/.config"
    cp "${DEFCONFIG}" "${SRC_DIR}/.config.before_olddefconfig"

    log "Running olddefconfig"
    (
        cd "${SRC_DIR}" && \
        make olddefconfig
    )

    diff -u "${SRC_DIR}/.config.before_olddefconfig" "${SRC_DIR}/.config" > "${CONFIG_DIFF_FILE}" || true
    if [[ -s "${CONFIG_DIFF_FILE}" ]]; then
        warn "olddefconfig changed the config; diff saved to ${CONFIG_DIFF_FILE}"
        sed -n '1,200p' "${CONFIG_DIFF_FILE}" >&2
    else
        log "olddefconfig preserved all pinned values"
        rm -f "${CONFIG_DIFF_FILE}"
    fi

    verify_required_options_in_file "${SRC_DIR}/.config" || \
        die "Resolved .config is missing required options after olddefconfig"
    cp "${SRC_DIR}/.config" "${CONFIG_SNAPSHOT}"
}

build_kernel() {
    log "Building Debian kernel packages via bindeb-pkg"
    (
        cd "${SRC_DIR}" && \
        env \
            KBUILD_BUILD_TIMESTAMP="${KBUILD_BUILD_TIMESTAMP}" \
            KBUILD_BUILD_USER="${KBUILD_BUILD_USER}" \
            KBUILD_BUILD_HOST="${KBUILD_BUILD_HOST}" \
            SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH}" \
            make -j"${JOBS}" \
                LOCALVERSION="${LOCALVERSION}" \
                KDEB_PKGVERSION="${BASE_KERNEL_VERSION}-cognos1" \
                bindeb-pkg
    )
}

collect_artifacts() {
    mkdir -p "${ARTIFACT_DIR}"
    shopt -s nullglob
    local candidates=("${CACHE_DIR}"/*.deb "${BUILD_ROOT}"/*.deb)
    shopt -u nullglob

    local moved=0
    local deb
    for deb in "${candidates[@]}"; do
        [[ -f "${deb}" ]] || continue
        mv -f "${deb}" "${ARTIFACT_DIR}/"
        moved=$((moved + 1))
    done

    if [[ ${moved} -eq 0 ]]; then
        die "bindeb-pkg did not produce any .deb packages"
    fi

    (cd "${ARTIFACT_DIR}" && sha256sum ./*.deb > SHA256SUMS)
    log "Collected ${moved} .deb package(s) in ${ARTIFACT_DIR}"
}

run_post_build_verifier() {
    log "Running post-build verification on produced packages"
    "${SCRIPT_DIR}/verify_kernel_package.sh" "${ARTIFACT_DIR}"
}

main() {
    log "Kernel base version: ${BASE_KERNEL_VERSION}"
    log "Kernel source sha256: ${KERNEL_SOURCE_HASH}"

    check_prerequisites
    verify_defconfig_inputs
    download_kernel
    extract_kernel
    apply_patches
    configure_kernel
    build_kernel
    collect_artifacts
    run_post_build_verifier

    log "Kernel .deb artifacts ready in ${ARTIFACT_DIR}"
}

main "$@"
