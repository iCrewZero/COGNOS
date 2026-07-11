#!/usr/bin/env bash
set -euo pipefail

# COGNOS rootfs builder — installs the runnable local runtime into build/rootfs/.

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
readonly WORK_DIR="${BUILD_DIR}/rootfs_work"
readonly ROOTFS_DIR="${BUILD_DIR}/rootfs"
readonly BASE_PACKAGES="${BUILD_DIR}/rootfs/base_packages.txt"
readonly OUTPUT_SQUASHFS="${BUILD_DIR}/rootfs.squashfs"
readonly OUTPUT_SHA256="${BUILD_DIR}/rootfs.squashfs.sha256"

readonly RELEASE_DIR="${PROJECT_ROOT}/target/release"
readonly AGENTS_SRC="${PROJECT_ROOT}/agents"
readonly CONFIG_SRC_DIR="${PROJECT_ROOT}/config"
readonly SYSTEMD_SRC_DIR="${PROJECT_ROOT}/systemd"
readonly SERVICES_SRC_DIR="${PROJECT_ROOT}/services"
readonly SECURITY_DIR="${PROJECT_ROOT}/security"
readonly INTENT_GRAMMAR_SRC="${PROJECT_ROOT}/intent-engine/grammar/intent.gbnf"
readonly PROTO_PYTHON="${COGNOS_PROTO_PYTHON:-${PROJECT_ROOT}/.venv/bin/python}"

readonly DEFAULT_MODEL_NAME="qwen3-7b-q4_k_m"
readonly SMALL_MODEL_NAME="qwen3.5-4b-q4_k_m"

readonly CORE_RUST_BINARIES=(
    "cognos-hal"
    "cognos-intent"
    "cognos-scheduler"
    "cognos-memory"
    "cognos-orchestrator"
)
readonly SUPPORT_RUST_BINARIES=(
    "cognos-ipc-server"
    "cognos"
)

readonly LLAMA_SERVER_BIN="${COGNOS_LLAMA_SERVER_BIN:-}"
readonly MODEL_SOURCE_DEFAULT="${COGNOS_GGUF_MODEL_SOURCE:-}"
readonly MODEL_SOURCE_SMALL="${COGNOS_GGUF_MODEL_SOURCE_SMALL:-}"

KEEP_WORK=0
SKIP_BOOTSTRAP=0
USE_SMALL_MODEL=0
MOUNTS_ACTIVE=()

log()  { echo "[INFO] $*" >&2; }
warn() { echo "[WARN] $*" >&2; }
die()  { echo "[ERR]  $*" >&2; exit 1; }

cleanup() {
    local exit_code=$?
    umount_all
    if (( exit_code != 0 )); then
        echo "[ERR]  rootfs_builder.sh failed (exit $exit_code). Rootfs preserved at $ROOTFS_DIR" >&2
    elif (( KEEP_WORK == 0 )); then
        rm -rf "$WORK_DIR" 2>/dev/null || true
    fi
}
trap cleanup EXIT ERR INT TERM

parse_args() {
    while (( $# > 0 )); do
        case "$1" in
            --keep-work) KEEP_WORK=1 ;;
            --skip-bootstrap) SKIP_BOOTSTRAP=1 ;;
            --small) USE_SMALL_MODEL=1 ;;
            -h|--help)
                sed -n '1,70p' "$0"
                exit 0
                ;;
            *) die "Unknown argument: $1" ;;
        esac
        shift
    done
}

selected_model_name() {
    if (( USE_SMALL_MODEL == 1 )); then
        printf '%s\n' "${SMALL_MODEL_NAME}"
    else
        printf '%s\n' "${DEFAULT_MODEL_NAME}"
    fi
}

selected_model_source() {
    if (( USE_SMALL_MODEL == 1 )); then
        [[ -n "${MODEL_SOURCE_SMALL}" ]] || \
            die "--small requested but COGNOS_GGUF_MODEL_SOURCE_SMALL is unset"
        printf '%s\n' "${MODEL_SOURCE_SMALL}"
    else
        [[ -n "${MODEL_SOURCE_DEFAULT}" ]] || \
            die "COGNOS_GGUF_MODEL_SOURCE is unset"
        printf '%s\n' "${MODEL_SOURCE_DEFAULT}"
    fi
}

