#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
readonly PROJECT_ROOT

ROOTFS_DIR="${1:-${PROJECT_ROOT}/build/rootfs}"

log()  { echo "[INFO] $*" >&2; }
die()  { echo "[ERR]  $*" >&2; exit 1; }

require_path() {
    local path="$1"
    [[ -e "${ROOTFS_DIR}${path}" ]] || die "Missing ${path}"
}

require_file() {
    local path="$1"
    [[ -f "${ROOTFS_DIR}${path}" ]] || die "Missing file ${path}"
}

require_exec() {
    local path="$1"
    [[ -x "${ROOTFS_DIR}${path}" ]] || die "Missing executable ${path}"
}

main() {
    [[ -d "$ROOTFS_DIR" ]] || die "Rootfs directory not found: $ROOTFS_DIR"

    local bin
    for bin in cognos-hal cognos-intent cognos-scheduler cognos-memory cognos-orchestrator cognos-ipc-server cognos llama-server; do
        require_exec "/usr/bin/${bin}"
    done

    for link in cognos-hal cognos-intent cognos-scheduler cognos-memory cognos-orchestrator cognos-ipc-server cognos llama-server; do
        require_path "/usr/lib/cognos/${link}"
    done

    require_exec "/opt/cognos/venv/bin/python3"
    require_file "/opt/cognos/agents/proto/cognos_pb2.py"
    require_file "/opt/cognos/agents/proto/cognos_pb2_grpc.py"
    require_file "/etc/cognos/intent.toml"
    require_file "/etc/cognos/ipc.toml"
    require_file "/etc/cognos/orchestrator.toml"
    require_file "/etc/cognos/memory.toml"
    require_file "/etc/cognos/scheduler.toml"
    require_file "/etc/cognos/hal.toml"
    require_file "/etc/cognos/intent.gbnf"
    require_file "/etc/cognos/agents.env"
    require_file "/etc/cognos/llama-server.env"

    require_file "/usr/lib/systemd/system/cognos-hal.service"
    require_file "/usr/lib/systemd/system/cognos-intent.service"
    require_file "/usr/lib/systemd/system/cognos-memory.service"
    require_file "/usr/lib/systemd/system/cognos-orchestrator.service"
    require_file "/usr/lib/systemd/system/cognos-scheduler.service"
    require_file "/usr/lib/systemd/system/cognos-ipc.service"
    require_file "/usr/lib/systemd/system/cognos-agents.service"
    require_file "/usr/lib/systemd/system/cognos-ui-agent.service"
    require_file "/usr/lib/systemd/system/cognos-llm.service"
    require_file "/usr/lib/systemd/system/cognos-init-ipc-secret.service"
    require_file "/usr/lib/systemd/system/cognos-qemu-e2e.service"
    require_file "/usr/lib/systemd/system/cognos.target"
    require_exec "/usr/libexec/cognos-init-ipc-secret"
    require_exec "/usr/libexec/cognos-qemu-e2e"

    require_file "/etc/nftables.d/cognos-ai-isolation.nft"
    require_file "/etc/apparmor.d/cognos-agents"
    require_file "/etc/apparmor.d/cognos-ai-daemon"
    require_file "/usr/lib/systemd/system/cognos.slice"

    require_path "/var/lib/cognos"
    require_path "/var/lib/cognos/state"
    require_path "/var/lib/cognos/models"
    require_path "/var/lib/cognos/memory"
    require_file "/var/lib/cognos/models/default-model.txt"
    require_path "/etc/systemd/system/multi-user.target.wants/cognos-init-ipc-secret.service"
    require_path "/etc/systemd/system/multi-user.target.wants/cognos-qemu-e2e.service"

    grep -q '^cognos:' "${ROOTFS_DIR}/etc/passwd" || die "cognos user missing from /etc/passwd"
    grep -q '^cognos:' "${ROOTFS_DIR}/etc/group" || die "cognos group missing from /etc/group"

    local model_path
    model_path="$(sed -n 's/^LLAMA_MODEL=//p' "${ROOTFS_DIR}/etc/cognos/llama-server.env")"
    [[ -n "$model_path" ]] || die "LLAMA_MODEL missing in /etc/cognos/llama-server.env"
    require_file "$model_path"

    log "Rootfs checklist passed for ${ROOTFS_DIR}"
}

main "$@"
