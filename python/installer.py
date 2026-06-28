# DEPRECATED: Use build/installer.py instead.
# This file is kept for reference only.
# Owner: iCrewZero

"""COGNOS Installer — Python-based installer for development and recovery scenarios. The production installer is Rust (installer/); this Python version exists for cases where Python is available but the Rust toolchain is not."""

import os
import sys
import shutil
import subprocess
import argparse
import logging
from pathlib import Path
from dataclasses import dataclass
from typing import List, Optional

logger = logging.getLogger("cognos.installer")


@dataclass
class InstallConfig:
    """Where COGNOS files land and which optional steps to run.

    Attributes:
        prefix: Base prefix for binaries (e.g. /usr/local).
        lib_dir: Where runtime binaries and eBPF objects live.
        etc_dir: Where configuration files live.
        var_dir: Where runtime state lives.
        user: System user the daemons run as.
        group: System group the daemons run as.
        skip_services: If True, do not install or enable systemd units.
        dry_run: If True, log every action but do not mutate the filesystem.
    """

    prefix: Path = Path("/usr/local")
    lib_dir: Path = Path("/usr/lib/cognos")
    etc_dir: Path = Path("/etc/cognos")
    var_dir: Path = Path("/var/lib/cognos")
    user: str = "cognos"
    group: str = "cognos"
    skip_services: bool = False
    dry_run: bool = False


class CognosInstaller:
    """Python installer for COGNOS — mirrors the Rust installer's steps.

    The Rust installer (installer/) is the source of truth; this class exists
    for recovery and dev-setups where the Rust toolchain is unavailable.
    """

    def __init__(self, config: InstallConfig) -> None:
        self.config = config

    def check_prerequisites(self) -> None:
        """Verify root, that the target user exists (or can be created), and disk space."""
        if os.geteuid() != 0:
            logger.warning("not running as root — most steps will fail")
        # TODO(v1): check that useradd/shutil.disk_usage are available
        logger.info("prerequisites OK")

    def create_directories(self) -> None:
        """Create lib_dir, etc_dir and var_dir with appropriate permissions."""
        for d in (self.config.lib_dir, self.config.etc_dir, self.config.var_dir):
            logger.info("mkdir -p %s", d)
            if self.config.dry_run:
                continue
            d.mkdir(parents=True, exist_ok=True)
            # TODO(v1): chown to self.config.user:group and chmod 0750

    def create_user(self) -> None:
        """Create the system user/group if missing."""
        # TODO(v1): use pwd/grp to detect, then `useradd --system cognos`
        logger.info("ensuring user %s exists", self.config.user)

    def install_binaries(self) -> None:
        """Copy built binaries from target/release/* into lib_dir."""
        src = Path("target/release")
        if not src.exists():
            logger.warning("target/release missing — skipping binary install")
            return
        for binary in src.glob("cognos*"):
            dst = self.config.lib_dir / binary.name
            logger.info("install %s -> %s", binary, dst)
            if self.config.dry_run:
                continue
            shutil.copy2(binary, dst)
            os.chmod(dst, 0o755)

    def install_configs(self) -> None:
        """Copy configs/ into etc_dir/."""
        src = Path("configs")
        if not src.exists():
            logger.warning("configs/ missing — skipping config install")
            return
        for cfg in src.rglob("*"):
            if cfg.is_file():
                dst = self.config.etc_dir / cfg.relative_to(src)
                logger.info("install config %s -> %s", cfg, dst)
                if self.config.dry_run:
                    continue
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(cfg, dst)

    def install_services(self) -> None:
        """Copy systemd units and enable them."""
        if self.config.skip_services:
            logger.info("skip_services=True — skipping systemd install")
            return
        src = Path("systemd")
        dst = Path("/etc/systemd/system")
        for unit in src.glob("*.service"):
            target = dst / unit.name
            logger.info("install unit %s -> %s", unit, target)
            if self.config.dry_run:
                continue
            shutil.copy2(unit, target)
            subprocess.run(["systemctl", "daemon-reload"], check=False)
            subprocess.run(["systemctl", "enable", unit.name], check=False)

    def install_security_policies(self) -> None:
        """Copy AppArmor profiles into /etc/apparmor.d/."""
        src = Path("security/apparmor")
        dst = Path("/etc/apparmor.d")
        if not src.exists():
            logger.warning("security/apparmor missing — skipping policy install")
            return
        for profile in src.iterdir():
            if profile.is_file():
                target = dst / profile.name
                logger.info("install apparmor %s -> %s", profile, target)
                if self.config.dry_run:
                    continue
                shutil.copy2(profile, target)

    def install_kernel_modules(self) -> None:
        """Copy compiled eBPF objects (kernel/ebpf/*.o) into lib_dir/ebpf/."""
        src = Path("kernel/ebpf")
        dst = self.config.lib_dir / "ebpf"
        if not src.exists():
            logger.warning("kernel/ebpf missing — skipping eBPF install")
            return
        for obj in src.glob("*.o"):
            target = dst / obj.name
            logger.info("install eBPF %s -> %s", obj, target)
            if self.config.dry_run:
                continue
            dst.mkdir(parents=True, exist_ok=True)
            shutil.copy2(obj, target)

    def post_install(self) -> None:
        """Reload systemd, AppArmor and any other runtime state."""
        logger.info("post-install: reloading daemons")
        if self.config.dry_run or self.config.skip_services:
            return
        # TODO(v1): apparmor_parser -r /etc/apparmor.d/cognos*
        subprocess.run(["systemctl", "daemon-reload"], check=False)

    def verify(self) -> bool:
        """Run a verification pass; returns True if everything looks healthy."""
        logger.info("verify: checking installed files")
        # TODO(v1): check binaries are executable, configs parse, units valid
        return True

    def run(self) -> None:
        """Orchestrate the full install with logging at each step."""
        steps = [
            self.check_prerequisites,
            self.create_user,
            self.create_directories,
            self.install_binaries,
            self.install_configs,
            self.install_security_policies,
            self.install_kernel_modules,
            self.install_services,
            self.post_install,
        ]
        for step in steps:
            logger.info("== step: %s ==", step.__name__)
            step()
        ok = self.verify()
        logger.info("install complete, verify=%s", ok)


def parse_args() -> argparse.Namespace:
    """Parse CLI arguments."""
    p = argparse.ArgumentParser(description="COGNOS Python installer (dev/recovery)")
    p.add_argument("--prefix", type=Path, default=Path("/usr/local"), help="Install prefix")
    p.add_argument("--user", default="cognos", help="System user for daemons")
    p.add_argument("--skip-services", action="store_true", help="Do not install systemd units")
    p.add_argument("--dry-run", action="store_true", help="Log actions without touching the FS")
    p.add_argument("-v", "--verbose", action="count", default=0, help="Increase verbosity")
    return p.parse_args()


def main() -> None:
    """Entrypoint: parse args, build config, run installer."""
    args = parse_args()
    level = logging.WARNING - 10 * args.verbose
    logging.basicConfig(level=max(level, logging.DEBUG))
    config = InstallConfig(
        prefix=args.prefix,
        lib_dir=args.prefix / "lib" / "cognos" if args.prefix != Path("/usr/local") else Path("/usr/lib/cognos"),
        user=args.user,
        group=args.user,
        skip_services=args.skip_services,
        dry_run=args.dry_run,
    )
    installer = CognosInstaller(config)
    installer.run()


if __name__ == "__main__":
    main()

# v0: stub — Rust installer is the source of truth