check_prerequisites() {
    local missing=()
    for cmd in debootstrap chroot mksquashfs sha256sum make install rsync; do
        command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
    done
    if (( ${#missing[@]} > 0 )); then
        die "Missing required tools: ${missing[*]}"
    fi
    if (( EUID != 0 )); then
        die "This script must run as root."
    fi
    [[ -f "$BASE_PACKAGES" ]] || die "Base packages list not found: $BASE_PACKAGES"
    [[ -d "$AGENTS_SRC" ]] || die "Agents dir not found: $AGENTS_SRC"
    [[ -d "$SYSTEMD_SRC_DIR" ]] || die "Systemd units dir not found: $SYSTEMD_SRC_DIR"
    [[ -d "$SERVICES_SRC_DIR" ]] || die "Services dir not found: $SERVICES_SRC_DIR"
    [[ -d "$SECURITY_DIR" ]] || die "Security dir not found: $SECURITY_DIR"
    [[ -f "$INTENT_GRAMMAR_SRC" ]] || die "Intent grammar missing: $INTENT_GRAMMAR_SRC"
    [[ -x "$PROTO_PYTHON" ]] || die "Proto Python interpreter missing: $PROTO_PYTHON"
}

validate_host_artifacts() {
    log "Validating host build artifacts before bootstrap"

    local bin
    for bin in "${CORE_RUST_BINARIES[@]}" "${SUPPORT_RUST_BINARIES[@]}"; do
        [[ -x "${RELEASE_DIR}/${bin}" ]] || die "Required Rust binary not built: ${RELEASE_DIR}/${bin}"
    done

    [[ -n "${LLAMA_SERVER_BIN}" ]] || die "COGNOS_LLAMA_SERVER_BIN is unset"
    [[ -x "${LLAMA_SERVER_BIN}" ]] || die "llama-server binary not executable: ${LLAMA_SERVER_BIN}"

    local model_src
    model_src="$(selected_model_source)"
    [[ -f "${model_src}" ]] || die "GGUF model missing: ${model_src}"

    find "$BUILD_DIR/artifacts" "$BUILD_DIR" -maxdepth 2 -name 'linux-image-*.deb' -print -quit 2>/dev/null | \
        grep -q . || die "Kernel package missing: expected linux-image-*.deb under $BUILD_DIR or $BUILD_DIR/artifacts"

    make -C "${PROJECT_ROOT}/build" proto PYTHON="${PROTO_PYTHON}"
    [[ -f "${AGENTS_SRC}/proto/cognos_pb2.py" ]] || die "Generated proto missing: agents/proto/cognos_pb2.py"
    [[ -f "${AGENTS_SRC}/proto/cognos_pb2_grpc.py" ]] || die "Generated proto missing: agents/proto/cognos_pb2_grpc.py"
}

bind_mount() {
    local src="$1"
    local dst="$2"
    mkdir -p "$dst"
    mount --bind "$src" "$dst"
    MOUNTS_ACTIVE+=("$dst")
}

umount_all() {
    local mnt
    for (( i=${#MOUNTS_ACTIVE[@]}-1; i>=0; i-- )); do
        mnt="${MOUNTS_ACTIVE[$i]}"
        if mountpoint -q "$mnt" 2>/dev/null; then
            umount -lf "$mnt" 2>/dev/null || true
        fi
    done
    MOUNTS_ACTIVE=()
}

prepare_chroot() {
    mkdir -p "$WORK_DIR"
    if (( SKIP_BOOTSTRAP == 1 )) && [[ -d "$ROOTFS_DIR" && -f "$ROOTFS_DIR/etc/debian_version" ]]; then
        log "Reusing existing rootfs at $ROOTFS_DIR"
    else
        rm -rf "$ROOTFS_DIR"
        mkdir -p "$ROOTFS_DIR"
        log "Bootstrapping Debian rootfs ($SUITE/$ARCH)"
        debootstrap \
            --variant=minbase \
            --arch="$ARCH" \
            --no-install-recommends \
            --merged-usr \
            "$SUITE" \
            "$ROOTFS_DIR" \
            "$MIRROR"
    fi

    bind_mount /proc "$ROOTFS_DIR/proc"
    bind_mount /sys "$ROOTFS_DIR/sys"
    bind_mount /dev "$ROOTFS_DIR/dev"
    mkdir -p "$ROOTFS_DIR/dev/pts"
    mount -t devpts devpts "$ROOTFS_DIR/dev/pts"
    MOUNTS_ACTIVE+=("$ROOTFS_DIR/dev/pts")
    [[ -f /etc/resolv.conf ]] && cp /etc/resolv.conf "$ROOTFS_DIR/etc/resolv.conf"
}

chroot_run() {
    chroot "$ROOTFS_DIR" /usr/bin/env -i \
        HOME=/root \
        PATH=/usr/sbin:/usr/bin:/sbin:/bin \
        DEBIAN_FRONTEND=noninteractive \
        bash -c "$1"
}

install_base_packages() {
    local packages
    packages="$(grep -vE '^\s*(#|$)' "$BASE_PACKAGES" | tr '\n' ' ')"
    [[ -n "$packages" ]] || die "No packages listed in $BASE_PACKAGES"

    chroot_run "
        apt-get update -qq
        apt-get install -y --no-install-recommends ${packages}
        apt-get clean
        rm -rf /var/lib/apt/lists/*
    "
}

create_cognos_user() {
    chroot_run "
        getent group cognos >/dev/null 2>&1 || groupadd --system cognos
        id -u cognos >/dev/null 2>&1 || useradd --system --gid cognos --home-dir /var/lib/cognos --create-home --shell /usr/sbin/nologin cognos
    "
}

install_rust_runtime() {
    install -d -m 0755 "$ROOTFS_DIR/usr/bin" "$ROOTFS_DIR/usr/lib/cognos"
    local bin
    for bin in "${CORE_RUST_BINARIES[@]}" "${SUPPORT_RUST_BINARIES[@]}"; do
        install -m 0755 "${RELEASE_DIR}/${bin}" "$ROOTFS_DIR/usr/bin/${bin}"
        ln -sfn "/usr/bin/${bin}" "$ROOTFS_DIR/usr/lib/cognos/${bin}"
        log "  installed Rust runtime: ${bin}"
    done
}

install_llama_runtime() {
    local model_src model_name model_dst
    model_src="$(selected_model_source)"
    model_name="$(selected_model_name)"
    model_dst="$ROOTFS_DIR/var/lib/cognos/models/$(basename "$model_src")"

    install -d -m 0755 "$ROOTFS_DIR/usr/bin" "$ROOTFS_DIR/var/lib/cognos/models"
    install -m 0755 "${LLAMA_SERVER_BIN}" "$ROOTFS_DIR/usr/bin/llama-server"
    ln -sfn "/usr/bin/llama-server" "$ROOTFS_DIR/usr/lib/cognos/llama-server"
    install -m 0644 "${model_src}" "${model_dst}"
    printf '%s\n' "${model_name}" > "$ROOTFS_DIR/var/lib/cognos/models/default-model.txt"
}

install_agents_runtime() {
    local agents_dst="$ROOTFS_DIR/opt/cognos/agents"
    install -d -m 0755 "$agents_dst" "$ROOTFS_DIR/opt/cognos"
    rsync -a --delete "${AGENTS_SRC}/" "${agents_dst}/"

    chroot_run "
        python3 -m venv /opt/cognos/venv
        /opt/cognos/venv/bin/pip install --upgrade pip
        /opt/cognos/venv/bin/pip install -r /opt/cognos/agents/requirements.txt
    "
}

write_cognos_configs() {
    local model_basename model_name
    model_basename="$(basename "$(selected_model_source)")"
    model_name="$(selected_model_name)"

    install -d -m 0755 "$ROOTFS_DIR/etc/cognos" "$ROOTFS_DIR/etc/cognos/sway.config.d"
    install -m 0644 "$CONFIG_SRC_DIR/intent.toml" "$ROOTFS_DIR/etc/cognos/intent.toml"
    install -m 0644 "$INTENT_GRAMMAR_SRC" "$ROOTFS_DIR/etc/cognos/intent.gbnf"

    "$PROTO_PYTHON" -c "from pathlib import Path; p=Path(r'$ROOTFS_DIR/etc/cognos/intent.toml'); t=p.read_text(); t=t.replace('model = \"qwen3-7b-q4_k_m\"', 'model = \"$model_name\"'); p.write_text(t)"

    cat > "$ROOTFS_DIR/etc/cognos/ipc.toml" <<EOF
[server]
bind = "127.0.0.1:7443"
secret_env = "COGNOS_IPC_SECRET"
EOF

    cat > "$ROOTFS_DIR/etc/cognos/orchestrator.toml" <<EOF
[hal]
endpoint = "http://127.0.0.1:7444"

[intent]
endpoint = "http://127.0.0.1:7445"

[agents]
dir = "/opt/cognos/agents"
python = "/opt/cognos/venv/bin/python3"
EOF

    cat > "$ROOTFS_DIR/etc/cognos/memory.toml" <<EOF
[storage]
root = "/var/lib/cognos/memory"

[chromadb]
path = "/var/lib/cognos/memory/chromadb"
EOF

    cat > "$ROOTFS_DIR/etc/cognos/scheduler.toml" <<EOF
[scheduler]
state_dir = "/var/lib/cognos/scheduler"

[telemetry]
ebpf_enabled = true
EOF

    cat > "$ROOTFS_DIR/etc/cognos/hal.toml" <<EOF
[audit]
path = "/var/lib/cognos/hal/audit.jsonl"
EOF

    cat > "$ROOTFS_DIR/etc/cognos/agents.env" <<EOF
PYTHONPATH=/opt/cognos/agents
COGNOS_IPC_ENDPOINT=http://127.0.0.1:7443
EOF

    cat > "$ROOTFS_DIR/etc/cognos/llama-server.env" <<EOF
LLAMA_MODEL=/var/lib/cognos/models/${model_basename}
LLAMA_ALIAS=${model_name}
LLAMA_HOST=127.0.0.1
LLAMA_PORT=8080
EOF
}

install_security_configs() {
    install -d -m 0755 "$ROOTFS_DIR/etc/nftables.d" "$ROOTFS_DIR/etc/apparmor.d" "$ROOTFS_DIR/usr/lib/systemd/system"
    install -m 0644 "$SECURITY_DIR/nftables/ai-isolation.nft" "$ROOTFS_DIR/etc/nftables.d/cognos-ai-isolation.nft"

    local profile
    for profile in "$SECURITY_DIR"/apparmor/*; do
        [[ -f "$profile" ]] || continue
        install -m 0644 "$profile" "$ROOTFS_DIR/etc/apparmor.d/$(basename "$profile")"
    done

    install -m 0644 "$SECURITY_DIR/cgroups/cognos.slice" "$ROOTFS_DIR/usr/lib/systemd/system/cognos.slice"
}

install_systemd_units() {
    install -d -m 0755 "$ROOTFS_DIR/usr/lib/systemd/system"

    install -m 0644 "$SERVICES_SRC_DIR/cognos-hal.service" "$ROOTFS_DIR/usr/lib/systemd/system/"
    install -m 0644 "$SERVICES_SRC_DIR/cognos-intent.service" "$ROOTFS_DIR/usr/lib/systemd/system/"
    install -m 0644 "$SERVICES_SRC_DIR/cognos-memory.service" "$ROOTFS_DIR/usr/lib/systemd/system/"
    install -m 0644 "$SERVICES_SRC_DIR/cognos-orchestrator.service" "$ROOTFS_DIR/usr/lib/systemd/system/"
    install -m 0644 "$SERVICES_SRC_DIR/cognos-scheduler.service" "$ROOTFS_DIR/usr/lib/systemd/system/"
    install -m 0644 "$SERVICES_SRC_DIR/cognos-ipc.service" "$ROOTFS_DIR/usr/lib/systemd/system/"
    install -m 0644 "$SERVICES_SRC_DIR/cognos-agents.service" "$ROOTFS_DIR/usr/lib/systemd/system/"
    install -m 0644 "$SERVICES_SRC_DIR/cognos-llm.service" "$ROOTFS_DIR/usr/lib/systemd/system/"
    install -m 0644 "$SERVICES_SRC_DIR/cognos-ui-agent.service" "$ROOTFS_DIR/usr/lib/systemd/system/"
    install -m 0644 "$SERVICES_SRC_DIR/cognos.target" "$ROOTFS_DIR/usr/lib/systemd/system/"

    cat > "$ROOTFS_DIR/usr/lib/systemd/system/cognos-init-ipc-secret.service" <<'EOF'
[Unit]
Description=Generate COGNOS IPC HMAC secret on first boot
DefaultDependencies=no
Before=cognos-ipc.service cognos-intent.service cognos-orchestrator.service cognos-agents.service

[Service]
Type=oneshot
ExecStart=/usr/libexec/cognos-init-ipc-secret
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF

    install -d -m 0755 "$ROOTFS_DIR/usr/libexec"
    cat > "$ROOTFS_DIR/usr/libexec/cognos-init-ipc-secret" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
install -d -m 0750 /etc/cognos
if [[ ! -s /etc/cognos/ipc.env ]]; then
    umask 0027
    secret="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
    printf 'COGNOS_IPC_SECRET=%s\n' "$secret" > /etc/cognos/ipc.env
    chown root:cognos /etc/cognos/ipc.env
    chmod 0640 /etc/cognos/ipc.env
fi
EOF
    chmod 0755 "$ROOTFS_DIR/usr/libexec/cognos-init-ipc-secret"

    local svc
    for svc in cognos-ipc cognos-intent cognos-orchestrator cognos-agents; do
        install -d -m 0755 "$ROOTFS_DIR/usr/lib/systemd/system/${svc}.service.d"
        cat > "$ROOTFS_DIR/usr/lib/systemd/system/${svc}.service.d/10-ipc-secret.conf" <<EOF
[Service]
EnvironmentFile=-/etc/cognos/ipc.env
EOF
    done

    install -d -m 0755 "$ROOTFS_DIR/usr/lib/systemd/system/cognos-agents.service.d"
    cat > "$ROOTFS_DIR/usr/lib/systemd/system/cognos-agents.service.d/20-agents-env.conf" <<'EOF'
[Service]
EnvironmentFile=-/etc/cognos/agents.env
EOF

    cat > "$ROOTFS_DIR/usr/lib/systemd/system/cognos-qemu-e2e.service" <<'EOF'
[Unit]
Description=COGNOS automated QEMU E2E smoke test
ConditionPathExists=/dev/disk/by-label/COGNOSCFG
After=local-fs.target network.target
Wants=network.target

[Service]
Type=oneshot
ExecStart=/usr/libexec/cognos-qemu-e2e
StandardOutput=journal+console
StandardError=journal+console

[Install]
WantedBy=multi-user.target
EOF

    cat > "$ROOTFS_DIR/usr/libexec/cognos-qemu-e2e" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

CFG_LABEL="COGNOSCFG"
CFG_MNT="/run/cognos-qemu-e2e"
CFG_ENV="${CFG_MNT}/e2e.env"
LOG_PREFIX="[cognos-qemu-e2e]"

log() {
    echo "${LOG_PREFIX} $*"
}

fail() {
    log "COGNOS_E2E_RESULT=FAIL reason=$*"
    sync
    systemctl --no-block poweroff || true
    exit 1
}

cleanup() {
    umount "${CFG_MNT}" 2>/dev/null || true
}
trap cleanup EXIT

mkdir -p "${CFG_MNT}"
mount -o ro "/dev/disk/by-label/${CFG_LABEL}" "${CFG_MNT}"
[[ -f "${CFG_ENV}" ]] || fail "missing ${CFG_ENV}"
# shellcheck disable=SC1090
source "${CFG_ENV}"

INTENT_TEXT="${COGNOS_E2E_INTENT:-crée un dossier test dans /tmp}"
TARGET_DIR="${COGNOS_E2E_TARGET:-/tmp/test}"
MODE="${COGNOS_E2E_MODE:-live}"
TIMEOUT_SEC="${COGNOS_E2E_TIMEOUT_SEC:-180}"
export COGNOS_EXTRA_PATHS="${COGNOS_EXTRA_PATHS:-/tmp}"

wait_for_unit() {
    local unit="$1" deadline=$((SECONDS + TIMEOUT_SEC))
    while [[ "$(systemctl is-active "${unit}" 2>/dev/null || true)" != "active" ]]; do
        if (( SECONDS >= deadline )); then
            fail "timeout waiting for ${unit}"
        fi
        sleep 1
    done
}

if [[ "${MODE}" == "no-model" ]]; then
    log "masking cognos-llm.service for degraded fallback boot"
    systemctl mask --runtime cognos-llm.service || true
    shopt -s nullglob
    for model in /var/lib/cognos/models/*.gguf; do
        mv "${model}" "${model}.bak"
    done
    shopt -u nullglob
fi

log "starting cognos.target"
systemctl start cognos.target
wait_for_unit cognos-ipc.service
wait_for_unit cognos-hal.service
wait_for_unit cognos-scheduler.service
wait_for_unit cognos-memory.service
wait_for_unit cognos-orchestrator.service
wait_for_unit cognos-intent.service
wait_for_unit cognos-agents.service
if [[ "${MODE}" != "no-model" ]]; then
    wait_for_unit cognos-llm.service
fi

rm -rf "${TARGET_DIR}"
log "running CLI intent: ${INTENT_TEXT}"
if ! CLI_OUT="$(/usr/bin/cognos intent "${INTENT_TEXT}" 2>&1)"; then
    echo "${CLI_OUT}"
    fail "cli intent failed"
fi
echo "${CLI_OUT}"

[[ -d "${TARGET_DIR}" ]] || fail "expected target dir missing: ${TARGET_DIR}"
echo "${CLI_OUT}" | grep -qi 'HAL:' || fail "cli output missing HAL decision"
grep -q '"outcome":"approved"' /var/lib/cognos/hal/audit.jsonl || fail "HAL audit missing approved outcome"

if [[ "${MODE}" == "no-model" ]]; then
    systemctl is-active --quiet cognos-llm.service && fail "llm service unexpectedly active in no-model mode"
fi

log "COGNOS_E2E_RESULT=PASS mode=${MODE} target=${TARGET_DIR}"
sync
systemctl --no-block poweroff || true
EOF
    chmod 0755 "$ROOTFS_DIR/usr/libexec/cognos-qemu-e2e"

    install -d -m 0755 \
        "$ROOTFS_DIR/etc/systemd/system/multi-user.target.wants" \
        "$ROOTFS_DIR/etc/systemd/system/getty.target.wants"
    ln -sfn /usr/lib/systemd/system/cognos-init-ipc-secret.service \
        "$ROOTFS_DIR/etc/systemd/system/multi-user.target.wants/cognos-init-ipc-secret.service"
    ln -sfn /usr/lib/systemd/system/cognos-qemu-e2e.service \
        "$ROOTFS_DIR/etc/systemd/system/multi-user.target.wants/cognos-qemu-e2e.service"
    if [[ -f "$ROOTFS_DIR/usr/lib/systemd/system/serial-getty@.service" ]]; then
        ln -sfn /usr/lib/systemd/system/serial-getty@.service \
            "$ROOTFS_DIR/etc/systemd/system/getty.target.wants/serial-getty@ttyS0.service"
    fi
}

create_runtime_layout() {
    install -d -m 0750 "$ROOTFS_DIR/var/lib/cognos"
    install -d -m 0750 "$ROOTFS_DIR/var/lib/cognos/state"
    install -d -m 0755 "$ROOTFS_DIR/var/lib/cognos/models"
    install -d -m 0750 "$ROOTFS_DIR/var/lib/cognos/memory"
    install -d -m 0750 "$ROOTFS_DIR/var/lib/cognos/hal"
    install -d -m 0750 "$ROOTFS_DIR/var/lib/cognos/ipc"
    install -d -m 0750 "$ROOTFS_DIR/var/lib/cognos/intent"
    install -d -m 0750 "$ROOTFS_DIR/var/lib/cognos/orchestrator"
    install -d -m 0750 "$ROOTFS_DIR/var/lib/cognos/scheduler"
    install -d -m 0755 "$ROOTFS_DIR/var/log/cognos"
    install -d -m 0755 "$ROOTFS_DIR/run/cognos"

    chroot_run "
        chown -R cognos:cognos /var/lib/cognos /var/log/cognos
        chmod 0750 /var/lib/cognos /var/lib/cognos/state /var/lib/cognos/memory /var/lib/cognos/hal /var/lib/cognos/ipc /var/lib/cognos/intent /var/lib/cognos/orchestrator /var/lib/cognos/scheduler
    "
}

setup_fstab() {
    cat > "$ROOTFS_DIR/etc/fstab" <<EOF
/dev/root / ext4 defaults,noatime,errors=ro 0 1
tmpfs /tmp tmpfs defaults,noexec,nosuid,nodev 0 0
tmpfs /var/tmp tmpfs defaults,noexec,nosuid,nodev 0 0
proc /proc proc defaults 0 0
sysfs /sys sysfs defaults 0 0
EOF

    echo "$HOSTNAME_VAL" > "$ROOTFS_DIR/etc/hostname"
    cat > "$ROOTFS_DIR/etc/hosts" <<EOF
127.0.0.1 localhost
127.0.1.1 $HOSTNAME_VAL
::1 localhost ip6-localhost ip6-loopback
EOF
}

install_kernel() {
    local kernel_deb
    kernel_deb="$(find "$BUILD_DIR/artifacts" "$BUILD_DIR" -maxdepth 2 -name 'linux-image-*.deb' -print -quit 2>/dev/null || true)"
    [[ -n "$kernel_deb" ]] || die "No linux-image-*.deb found under $BUILD_DIR or $BUILD_DIR/artifacts"
    cp "$kernel_deb" "$ROOTFS_DIR/tmp/"
    chroot_run "
        dpkg -i /tmp/$(basename "$kernel_deb") || apt-get install -y -f --no-install-recommends
    "
    rm -f "$ROOTFS_DIR/tmp/$(basename "$kernel_deb")"
}

setup_initramfs() {
    install -d -m 0755 "$ROOTFS_DIR/etc/initramfs-tools/conf.d"
    cat > "$ROOTFS_DIR/etc/initramfs-tools/conf.d/cognos-live.conf" <<EOF
export COGNOS_LIVE=1
EOF
    chroot_run "update-initramfs -u -k all" || warn "update-initramfs reported errors"
}

create_squashfs() {
    umount_all
    rm -f "$OUTPUT_SQUASHFS"
    mksquashfs "$ROOTFS_DIR" "$OUTPUT_SQUASHFS" -comp zstd -Xcompression-level 19 -no-xattrs -noappend -all-root -quiet
    (cd "$(dirname "$OUTPUT_SQUASHFS")" && sha256sum "$(basename "$OUTPUT_SQUASHFS")" > "$OUTPUT_SHA256")
}

main() {
    parse_args "$@"
    check_prerequisites
    validate_host_artifacts

    log "COGNOS rootfs build — output rootfs: $ROOTFS_DIR"
    prepare_chroot
    install_base_packages
    create_cognos_user
    install_rust_runtime
    install_llama_runtime
    install_agents_runtime
    write_cognos_configs
    install_security_configs
    install_systemd_units
    create_runtime_layout
    install_kernel
    setup_fstab
    setup_initramfs
    create_squashfs

    "${SCRIPT_DIR}/verify_rootfs.sh" "$ROOTFS_DIR"
    log "Rootfs ready at $ROOTFS_DIR"
}

main "$@"
