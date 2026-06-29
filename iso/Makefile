# ═══════════════════════════════════════════════════════════════════════════════
# COGNOS OS — ISO Builder Makefile
# ═══════════════════════════════════════════════════════════════════════════════
#
# Purpose:
#   Drives the assembly of a bootable COGNOS ISO image. This Makefile is a
#   thin orchestrator: it delegates rootfs/kernel/iso construction to the
#   scripts under $(SCRIPTS_DIR) and the existing kernel build under
#   ../build/. It is the user-facing entry point for producing release ISO
#   artifacts.
#
# Usage:
#   make              — Build the ISO (default target)
#   make iso          — Same as `make all`
#   make rootfs       — Build the squashfs root filesystem only
#   make kernel       — Build the COGNOS-patched kernel
#   make verify       — Verify the ISO SHA-256 checksum
#   make clean        — Remove ISO/rootfs artifacts
#   make help         — Show this help message
#
# Variables (override with `make VAR=value`):
#   VERSION       COGNOS release version           (default: 0.1.0)
#   ARCH          Target architecture              (default: x86_64)
#   BUILD_DIR     Output directory for artifacts   (default: build)
#   ISO           Final ISO path                   (derived from VERSION)
#   SCRIPTS_DIR   Directory of builder scripts     (default: ../scripts)
#
# ═══════════════════════════════════════════════════════════════════════════════

VERSION     ?= 0.1.0
ARCH        ?= x86_64
BUILD_DIR   ?= build
ISO         ?= $(BUILD_DIR)/cognos-$(VERSION).iso
SCRIPTS_DIR ?= ../scripts

.DEFAULT_GOAL := all

.PHONY: all iso rootfs kernel clean verify help

# ─── Targets ───────────────────────────────────────────────────────────────────

all: iso

iso: rootfs kernel
	@echo "[iso] Assembling ISO image -> $(ISO)"
	ISO_OUTPUT="$(ISO)" VERSION="$(VERSION)" ARCH="$(ARCH)" "$(SCRIPTS_DIR)/iso_builder.sh"

rootfs:
	@echo "[iso] Building root filesystem..."
	OUTPUT_SQUASHFS="$(BUILD_DIR)/rootfs.squashfs" "$(SCRIPTS_DIR)/rootfs_builder.sh"

kernel:
	@echo "[iso] Building kernel (delegating to ../build/kernel_build.sh)..."
	"../build/kernel_build.sh"

verify:
	@echo "[iso] Verifying ISO checksum..."
	sha256sum -c "$(ISO).sha256"

clean:
	@echo "[iso] Cleaning ISO artifacts..."
	rm -rf "$(BUILD_DIR)/rootfs.squashfs" \
	       "$(BUILD_DIR)"/cognos-*.iso \
	       "$(BUILD_DIR)"/cognos-*.iso.sha256
	@echo "[iso] Clean complete."

help:
	@echo "COGNOS OS — ISO Builder"
	@echo ""
	@echo "Targets:"
	@echo "  all       Build the full ISO (default)"
	@echo "  iso       Same as 'all'"
	@echo "  rootfs    Build the squashfs root filesystem"
	@echo "  kernel    Build the COGNOS kernel"
	@echo "  verify    Verify ISO SHA-256 checksum"
	@echo "  clean     Remove ISO/rootfs artifacts"
	@echo "  help      Show this help message"
	@echo ""
	@echo "Variables (override with 'make VAR=value'):"
	@echo "  VERSION       Release version   (default: $(VERSION))"
	@echo "  ARCH          Architecture      (default: $(ARCH))"
	@echo "  BUILD_DIR     Output directory  (default: $(BUILD_DIR))"
	@echo "  SCRIPTS_DIR   Scripts directory (default: $(SCRIPTS_DIR))"
	@echo "  ISO           ISO path          (default: $(ISO))"

# TODO(v1): multi-arch (aarch64) support, signed ISO (gpg/minisign),
#           reproducible builds via SOURCE_DATE_EPOCH, parallel kernel+rootfs,
#           incremental rebuilds via stamp files.
# v0: stub — single-arch build only
