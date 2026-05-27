# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS OS Build System — Top-level Orchestrator
# ═══════════════════════════════════════════════════════════════════════════════
#
# Usage:
#   make all        — Full build: kernel → verify → rootfs → boot → iso
#   make kernel     — Build kernel .deb packages
#   make verify     — Verify kernel config compliance
#   make rootfs     — Build root filesystem (squashfs)
#   make boot       — Configure bootloader in rootfs
#   make iso        — Assemble bootable ISO image
#   make checksums  — Generate SHA256SUMS for all artifacts
#   make clean      — Remove all build artifacts
#
# ═══════════════════════════════════════════════════════════════════════════════

SHELL := /bin/bash
.DEFAULT_GOAL := all

BUILD_DIR := $(shell cd "$(dir $(lastword $(MAKEFILE_LIST)))" && pwd)
PROJECT_ROOT := $(shell cd "$(BUILD_DIR)/.." && pwd)
OUTPUT_DIR := $(BUILD_DIR)/output

.PHONY: all kernel verify rootfs boot iso checksums clean check-tools help

# ─── Help ─────────────────────────────────────────────────────────────────────
help:
	@echo "COGNOS OS Build System"
	@echo ""
	@echo "Targets:"
	@echo "  all        Full pipeline: kernel → verify → rootfs → boot → iso → checksums"
	@echo "  kernel     Build Linux kernel with COGNOS defconfig"
	@echo "  verify     Verify kernel .config compliance against security policy"
	@echo "  rootfs     Build root filesystem (depends on kernel)"
	@echo "  boot       Configure bootloader and boot parameters"
	@echo "  iso        Assemble bootable ISO image (depends on rootfs)"
	@echo "  checksums  Generate SHA256SUMS for all output artifacts"
	@echo "  clean      Remove all build artifacts and caches"
	@echo "  check-tools  Verify all required tools are installed"
	@echo ""
	@echo "Environment variables:"
	@echo "  COGNOS_KERNEL_VERSION  Kernel version (default: 6.12.10)"
	@echo "  COGNOS_KERNEL_SHA256   Expected SHA-256 of kernel tarball"
	@echo "  COGNOS_BUILD_JOBS      Parallel jobs (default: nproc)"
	@echo "  COGNOS_SUITE           Debian suite (default: bookworm)"
	@echo "  COGNOS_MIRROR          APT mirror URL"

# ─── Full Pipeline ────────────────────────────────────────────────────────────
all: kernel verify rootfs boot iso checksums
	@echo ""
	@echo "═══════════════════════════════════════════════════════════"
	@echo " COGNOS OS BUILD COMPLETE"
	@echo " Artifacts: $(OUTPUT_DIR)/"
	@echo "═══════════════════════════════════════════════════════════"

# ─── Kernel Build ─────────────────────────────────────────────────────────────
kernel: check-tools
	@echo "[make] Building kernel..."
	@bash "$(BUILD_DIR)/kernel_build.sh"

# ─── Kernel Config Verification ───────────────────────────────────────────────
verify: kernel
	@echo "[make] Verifying kernel config compliance..."
	@bash "$(BUILD_DIR)/verify_kernel_config.sh"

# ─── Root Filesystem ──────────────────────────────────────────────────────────
rootfs: kernel
	@echo "[make] Building root filesystem..."
	@bash "$(BUILD_DIR)/rootfs_builder.sh"

# ─── Boot Configuration ──────────────────────────────────────────────────────
boot: rootfs
	@echo "[make] Configuring bootloader..."
	@bash "$(BUILD_DIR)/boot_optimize.sh"

# ─── ISO Assembly ─────────────────────────────────────────────────────────────
iso: boot
	@echo "[make] Assembling ISO image..."
	@bash "$(BUILD_DIR)/iso_builder.sh"

# ─── Checksums ────────────────────────────────────────────────────────────────
checksums:
	@echo "[make] Generating checksums..."
	@mkdir -p "$(OUTPUT_DIR)"
	@cd "$(OUTPUT_DIR)" && \
		if ls *.deb *.squashfs *.iso 2>/dev/null | head -1 > /dev/null 2>&1; then \
			sha256sum *.deb *.squashfs *.iso 2>/dev/null > SHA256SUMS || true; \
			echo "[make] SHA256SUMS written to $(OUTPUT_DIR)/SHA256SUMS"; \
		else \
			echo "[make] No artifacts found to checksum"; \
		fi

# ─── Clean ────────────────────────────────────────────────────────────────────
clean:
	@echo "[make] Cleaning build artifacts..."
	rm -rf "$(OUTPUT_DIR)"
	rm -rf "$(BUILD_DIR)/cache/linux-src"
	rm -rf "$(BUILD_DIR)/rootfs_work"
	rm -rf "$(BUILD_DIR)/iso_work"
	@echo "[make] Clean complete"

# ─── Tool Verification ────────────────────────────────────────────────────────
REQUIRED_TOOLS := bash make gcc flex bison bc tar curl
ROOTFS_TOOLS := debootstrap mksquashfs chroot
ISO_TOOLS := xorriso mtools

check-tools:
	@echo "[make] Checking build prerequisites..."
	@all_ok=true; \
	for tool in $(REQUIRED_TOOLS); do \
		if ! command -v "$$tool" >/dev/null 2>&1; then \
			echo "  [MISSING] $$tool"; \
			all_ok=false; \
		fi; \
	done; \
	if [ "$$all_ok" = "false" ]; then \
		echo "[ERROR] Missing required tools. Install them and retry."; \
		exit 1; \
	fi; \
	echo "[make] Core tools OK"
	@for tool in $(ROOTFS_TOOLS) $(ISO_TOOLS); do \
		if ! command -v "$$tool" >/dev/null 2>&1; then \
			echo "  [WARN] $$tool not found (needed for rootfs/iso targets)"; \
		fi; \
	done
