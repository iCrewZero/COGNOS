#!/usr/bin/env python3
"""
COGNOS OS Installer

Securely installs COGNOS OS to a target disk with:
- Dry-run mode by default (--execute required for actual writes)
- SHA-256 image verification before any write
- System disk detection and protection
- GPT partitioning with LUKS2 encryption support
- Interactive confirmations with disk identification

Usage:
    python3 installer.py --image cognos-rootfs.squashfs --target /dev/sdX
    python3 installer.py --image cognos-rootfs.squashfs --target /dev/sdX --execute

Exit codes:
    0 — Success
    1 — User error (bad arguments, user cancelled)
    2 — System error (missing tools, I/O failure)
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

# ─── Constants ────────────────────────────────────────────────────────────────

ESP_SIZE_MB = 512
SWAP_MAX_MB = 8192
BLOCK_SIZE = 4096
LARGE_DISK_THRESHOLD_GB = 256

logger = logging.getLogger("cognos-installer")


# ─── Data Classes ─────────────────────────────────────────────────────────────


@dataclass
class DiskInfo:
    path: str
    model: str
    size_bytes: int
    serial: str
    removable: bool
    partitions: list[str]
    mounted_partitions: list[str]

    @property
    def size_gb(self) -> float:
        return self.size_bytes / (1024**3)

    @property
    def size_human(self) -> str:
        gb = self.size_gb
        if gb >= 1000:
            return f"{gb / 1024:.1f} TB"
        return f"{gb:.1f} GB"


@dataclass
class PartitionLayout:
    esp: str
    root: str
    swap: Optional[str]
    encrypted: bool


# ─── Disk Validator ───────────────────────────────────────────────────────────


class DiskValidator:
    """Validates target disk safety before destructive operations."""

    @staticmethod
    def list_block_devices() -> list[dict]:
        """List all block devices using lsblk."""
        result = subprocess.run(
            [
                "lsblk",
                "-J",
                "-o",
                "NAME,SIZE,MODEL,SERIAL,RM,TYPE,MOUNTPOINT,FSTYPE",
                "-b",
            ],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            logger.error("lsblk failed: %s", result.stderr)
            return []
        data = json.loads(result.stdout)
        return data.get("blockdevices", [])

    @staticmethod
    def get_disk_info(device: str) -> Optional[DiskInfo]:
        """Gather detailed information about a disk device."""
        dev_name = os.path.basename(device)
        devices = DiskValidator.list_block_devices()

        for dev in devices:
            if dev.get("name") == dev_name and dev.get("type") == "disk":
                partitions = []
                mounted = []
                for child in dev.get("children", []):
                    part_path = f"/dev/{child['name']}"
                    partitions.append(part_path)
                    if child.get("mountpoint"):
                        mounted.append(f"{part_path} -> {child['mountpoint']}")

                return DiskInfo(
                    path=device,
                    model=dev.get("model", "Unknown").strip(),
                    size_bytes=int(dev.get("size", 0)),
                    serial=dev.get("serial", "N/A") or "N/A",
                    removable=dev.get("rm", False),
                    partitions=partitions,
                    mounted_partitions=mounted,
                )
        return None

    @staticmethod
    def is_system_disk(device: str) -> bool:
        """Check if the device contains the root filesystem or critical mounts."""
        dev_name = os.path.basename(device)
        devices = DiskValidator.list_block_devices()

        critical_mounts = {"/", "/boot", "/boot/efi", "/home", "/var"}

        for dev in devices:
            if dev.get("name") == dev_name:
                for child in dev.get("children", []):
                    mp = child.get("mountpoint")
                    if mp and mp in critical_mounts:
                        return True
        return False

    @staticmethod
    def is_mounted(device: str) -> bool:
        """Check if any partition on the device is currently mounted."""
        info = DiskValidator.get_disk_info(device)
        if info is None:
            return False
        return len(info.mounted_partitions) > 0

    @staticmethod
    def confirm_destruction(device: str, force: bool = False) -> bool:
        """Interactive confirmation before destructive operations."""
        info = DiskValidator.get_disk_info(device)
        if info is None:
            logger.error("Cannot read disk info for %s", device)
            return False

        print("\n" + "=" * 60)
        print("  WARNING: DESTRUCTIVE OPERATION")
        print("=" * 60)
        print(f"  Device:   {info.path}")
        print(f"  Model:    {info.model}")
        print(f"  Size:     {info.size_human}")
        print(f"  Serial:   {info.serial}")
        print(f"  Partitions: {len(info.partitions)}")
        if info.mounted_partitions:
            print(f"  MOUNTED:  {', '.join(info.mounted_partitions)}")
        print("=" * 60)
        print("\n  ALL DATA ON THIS DEVICE WILL BE PERMANENTLY DESTROYED.\n")

        if force:
            return True

        response = input("  Type 'YES' to confirm: ").strip()
        if response != "YES":
            logger.info("User cancelled operation")
            return False

        if info.size_gb > LARGE_DISK_THRESHOLD_GB:
            print(f"\n  EXTRA WARNING: This disk is {info.size_human}.")
            print("  Large disks are likely data/media drives.")
            response2 = input("  Type 'DESTROY' to confirm: ").strip()
            if response2 != "DESTROY":
                logger.info("User cancelled at second confirmation")
                return False

        return True


# ─── Image Verifier ───────────────────────────────────────────────────────────


class ImageVerifier:
    """Verifies image integrity before installation."""

    @staticmethod
    def compute_sha256(path: Path) -> str:
        """Compute SHA-256 hash of a file."""
        h = hashlib.sha256()
        with open(path, "rb") as f:
            while chunk := f.read(1024 * 1024):
                h.update(chunk)
        return h.hexdigest()

    @staticmethod
    def verify_integrity(image: Path, expected_sha256: Optional[str] = None) -> bool:
        """Verify image integrity via SHA-256."""
        if not image.exists():
            logger.error("Image file not found: %s", image)
            return False

        sha256_file = image.with_suffix(image.suffix + ".sha256")

        if expected_sha256:
            actual = ImageVerifier.compute_sha256(image)
            if actual != expected_sha256:
                logger.error(
                    "SHA-256 mismatch!\n  Expected: %s\n  Actual:   %s",
                    expected_sha256,
                    actual,
                )
                return False
            logger.info("SHA-256 verified: %s", actual[:16] + "...")
            return True

        if sha256_file.exists():
            content = sha256_file.read_text().strip()
            expected = content.split()[0]
            actual = ImageVerifier.compute_sha256(image)
            if actual != expected:
                logger.error(
                    "SHA-256 mismatch!\n  Expected: %s\n  Actual:   %s",
                    expected,
                    actual,
                )
                return False
            logger.info("SHA-256 verified from %s", sha256_file.name)
            return True

        logger.warning("No SHA-256 checksum provided or found — skipping verification")
        return True

    @staticmethod
    def verify_gpg_signature(image: Path) -> bool:
        """Verify GPG signature if .asc file exists."""
        sig_file = image.with_suffix(image.suffix + ".asc")
        if not sig_file.exists():
            logger.info("No GPG signature file found — skipping GPG verification")
            return True

        result = subprocess.run(
            ["gpg", "--verify", str(sig_file), str(image)],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            logger.error("GPG verification failed:\n%s", result.stderr)
            return False

        logger.info("GPG signature verified")
        return True


# ─── Installer ────────────────────────────────────────────────────────────────


class Installer:
    """Main installer class orchestrating the COGNOS OS installation."""

    def __init__(
        self,
        image: Path,
        target: str,
        dry_run: bool = True,
        sha256: Optional[str] = None,
        encrypt: bool = False,
    ):
        self.image = image
        self.target = target
        self.dry_run = dry_run
        self.sha256 = sha256
        self.encrypt = encrypt
        self.layout: Optional[PartitionLayout] = None

    def _exec(self, cmd: list[str], desc: str) -> subprocess.CompletedProcess:
        """Execute a command, or log it in dry-run mode."""
        cmd_str = " ".join(cmd)
        if self.dry_run:
            logger.info("[DRY-RUN] Would execute: %s", cmd_str)
            return subprocess.CompletedProcess(cmd, 0, "", "")

        logger.info("[EXEC] %s", cmd_str)
        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode != 0:
            logger.error("%s failed (exit %d):\n%s", desc, result.returncode, result.stderr)
            raise RuntimeError(f"{desc} failed with exit code {result.returncode}")
        return result

    def validate(self) -> bool:
        """Pre-flight validation checks."""
        logger.info("Running pre-flight checks...")

        # Check required tools
        required_tools = ["sgdisk", "mkfs.fat", "mkfs.ext4", "unsquashfs", "lsblk"]
        if self.encrypt:
            required_tools.append("cryptsetup")

        missing = [t for t in required_tools if not shutil.which(t)]
        if missing:
            logger.error("Missing required tools: %s", ", ".join(missing))
            return False

        # Verify image
        if not ImageVerifier.verify_integrity(self.image, self.sha256):
            return False

        if not ImageVerifier.verify_gpg_signature(self.image):
            return False

        # Check target disk
        if not os.path.exists(self.target):
            logger.error("Target device does not exist: %s", self.target)
            return False

        if DiskValidator.is_system_disk(self.target):
            logger.error(
                "REFUSED: %s is a system disk (contains / or /boot). "
                "This is likely your running OS.",
                self.target,
            )
            return False

        if DiskValidator.is_mounted(self.target):
            logger.error(
                "REFUSED: %s has mounted partitions. Unmount them first.",
                self.target,
            )
            return False

        logger.info("All pre-flight checks passed")
        return True

    def partition_disk(self) -> PartitionLayout:
        """Create GPT partition table on target disk."""
        logger.info("Partitioning %s (GPT)...", self.target)

        # Wipe existing partition table
        self._exec(["sgdisk", "--zap-all", self.target], "Wipe partition table")

        # Partition 1: EFI System Partition (512 MB)
        self._exec(
            [
                "sgdisk",
                f"--new=1:0:+{ESP_SIZE_MB}M",
                "--typecode=1:EF00",
                "--change-name=1:ESP",
                self.target,
            ],
            "Create ESP",
        )

        # Partition 2: Root filesystem (remaining - swap)
        # Calculate swap size
        try:
            mem_bytes = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
            swap_mb = min(mem_bytes // (2 * 1024 * 1024), SWAP_MAX_MB)
        except (ValueError, OSError):
            swap_mb = 2048

        self._exec(
            [
                "sgdisk",
                f"--new=2:0:-{swap_mb}M",
                "--typecode=2:8300",
                "--change-name=2:COGNOS_ROOT",
                self.target,
            ],
            "Create root partition",
        )

        # Partition 3: Swap
        self._exec(
            [
                "sgdisk",
                "--new=3:0:0",
                "--typecode=3:8200",
                "--change-name=3:SWAP",
                self.target,
            ],
            "Create swap partition",
        )

        self.layout = PartitionLayout(
            esp=f"{self.target}1",
            root=f"{self.target}2",
            swap=f"{self.target}3",
            encrypted=self.encrypt,
        )

        return self.layout

    def format_partitions(self) -> None:
        """Format the partitions."""
        if self.layout is None:
            raise RuntimeError("Must partition disk before formatting")

        logger.info("Formatting partitions...")

        # ESP — FAT32
        self._exec(
            ["mkfs.fat", "-F", "32", "-n", "ESP", self.layout.esp],
            "Format ESP",
        )

        # Root — ext4 (or LUKS + ext4)
        if self.encrypt:
            logger.info("Setting up LUKS2 encryption on root partition...")
            if not self.dry_run:
                print("\n  You will be prompted to set the LUKS passphrase.\n")
            self._exec(
                [
                    "cryptsetup",
                    "luksFormat",
                    "--type=luks2",
                    "--cipher=aes-xts-plain64",
                    "--key-size=512",
                    "--hash=sha512",
                    "--pbkdf=argon2id",
                    self.layout.root,
                ],
                "LUKS format",
            )
            self._exec(
                ["cryptsetup", "open", self.layout.root, "cognos_root"],
                "LUKS open",
            )
            self._exec(
                ["mkfs.ext4", "-L", "COGNOS_ROOT", "-F", "/dev/mapper/cognos_root"],
                "Format root (encrypted)",
            )
        else:
            self._exec(
                ["mkfs.ext4", "-L", "COGNOS_ROOT", "-F", self.layout.root],
                "Format root",
            )

        # Swap
        if self.layout.swap:
            self._exec(
                ["mkswap", "-L", "SWAP", self.layout.swap],
                "Format swap",
            )

    def install_rootfs(self) -> None:
        """Extract squashfs to the root partition."""
        if self.layout is None:
            raise RuntimeError("Must partition and format before installing")

        mount_point = Path("/mnt/cognos_install")
        root_dev = (
            "/dev/mapper/cognos_root" if self.encrypt else self.layout.root
        )

        logger.info("Installing rootfs to %s...", root_dev)

        self._exec(["mkdir", "-p", str(mount_point)], "Create mount point")
        self._exec(["mount", root_dev, str(mount_point)], "Mount root")

        try:
            # Extract squashfs
            self._exec(
                ["unsquashfs", "-f", "-d", str(mount_point), str(self.image)],
                "Extract rootfs",
            )

            # Mount and populate ESP
            esp_mount = mount_point / "boot" / "efi"
            self._exec(["mkdir", "-p", str(esp_mount)], "Create ESP mount point")
            self._exec(
                ["mount", self.layout.esp, str(esp_mount)],
                "Mount ESP",
            )

            # Install bootloader
            self._exec(
                [
                    "bootctl",
                    f"--esp-path={esp_mount}",
                    "install",
                ],
                "Install systemd-boot",
            )

            self._exec(["umount", str(esp_mount)], "Unmount ESP")
        finally:
            self._exec(["umount", str(mount_point)], "Unmount root")
            if self.encrypt:
                self._exec(
                    ["cryptsetup", "close", "cognos_root"],
                    "Close LUKS",
                )

    def run(self) -> int:
        """Execute the full installation pipeline."""
        mode = "DRY-RUN" if self.dry_run else "LIVE"
        logger.info("=" * 60)
        logger.info("  COGNOS OS Installer [%s MODE]", mode)
        logger.info("=" * 60)
        logger.info("  Image:  %s", self.image)
        logger.info("  Target: %s", self.target)
        logger.info("  Encrypt: %s", self.encrypt)
        logger.info("=" * 60)

        if not self.validate():
            return 1

        if not self.dry_run:
            if not DiskValidator.confirm_destruction(self.target):
                return 1

        self.partition_disk()
        self.format_partitions()
        self.install_rootfs()

        logger.info("=" * 60)
        if self.dry_run:
            logger.info("  DRY-RUN COMPLETE — no changes were made")
            logger.info("  Re-run with --execute to perform installation")
        else:
            logger.info("  INSTALLATION COMPLETE")
            logger.info("  You may now reboot into COGNOS OS")
        logger.info("=" * 60)

        return 0


# ─── CLI ──────────────────────────────────────────────────────────────────────


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="COGNOS OS Installer — Secure disk installation",
        epilog="By default runs in DRY-RUN mode. Pass --execute to write to disk.",
    )
    parser.add_argument(
        "--image",
        type=Path,
        required=True,
        help="Path to the COGNOS rootfs squashfs image",
    )
    parser.add_argument(
        "--target",
        type=str,
        required=True,
        help="Target block device (e.g. /dev/sdb)",
    )
    parser.add_argument(
        "--execute",
        action="store_true",
        default=False,
        help="Actually perform the installation (default: dry-run)",
    )
    parser.add_argument(
        "--sha256",
        type=str,
        default=None,
        help="Expected SHA-256 hash of the image file",
    )
    parser.add_argument(
        "--encrypt",
        action="store_true",
        default=False,
        help="Encrypt root partition with LUKS2",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        default=False,
        help="Enable verbose logging",
    )
    parser.add_argument(
        "--list-disks",
        action="store_true",
        default=False,
        help="List available target disks and exit",
    )
    return parser.parse_args()


def list_available_disks() -> None:
    """Print available disks that are safe installation targets."""
    print("\nAvailable target disks:")
    print("-" * 60)

    devices = DiskValidator.list_block_devices()
    for dev in devices:
        if dev.get("type") != "disk":
            continue

        path = f"/dev/{dev['name']}"
        is_system = DiskValidator.is_system_disk(path)
        is_mounted = DiskValidator.is_mounted(path)

        size_bytes = int(dev.get("size", 0))
        size_gb = size_bytes / (1024**3)
        model = (dev.get("model") or "Unknown").strip()

        status = ""
        if is_system:
            status = " [SYSTEM — PROTECTED]"
        elif is_mounted:
            status = " [MOUNTED — unmount first]"
        else:
            status = " [AVAILABLE]"

        print(f"  {path:<12} {size_gb:>7.1f} GB  {model:<20}{status}")

    print("-" * 60)
    print()


def main() -> int:
    args = parse_args()

    log_level = logging.DEBUG if args.verbose else logging.INFO
    logging.basicConfig(
        level=log_level,
        format="%(asctime)s [%(levelname)s] %(message)s",
        datefmt="%H:%M:%S",
    )

    if args.list_disks:
        list_available_disks()
        return 0

    if not args.image.exists():
        logger.error("Image file not found: %s", args.image)
        return 1

    if os.geteuid() != 0 and args.execute:
        logger.error("Installation requires root privileges. Run with sudo.")
        return 2

    installer = Installer(
        image=args.image,
        target=args.target,
        dry_run=not args.execute,
        sha256=args.sha256,
        encrypt=args.encrypt,
    )

    try:
        return installer.run()
    except RuntimeError as e:
        logger.error("Installation failed: %s", e)
        return 2
    except KeyboardInterrupt:
        logger.info("\nInstallation cancelled by user")
        return 1


if __name__ == "__main__":
    sys.exit(main())
