#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly PROJECT_ROOT
BUILD_DIR="${BUILD_DIR:-${PROJECT_ROOT}/build}"
readonly BUILD_DIR

ISO_PATH="${ISO_PATH:-${BUILD_DIR}/cognos.iso}"
MODE="live"
TIMEOUT_SEC="${TIMEOUT_SEC:-420}"
QEMU_MEM="${QEMU_MEM:-8192}"
QEMU_CPUS="${QEMU_CPUS:-4}"
QEMU_BIN="${QEMU_BIN:-qemu-system-x86_64}"
WORK_DIR="${BUILD_DIR}/qemu_iso_test"

log() {
    echo "[INFO] $*" >&2
}

die() {
    echo "[ERR]  $*" >&2
    exit 1
}

parse_args() {
    while (( $# > 0 )); do
        case "$1" in
            --no-model) MODE="no-model" ;;
            -h|--help)
                sed -n '1,120p' "$0"
                exit 0
                ;;
            *)
                die "Unknown argument: $1"
                ;;
        esac
        shift
    done
}

check_prerequisites() {
    local missing=()
    for cmd in "$QEMU_BIN" timeout mkfs.vfat mcopy; do
        command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
    done
    (( ${#missing[@]} == 0 )) || die "Missing required tools: ${missing[*]}"
    [[ -f "$ISO_PATH" ]] || die "ISO not found: $ISO_PATH"
}

prepare_workdir() {
    rm -rf "$WORK_DIR"
    mkdir -p "$WORK_DIR"
}

create_config_drive() {
    local cfg_img="$WORK_DIR/cognoscfg.img"
    local cfg_env="$WORK_DIR/e2e.env"

    cat > "$cfg_env" <<EOF
COGNOS_E2E_MODE=${MODE}
COGNOS_E2E_INTENT=crée un dossier test dans /tmp
COGNOS_E2E_TARGET=/tmp/test
COGNOS_E2E_TIMEOUT_SEC=180
COGNOS_EXTRA_PATHS=/tmp
EOF

    truncate -s 4M "$cfg_img"
    mkfs.vfat -n COGNOSCFG "$cfg_img" >/dev/null
    mcopy -oi "$cfg_img" "$cfg_env" ::e2e.env >/dev/null
    echo "$cfg_img"
}

run_qemu() {
    local cfg_img="$1"
    local serial_log="$WORK_DIR/serial-${MODE}.log"
    local qemu_rc=0
    local cpu_model="max"
    local -a qemu_args
    if [[ -e /dev/kvm && -w /dev/kvm ]]; then
        cpu_model="host"
        qemu_args=(-enable-kvm)
    else
        qemu_args=()
    fi
    qemu_args+=(
        -m "$QEMU_MEM"
        -smp "$QEMU_CPUS"
        -cdrom "$ISO_PATH"
        -drive "if=virtio,format=raw,file=$cfg_img"
        -boot d
        -machine accel=kvm:tcg
        -cpu "$cpu_model"
        -nic user,model=virtio-net-pci
        -display none
        -serial "file:$serial_log"
        -monitor none
        -no-reboot
    )

    log "Booting ISO in QEMU (mode=${MODE})"
    set +e
    timeout --foreground "${TIMEOUT_SEC}s" "$QEMU_BIN" "${qemu_args[@]}"
    qemu_rc=$?
    set -e

    if (( qemu_rc != 0 )); then
        die "QEMU exited with status ${qemu_rc}; inspect ${serial_log}"
    fi

    [[ -f "$serial_log" ]] || die "Missing serial log: $serial_log"
    grep -q "COGNOS_E2E_RESULT=PASS mode=${MODE}" "$serial_log" \
        || die "Guest did not report PASS in ${serial_log}"
    grep -q "HAL:" "$serial_log" \
        || die "Serial log missing HAL decision output in ${serial_log}"

    log "QEMU test passed (${MODE})"
}

main() {
    parse_args "$@"
    check_prerequisites
    prepare_workdir
    local cfg_img
    cfg_img="$(create_config_drive)"
    run_qemu "$cfg_img"
}

main "$@"
