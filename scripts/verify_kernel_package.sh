#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
readonly PROJECT_ROOT

readonly REQUIRED_OPTIONS_FILE="${PROJECT_ROOT}/configs/kernel_required_options.conf"
readonly ARTIFACT_DIR_DEFAULT="${PROJECT_ROOT}/build/artifacts"

TARGET="${1:-${ARTIFACT_DIR_DEFAULT}}"

log()  { echo "[INFO] $*" >&2; }
die()  { echo "[ERR]  $*" >&2; exit 1; }

check_prerequisites() {
    local missing=()
    for cmd in dpkg-deb grep mktemp; do
        if ! command -v "${cmd}" >/dev/null 2>&1; then
            missing+=("${cmd}")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        die "Missing required tools: ${missing[*]}"
    fi
    [[ -f "${REQUIRED_OPTIONS_FILE}" ]] || die "Missing ${REQUIRED_OPTIONS_FILE}"
}

resolve_image_deb() {
    if [[ -f "${TARGET}" ]]; then
        printf '%s\n' "${TARGET}"
        return 0
    fi

    shopt -s nullglob
    local images=("${TARGET}"/linux-image-*.deb)
    shopt -u nullglob
    [[ ${#images[@]} -gt 0 ]] || die "No linux-image-*.deb found in ${TARGET}"
    printf '%s\n' "${images[0]}"
}

extract_config_from_deb() {
    local deb="$1"
    local temp_dir
    temp_dir="$(mktemp -d)"
    trap 'rm -rf "${temp_dir}"' EXIT

    dpkg-deb -x "${deb}" "${temp_dir}"

    shopt -s nullglob
    local configs=("${temp_dir}"/boot/config-*)
    shopt -u nullglob
    [[ ${#configs[@]} -gt 0 ]] || die "No /boot/config-* found inside ${deb}"

    printf '%s\n' "${configs[0]}"
}

verify_options() {
    local config_file="$1"
    local missing=()
    while IFS= read -r expected; do
        [[ -z "${expected}" || "${expected}" == \#* ]] && continue
        if ! grep -qx "${expected}" "${config_file}"; then
            missing+=("${expected}")
        fi
    done < "${REQUIRED_OPTIONS_FILE}"

    if [[ ${#missing[@]} -gt 0 ]]; then
        printf 'Produced package is missing critical kernel options:\n' >&2
        printf '  - %s\n' "${missing[@]}" >&2
        return 1
    fi

    log "Kernel package config verified: ${config_file}"
}

main() {
    check_prerequisites
    local image_deb
    image_deb="$(resolve_image_deb)"
    log "Inspecting ${image_deb}"
    local config_file
    config_file="$(extract_config_from_deb "${image_deb}")"
    verify_options "${config_file}"
}

main "$@"
