"""
UI Agent for COGNOS/OS.

Manages all user-facing display that is not HAL dialogs.
Translates agent state into the visual layer.
The only agent function that waits for user input: show_disambiguation().
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import socket
import time
import uuid
from dataclasses import dataclass, field, asdict
from datetime import datetime, UTC
from pathlib import Path
from typing import Literal

from shared.base_agent import BaseAgent

log = logging.getLogger("cognos.ui")

RUN_DIR = Path.home() / ".cognos" / "run"
UI_STATE_FILE = RUN_DIR / "ui-state.json"
NOTIF_SOCK = RUN_DIR / "notifications.sock"
RESOURCES_FILE = RUN_DIR / "resources.json"
AUDIT_LOG = Path.home() / ".cognos" / "audit.log"

AgentStatus = Literal["idle", "running", "thinking", "alert", "unavailable"]
NotifLevel = Literal["info", "success", "warning"]
LogoutAction = Literal["logout", "shutdown", "reboot", "cancel"]


# ─── Types ───────────────────────────────────────────────────────────────────

@dataclass
class ResourceStats:
    cpu_percent: float
    ram_used_gb: float
    ram_total_gb: float
    battery_percent: int | None
    battery_charging: bool
    ai_cpu_percent: float


@dataclass
class MemoryBrowserData:
    total_files: int
    storage_mb: float
    domains: dict[str, int]          # domain → file count
    recent_files: list[dict]         # last 20 indexed
    top_files: list[dict]            # highest importance score


# ─── UI Agent ────────────────────────────────────────────────────────────────

class UIAgent(BaseAgent):
    """
    Manages visual layer state: agent dots, notifications, resource stats.
    Does not modify agent behavior. Does not bypass HAL.
    """

    def __init__(self, memory_client=None):
        super().__init__("ui")
        self._memory = memory_client
        self._prev_cpu: tuple[int, int] | None = None
        self._agent_status: dict[str, str] = {
            name: "idle" for name in
            ("planner", "memory", "security", "scheduler", "file", "coding")
        }
        RUN_DIR.mkdir(parents=True, exist_ok=True)

    # ─── Agent status display ─────────────────────────────────────────────────

    async def update_agent_status(self, agent: str, status: AgentStatus) -> None:
        """Write agent status to ui-state.json for the shell top bar to read."""
        if agent in self._agent_status:
            self._agent_status[agent] = status

        state = {
            "agents": dict(self._agent_status),
            "updated_at": datetime.now(UTC).isoformat(),
        }

        tmp = UI_STATE_FILE.with_suffix(".tmp")
        tmp.write_text(json.dumps(state))
        tmp.rename(UI_STATE_FILE)  # atomic write — no partial reads

    # ─── Notifications ────────────────────────────────────────────────────────

    async def notify(
        self,
        message: str,
        level: NotifLevel = "info",
        duration_secs: int = 5,
        action: str | None = None,
    ) -> None:
        """
        Send a non-HAL informational notification to the shell layer.

        ONLY use for:
          - Memory indexing complete
          - Context preloaded (with undo)
          - Model loaded
          - Behavioral anomaly
          - Package installed
          - Update available
        """
        notif = {
            "id": str(uuid.uuid4()),
            "message": message,
            "level": level,
            "duration_secs": duration_secs,
            "action": action,
        }

        await self._send_to_socket(NOTIF_SOCK, json.dumps(notif))
        log.debug("Notification [%s]: %s", level, message)

    # ─── Resource display ─────────────────────────────────────────────────────

    async def update_resource_stats(self, stats: ResourceStats) -> None:
        """Write resource stats for shell top bar. Called every 2 seconds."""
        data = {
            "cpu_percent": round(stats.cpu_percent, 1),
            "ram_used_gb": round(stats.ram_used_gb, 1),
            "ram_total_gb": round(stats.ram_total_gb, 1),
            "battery_percent": stats.battery_percent,
            "battery_charging": stats.battery_charging,
            "ai_cpu_percent": round(stats.ai_cpu_percent, 1),
            "updated_at": datetime.now(UTC).isoformat(),
        }
        tmp = RESOURCES_FILE.with_suffix(".tmp")
        tmp.write_text(json.dumps(data))
        tmp.rename(RESOURCES_FILE)

    # ─── Memory browser data ──────────────────────────────────────────────────

    async def get_memory_browser_data(self) -> MemoryBrowserData:
        """
        Assemble data for the memory browser widget.
        Queries Memory Agent for indexed file stats.
        """
        if self._memory is None:
            return MemoryBrowserData(0, 0.0, {}, [], [])

        try:
            indexed = await asyncio.wait_for(
                self._memory.show_indexed(), timeout=3.0
            )
        except asyncio.TimeoutError:
            indexed = []

        # Compute domain breakdown
        domains: dict[str, int] = {}
        for path in indexed:
            domain = self._infer_domain(path)
            domains[domain] = domains.get(domain, 0) + 1

        # Estimate storage (ChromaDB size)
        chroma_dir = Path.home() / ".cognos" / "memory" / "chromadb"
        storage_mb = self._dir_size_mb(chroma_dir)

        return MemoryBrowserData(
            total_files=len(indexed),
            storage_mb=round(storage_mb, 1),
            domains=domains,
            recent_files=[{"path": p} for p in indexed[-20:]],
            top_files=[],  # populated by Memory Agent importance scores
        )

    # ─── Disambiguation display ───────────────────────────────────────────────

    async def show_disambiguation(
        self, question: str, options: list[str]
    ) -> str:
        """
        Show a disambiguation question in the intent bar area.
        Waits up to 30 seconds for user selection.
        Returns selected option string.
        """
        log.info("Disambiguation: %s  options=%s", question, options)

        # Write the question to a well-known file; the shell reads and displays it.
        q_file = RUN_DIR / "disambiguation_question.json"
        q_file.write_text(json.dumps({
            "question": question,
            "options": options,
            "id": str(uuid.uuid4()),
        }))

        # Poll for the response file
        resp_file = RUN_DIR / "disambiguation_response.json"
        if resp_file.exists():
            resp_file.unlink()

        for _ in range(300):  # 30s at 100ms intervals
            await asyncio.sleep(0.1)
            if resp_file.exists():
                try:
                    data = json.loads(resp_file.read_text())
                    resp_file.unlink()
                    return data.get("selection", options[0] if options else "")
                except (json.JSONDecodeError, OSError):
                    pass

        # Timeout: return first option, log uncertainty
        log.warning("Disambiguation timed out — using first option: %s", options[0] if options else "")
        return options[0] if options else ""

    # ─── Logout dialog ────────────────────────────────────────────────────────

    async def show_logout_dialog(self) -> LogoutAction:
        """Simple dialog for logout/shutdown/reboot. No AI involvement."""
        dialog_file = RUN_DIR / "logout_dialog.json"
        dialog_file.write_text(json.dumps({"show": True}))

        resp_file = RUN_DIR / "logout_response.json"
        if resp_file.exists():
            resp_file.unlink()

        for _ in range(600):  # 60s
            await asyncio.sleep(0.1)
            if resp_file.exists():
                try:
                    data = json.loads(resp_file.read_text())
                    resp_file.unlink()
                    return data.get("action", "cancel")
                except (json.JSONDecodeError, OSError):
                    pass

        return "cancel"

    # ─── Resource polling loop ────────────────────────────────────────────────

    async def _resource_poll_loop(self) -> None:
        """Background loop: reads system stats every 2 seconds."""
        while True:
            await asyncio.sleep(2)
            try:
                stats = await asyncio.get_running_loop().run_in_executor(
                    None, self._read_system_stats
                )
                await self.update_resource_stats(stats)
            except Exception as e:
                log.debug("Resource poll error: %s", e)

    def _read_system_stats(self) -> ResourceStats:
        """Read system stats from /proc. No external dependencies."""
        cpu = self._read_cpu_percent()
        ram_used, ram_total = self._read_ram_gb()
        bat_pct, bat_charging = self._read_battery()
        ai_cpu = self._read_ai_cgroup_cpu()
        return ResourceStats(
            cpu_percent=cpu,
            ram_used_gb=ram_used,
            ram_total_gb=ram_total,
            battery_percent=bat_pct,
            battery_charging=bat_charging,
            ai_cpu_percent=ai_cpu,
        )

    # NOTE: _prev_cpu must be instance-level (set in __init__), NOT class-level.
    # A class-level mutable default would be shared across all UIAgent instances,
    # corrupting CPU delta calculations. The old __post_init__ was dead code
    # because UIAgent inherits from BaseAgent (not a dataclass), so it never ran.
    # This comment serves as a guard against re-introducing that bug.

    def _read_cpu_percent(self) -> float:
        """Read current CPU usage as a percentage.

        Uses the delta between two /proc/stat reads to get the actual
        current usage, not the average since boot. The first call
        returns 0.0 because there's no previous sample to diff against.
        """
        try:
            with open("/proc/stat") as f:
                parts = f.readline().split()
            user = int(parts[1])
            nice = int(parts[2])
            system = int(parts[3])
            idle = int(parts[4])
            iowait = int(parts[5]) if len(parts) > 5 else 0
            current_total = user + nice + system + idle + iowait
            current_active = user + nice + system

            if self._prev_cpu is None:
                # First call — store and return 0.
                self._prev_cpu = (current_total, current_active)
                return 0.0

            prev_total, prev_active = self._prev_cpu
            self._prev_cpu = (current_total, current_active)

            delta_total = current_total - prev_total
            delta_active = current_active - prev_active

            if delta_total <= 0:
                return 0.0

            return round(100.0 * delta_active / delta_total, 1)
        except OSError:
            return 0.0

    def _read_ram_gb(self) -> tuple[float, float]:
        try:
            meminfo: dict[str, int] = {}
            with open("/proc/meminfo") as f:
                for line in f:
                    parts = line.split()
                    if len(parts) >= 2:
                        meminfo[parts[0].rstrip(":")] = int(parts[1])
            total_kb = meminfo.get("MemTotal", 0)
            avail_kb = meminfo.get("MemAvailable", 0)
            used_kb = total_kb - avail_kb
            return round(used_kb / 1024 / 1024, 1), round(total_kb / 1024 / 1024, 1)
        except OSError:
            return 0.0, 0.0

    def _read_battery(self) -> tuple[int | None, bool]:
        for bat in Path("/sys/class/power_supply").glob("BAT*"):
            try:
                cap = int((bat / "capacity").read_text().strip())
                status = (bat / "status").read_text().strip()
                charging = status in ("Charging", "Full")
                return cap, charging
            except OSError:
                continue
        return None, False

    def _read_ai_cgroup_cpu(self) -> float:
        cgroup = Path("/sys/fs/cgroup/cognos.slice/cognos-ai.slice/cpu.stat")
        if cgroup.exists():
            try:
                for line in cgroup.read_text().splitlines():
                    if line.startswith("usage_usec"):
                        return float(line.split()[1]) / 1_000_000
            except OSError:
                pass
        return 0.0

    async def _send_to_socket(self, sock_path: Path, data: str) -> None:
        if not sock_path.exists():
            return
        try:
            reader, writer = await asyncio.open_unix_connection(str(sock_path))
            writer.write(data.encode() + b"\n")
            await writer.drain()
            writer.close()
        except (OSError, ConnectionRefusedError):
            pass

    def _infer_domain(self, path: str) -> str:
        p = path.lower()
        if any(k in p for k in [".rs", ".py", ".js", ".ts", ".go", "src/", "projects/"]): return "coding"
        if any(k in p for k in [".md", ".txt", ".rst", "notes/", "docs/"]): return "writing"
        if any(k in p for k in ["research", "paper", "study"]): return "research"
        return "other"

    def _dir_size_mb(self, path: Path) -> float:
        total = 0
        try:
            for f in path.rglob("*"):
                if f.is_file():
                    total += f.stat().st_size
        except OSError:
            pass
        return total / 1024 / 1024

    async def run(self) -> None:
        log.info("UI Agent starting")
        await asyncio.gather(
            super().run(),
            self._resource_poll_loop(),
        )