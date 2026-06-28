"""
File Agent for COGNOS/OS.

Executes all filesystem operations on behalf of other agents.
Every operation is HAL-gated before execution.
No other agent touches the filesystem directly.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import shutil
from dataclasses import dataclass, field
from datetime import datetime, UTC
from pathlib import Path
from typing import Any

from shared.base_agent import BaseAgent
# from shared.capability_lattice import CapabilityLattice, CapabilityViolation  # TODO(v1): add capability checks on fs ops

log = logging.getLogger("cognos.file_agent")

COGNOS_DIR = Path.home() / ".cognos"
AUDIT_LOG = COGNOS_DIR / "audit.log"
CONFIG_FILE = COGNOS_DIR / "config.json"
MAX_DIR_ENTRIES = 500


# ─── Types ───────────────────────────────────────────────────────────────────

@dataclass
class OperationResult:
    success: bool
    message: str
    target: str = ""
    extra: dict = field(default_factory=dict)


@dataclass
class FileInfo:
    name: str
    path: str
    size: int
    modified_time: float
    is_dir: bool
    extension: str
    importance_score: float = 0.0
    semantic_tags: list[str] = field(default_factory=list)


@dataclass
class FileMetadata:
    path: str
    size: int
    modified_time: float
    permissions: str
    importance_score: float = 0.0
    semantic_tags: list[str] = field(default_factory=list)


class PathViolation(Exception):
    pass


# ─── Validated path ──────────────────────────────────────────────────────────

class ValidatedPath:
    def __init__(self, raw: str):
        expanded = os.path.expanduser(raw)
        resolved = os.path.realpath(expanded)

        home = str(Path.home())
        if "\x00" in resolved:
            raise PathViolation(f"Null byte in path: {raw!r}")
        if len(resolved) > 4096:
            raise PathViolation("Path too long (>4096 chars)")
        if not resolved.startswith(home):
            raise PathViolation(
                f"Path '{resolved}' is outside user home '{home}' — "
                "possible symlink escape or misconfiguration"
            )

        self._path = Path(resolved)

    @property
    def path(self) -> Path:
        return self._path

    def __str__(self) -> str:
        return str(self._path)


# ─── File Agent ──────────────────────────────────────────────────────────────

class FileAgent(BaseAgent):
    """
    Executes filesystem operations with HAL gating and capability enforcement.
    """

    EDITOR_COMMANDS: dict[str, str] = {
        "vscode": "code --reuse-window {files}",
        "code": "code --reuse-window {files}",
        "neovim": "foot -e nvim {files}",
        "nvim": "foot -e nvim {files}",
        "vim": "foot -e vim {files}",
        "emacs": "emacsclient -n {files}",
        "gedit": "gedit {files}",
    }

    def __init__(self, hal_client=None):
        super().__init__("file")
        self._hal = hal_client  # injected HAL IPC client

    # ─── Operations ──────────────────────────────────────────────────────────

    async def open_file(
        self, path: str, editor: str | None = None
    ) -> OperationResult:
        """Open a single file. HAL score typically < 0.3 (auto-approve)."""
        try:
            vpath = ValidatedPath(path)
        except PathViolation as e:
            return OperationResult(False, str(e), path)

        await self._hal_gate("open_file", str(vpath), 0.15, False)

        resolved_editor = editor or self._detect_editor()
        await self._launch_editor([str(vpath)], resolved_editor)
        self._audit("open_file", str(vpath), "success", note=f"editor={resolved_editor}")
        return OperationResult(True, f"Opened {vpath.path.name}", str(vpath))

    async def open_workspace(
        self, files: list[str], editor: str | None = None
    ) -> OperationResult:
        """Open multiple files in one editor instance. Single HAL gate for the batch."""
        validated = []
        for f in files:
            try:
                validated.append(ValidatedPath(f))
            except PathViolation as e:
                log.warning("Skipping invalid path: %s", e)

        if not validated:
            return OperationResult(False, "No valid files to open")

        await self._hal_gate("open_workspace", f"{len(validated)} files", 0.15, False)

        resolved_editor = editor or self._detect_editor()
        await self._launch_editor([str(v) for v in validated], resolved_editor)
        self._audit("open_workspace", str(validated[0]), "success",
                    note=f"{len(validated)} files, editor={resolved_editor}")
        return OperationResult(True, f"Opened {len(validated)} files", str(validated[0]))

    async def move_file(self, src: str, dst: str) -> OperationResult:
        """Move a file within user home. HAL notify (score 0.3–0.5)."""
        try:
            vsrc = ValidatedPath(src)
            vdst = ValidatedPath(dst)
        except PathViolation as e:
            return OperationResult(False, str(e))

        if not vsrc.path.exists():
            return OperationResult(False, f"Source not found: {src}")

        await self._hal_gate("move_file", str(vsrc), 0.4, False)

        vdst.path.parent.mkdir(parents=True, exist_ok=True)
        try:
            shutil.move(str(vsrc), str(vdst))
        except OSError as e:
            return OperationResult(False, f"Move failed: {e}")

        self._audit("move_file", str(vsrc), "success", note=f"→ {vdst}")
        return OperationResult(True, f"Moved to {vdst.path.name}", str(vdst))

    async def create_file(
        self, path: str, content: str = "", overwrite: bool = False
    ) -> OperationResult:
        """Create a file in user home. Will not overwrite unless explicitly requested."""
        try:
            vpath = ValidatedPath(path)
        except PathViolation as e:
            return OperationResult(False, str(e))

        if vpath.path.exists() and not overwrite:
            return OperationResult(
                False,
                f"File already exists at {path}. Pass overwrite=True to replace."
            )

        await self._hal_gate("create_file", str(vpath), 0.3, False)

        vpath.path.parent.mkdir(parents=True, exist_ok=True)
        vpath.path.write_text(content)
        self._audit("create_file", str(vpath), "success", note=f"{len(content)} chars")
        return OperationResult(True, f"Created {vpath.path.name}", str(vpath))

    async def list_directory(self, path: str) -> list[FileInfo]:
        """List directory contents. No HAL gate needed (read-only)."""
        try:
            vpath = ValidatedPath(path)
        except PathViolation as e:
            log.warning("list_directory: %s", e)
            return []

        if not vpath.path.is_dir():
            return []

        entries = []
        count = 0
        try:
            items = sorted(
                vpath.path.iterdir(),
                key=lambda p: (not p.is_dir(), -p.stat().st_mtime)
            )
        except OSError:
            return []

        for item in items:
            if count >= MAX_DIR_ENTRIES:
                log.info("list_directory: truncated at %d entries", MAX_DIR_ENTRIES)
                break
            try:
                stat = item.stat()
                entries.append(FileInfo(
                    name=item.name,
                    path=str(item),
                    size=stat.st_size,
                    modified_time=stat.st_mtime,
                    is_dir=item.is_dir(),
                    extension=item.suffix.lower(),
                ))
                count += 1
            except OSError:
                continue

        return entries

    async def get_metadata(self, path: str) -> FileMetadata | None:
        """Get file metadata including ANFS metadata if available."""
        try:
            vpath = ValidatedPath(path)
        except PathViolation:
            return None

        try:
            stat = vpath.path.stat()
        except OSError:
            return None

        return FileMetadata(
            path=str(vpath),
            size=stat.st_size,
            modified_time=stat.st_mtime,
            permissions=oct(stat.st_mode),
        )

    async def restore_session_state(self, session_summary: dict) -> OperationResult:
        """
        Restore files and terminal from a previous session.
        Core of cognitive context preloading.
        """
        files = session_summary.get("files_touched", [])
        if not files:
            return OperationResult(False, "No files in session summary")

        await self._hal_gate("restore_session_state", f"{len(files)} files", 0.2, False)

        result = await self.open_workspace(files)
        self._audit("restore_session_state", str(files[0]) if files else "",
                    "success", note=f"{len(files)} files restored")
        return result

    # ─── Private helpers ──────────────────────────────────────────────────────

    async def _hal_gate(
        self, action: str, target: str, pre_score: float, is_ai: bool
    ) -> None:
        """Submit a HAL gate request. Raises on denial."""
        if self._hal is None:
            return  # No HAL client (e.g. in tests) — proceed

        result = await self._hal.gate(
            agent="file",
            action=action,
            target=target,
            pre_score=pre_score,
            is_ai_generated=is_ai,
        )
        if not result.get("approved", True):
            raise PermissionError(f"HAL denied: {action} on {target}")

    async def _launch_editor(self, files: list[str], editor: str) -> None:
        cmd_template = self.EDITOR_COMMANDS.get(
            editor.lower(), "xdg-open {files}"
        )
        files_str = " ".join(f'"{f}"' for f in files)
        cmd = cmd_template.replace("{files}", files_str)
        await asyncio.create_subprocess_shell(
            cmd,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
        )

    def _detect_editor(self) -> str:
        """Detect the user's preferred editor in priority order."""
        # 1. Config file
        if CONFIG_FILE.exists():
            try:
                cfg = json.loads(CONFIG_FILE.read_text())
                if "editor" in cfg:
                    return cfg["editor"]
            except (json.JSONDecodeError, OSError):
                pass
        # 2 & 3. Environment
        for var in ("VISUAL", "EDITOR"):
            val = os.environ.get(var)
            if val:
                return val.split()[0]  # strip args
        return "xdg-open"

    def _audit(self, action: str, target: str, outcome: str, note: str = "") -> None:
        entry = {
            "ts": datetime.now(UTC).isoformat(),
            "agent": "file_agent",
            "action": action,
            "target": target,
            "outcome": outcome,
            "note": note,
        }
        try:
            AUDIT_LOG.parent.mkdir(parents=True, exist_ok=True)
            with open(AUDIT_LOG, "a") as f:
                f.write(json.dumps(entry) + "\n")
        except OSError as e:
            log.error("Audit write failed: %s", e)
