#!/usr/bin/env bash
set -euo pipefail

# Verifies that the compiled kernel .config meets COGNOS security requirements.
# Exit 1 if any mandatory option is missing or any forbidden option is present.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly SCRIPT_DIR

CONFIG_FILE="${1:-${SCRIPT_DIR}/cache/linux-src/.config}"

if [[ ! -f "$CONFIG_FILE" ]]; then
    echo "[ERROR] Kernel .config not found at: $CONFIG_FILE"
    echo "Usage: $0 [path/to/.config]"
    exit 1
fi

VIOLATIONS=0

log_pass() { echo "[PASS] $1"; }
log_fail() { echo "[FAIL] $1"; VIOLATIONS=$((VIOLATIONS + 1)); }

# ─── Mandatory options (must be set to =y or =m) ─────────────────────────────
MUST_HAVE=(
    # Memory hardening
    "CONFIG_RANDOMIZE_BASE=y"
    "CONFIG_RANDOMIZE_MEMORY=y"
    "CONFIG_STACKPROTECTOR_STRONG=y"
    "CONFIG_FORTIFY_SOURCE=y"
    "CONFIG_HARDENED_USERCOPY=y"
    "CONFIG_INIT_ON_ALLOC_DEFAULT_ON=y"
    "CONFIG_INIT_ON_FREE_DEFAULT_ON=y"
    "CONFIG_KFENCE=y"
    "CONFIG_SLAB_FREELIST_RANDOM=y"
    "CONFIG_SLAB_FREELIST_HARDENED=y"
    "CONFIG_PAGE_TABLE_ISOLATION=y"
    "CONFIG_VMAP_STACK=y"
    "CONFIG_INIT_STACK_ALL_ZERO=y"

    # Access control
    "CONFIG_STRICT_DEVMEM=y"
    "CONFIG_IO_STRICT_DEVMEM=y"
    "CONFIG_SECURITY_DMESG_RESTRICT=y"
    "CONFIG_SECURITY_LOCKDOWN_LSM=y"
    "CONFIG_LOCK_DOWN_KERNEL_FORCE_INTEGRITY=y"

    # Module signing
    "CONFIG_MODULE_SIG=y"
    "CONFIG_MODULE_SIG_FORCE=y"
    "CONFIG_MODULE_SIG_SHA512=y"

    # LSM
    "CONFIG_SECURITY=y"
    "CONFIG_SECURITY_APPARMOR=y"
    "CONFIG_SECURITY_LANDLOCK=y"
    "CONFIG_SECURITY_YAMA=y"
    "CONFIG_INTEGRITY=y"
    "CONFIG_IMA=y"
    "CONFIG_EVM=y"

    # eBPF (required by COGNOS telemetry)
    "CONFIG_BPF=y"
    "CONFIG_BPF_SYSCALL=y"
    "CONFIG_BPF_JIT=y"
    "CONFIG_BPF_JIT_ALWAYS_ON=y"
    "CONFIG_BPF_LSM=y"
    "CONFIG_HAVE_EBPF_JIT=y"
    "CONFIG_DEBUG_INFO_BTF=y"
    "CONFIG_TRACEPOINTS=y"
    "CONFIG_PERF_EVENTS=y"
    "CONFIG_CGROUP_BPF=y"

    # Filesystem integrity
    "CONFIG_DM_VERITY=y"
    "CONFIG_DM_CRYPT=y"
    "CONFIG_SQUASHFS=y"
    "CONFIG_OVERLAY_FS=y"

    # Namespaces & cgroups (agent sandboxing)
    "CONFIG_NAMESPACES=y"
    "CONFIG_SECCOMP=y"
    "CONFIG_SECCOMP_FILTER=y"
    "CONFIG_CGROUPS=y"
    "CONFIG_MEMCG=y"

    # EFI/Secure Boot
    "CONFIG_EFI=y"
    "CONFIG_EFI_STUB=y"
)

# ─── Forbidden options (must NOT be set) ──────────────────────────────────────
MUST_NOT_HAVE=(
    "CONFIG_KGDB"
    "CONFIG_DEBUG_FS"
    "CONFIG_MAGIC_SYSRQ"
    "CONFIG_DEVKMEM"
    "CONFIG_ACPI_CUSTOM_DSDT"
    "CONFIG_HIBERNATION"
    "CONFIG_USERFAULTFD"
    "CONFIG_BINFMT_MISC"
    "CONFIG_KPROBES"
    "CONFIG_DEVMEM"
    "CONFIG_PROC_KCORE"
    "CONFIG_MODIFY_LDT_SYSCALL"
    "CONFIG_COMPAT_BRK"
    "CONFIG_USELIB"
    "CONFIG_X86_MSR"
    "CONFIG_X86_CPUID"
    "CONFIG_STAGING"
    "CONFIG_KSM"
)

echo "═══════════════════════════════════════════════════════════════"
echo " COGNOS Kernel Config Compliance Check"
echo " Config: $CONFIG_FILE"
echo "═══════════════════════════════════════════════════════════════"
echo ""

echo "── Mandatory Options ──────────────────────────────────────────"
for entry in "${MUST_HAVE[@]}"; do
    if grep -qx "$entry" "$CONFIG_FILE"; then
        log_pass "$entry"
    else
        log_fail "$entry — NOT FOUND or incorrect value"
    fi
done

echo ""
echo "── Forbidden Options ──────────────────────────────────────────"
for option in "${MUST_NOT_HAVE[@]}"; do
    if grep -q "^${option}=y" "$CONFIG_FILE" || grep -q "^${option}=m" "$CONFIG_FILE"; then
        actual=$(grep "^${option}=" "$CONFIG_FILE" | head -1)
        log_fail "${option} must be disabled (found: ${actual})"
    else
        log_pass "${option} is not set"
    fi
done

echo ""
echo "═══════════════════════════════════════════════════════════════"
if [[ $VIOLATIONS -gt 0 ]]; then
    echo "ERROR: ${VIOLATIONS} violation(s) detected. Kernel config NON-COMPLIANT."
    exit 1
else
    echo "SUCCESS: All checks passed. Kernel config is compliant."
    exit 0
fi
