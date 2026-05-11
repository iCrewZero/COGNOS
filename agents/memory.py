import asyncio
import json
import os
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .shared.base_agent import BaseAgent


@dataclass
class MemoryResult:
    path: str
    relevance_score: float
    last_session_summary: str | None
    cursor_position: int | None
    co_opened_files: list[str]


class MemoryAgent(BaseAgent):
    def __init__(self) -> None:
        super().__init__("memory")
        self.home = Path.home()
        self.cognos_dir = self.home / ".cognos"
        self.memory_dir = self.cognos_dir / "memory"
        self.index_queue = self.memory_dir / "index_queue"
        self.index_scope = self.memory_dir / "index_scope.json"
        self.audit_log = self.cognos_dir / "audit.log"
        self.index_db = self.memory_dir / "indexed.json"
        self._open_files: set[str] = set()

    async def run_indexer(self) -> None:
        while True:
            for file_path in await self._drain_queue():
                await self._index_file(Path(file_path))
                await asyncio.sleep(0)
            await asyncio.sleep(1)

    async def query(self, intent_schema: dict) -> list[MemoryResult]:
        goal = intent_schema.get("goal", "")
        domain = intent_schema.get("domain", "")
        query_text = f"{goal} {domain}".strip().lower()

        records = await self._load_index()
        scored = []
        for item in records:
            score = 0.4 if query_text in (item.get("path", "") + item.get("filename", "")).lower() else 0.1
            score += self._temporal_boost(item) + self._co_occurrence_boost(item)
            scored.append((score, item))
            await asyncio.sleep(0)

        scored.sort(key=lambda x: x[0], reverse=True)
        top = scored[:5]
        return [
            MemoryResult(
                path=i["path"],
                relevance_score=round(s, 4),
                last_session_summary=i.get("last_session_summary"),
                cursor_position=i.get("cursor_position"),
                co_opened_files=i.get("co_opened_files", []),
            )
            for s, i in top
        ]

    async def show_indexed(self) -> list[str]:
        records = await self._load_index()
        return [r["path"] for r in records]

    async def forget(self, scope: str) -> None:
        records = await self._load_index()
        filtered = [r for r in records if not r.get("path", "").startswith(scope)]
        await self._save_index(filtered)
        await self._audit(f"forget scope={scope} removed={len(records)-len(filtered)}")

    async def on_session_end(self, session_id: str, last_errors: list[str], git_status: str) -> None:
        sessions_dir = self.cognos_dir / "context" / "sessions"
        sessions_dir.mkdir(parents=True, exist_ok=True)
        payload = {
            "session_id": session_id,
            "files_touched": sorted(self._open_files),
            "duration": "unknown",
            "last_errors_seen": last_errors,
            "git_status": git_status,
        }
        out = sessions_dir / f"{session_id}.json"
        out.write_text(json.dumps(payload, indent=2), encoding="utf-8")
        self._open_files.clear()

    async def _drain_queue(self) -> list[str]:
        self.index_queue.parent.mkdir(parents=True, exist_ok=True)
        if not self.index_queue.exists():
            return []
        raw = self.index_queue.read_text(encoding="utf-8")
        self.index_queue.write_text("", encoding="utf-8")
        lines = [x.strip() for x in raw.splitlines() if x.strip()]
        scope = await self._load_scope()
        no_index = [x for x in os.environ.get("COGNOS_NO_INDEX", "").split(":") if x]
        return [p for p in lines if self._path_allowed(p, scope, no_index)]

    def _path_allowed(self, p: str, scope: list[str], no_index: list[str]) -> bool:
        blocked = ["/.cognos/", "/tmp/", "/node_modules/", "/.git/objects/"]
        if any(x in p for x in blocked):
            return False
        if any(p.startswith(prefix) for prefix in no_index):
            return False
        return any(p.startswith(s) for s in scope)

    async def _index_file(self, path: Path) -> None:
        if not path.exists() or path.is_dir():
            return
        if path.stat().st_size > 10 * 1024 * 1024:
            return
        text = path.read_text(encoding="utf-8", errors="ignore")
        records = await self._load_index()
        prev = next((r for r in records if r["path"] == str(path)), None)
        mtime = path.stat().st_mtime
        if prev and prev.get("modified_time") == mtime:
            return
        access_frequency = float(prev.get("access_frequency", 1.0) if prev else 1.0)
        recency = 1.0
        session_depth = float(len(self._open_files) + 1)
        importance_score = 0.3 * access_frequency + 0.4 * recency + 0.3 * session_depth
        rec = {
            "path": str(path),
            "filename": path.name,
            "modified_time": mtime,
            "size": path.stat().st_size,
            "last_session_id": "unknown",
            "co_opened_files": sorted(self._open_files),
            "project_domain": "other",
            "importance_score": importance_score,
            "embedding_model": "all-MiniLM-L6-v2",
            "text_preview": text[:400],
        }
        records = [r for r in records if r["path"] != str(path)] + [rec]
        self._open_files.add(str(path))
        await self._save_index(records)
        await self._audit(f"indexed path={path}")

    async def _load_scope(self) -> list[str]:
        self.index_scope.parent.mkdir(parents=True, exist_ok=True)
        if not self.index_scope.exists():
            default = [str(self.home)]
            self.index_scope.write_text(json.dumps(default), encoding="utf-8")
            return default
        return json.loads(self.index_scope.read_text(encoding="utf-8"))

    async def _load_index(self) -> list[dict[str, Any]]:
        self.index_db.parent.mkdir(parents=True, exist_ok=True)
        if not self.index_db.exists():
            return []
        return json.loads(self.index_db.read_text(encoding="utf-8"))

    async def _save_index(self, payload: list[dict[str, Any]]) -> None:
        self.index_db.parent.mkdir(parents=True, exist_ok=True)
        self.index_db.write_text(json.dumps(payload), encoding="utf-8")

    async def _audit(self, message: str) -> None:
        self.audit_log.parent.mkdir(parents=True, exist_ok=True)
        now = datetime.now(timezone.utc).isoformat()
        with self.audit_log.open("a", encoding="utf-8") as f:
            f.write(f"{now} memory_agent {message}\n")

    def _temporal_boost(self, item: dict[str, Any]) -> float:
        age = max(1.0, datetime.now().timestamp() - float(item.get("modified_time", 0)))
        return min(0.25, 86_400.0 / age * 0.05)

    def _co_occurrence_boost(self, item: dict[str, Any]) -> float:
        overlap = len(set(item.get("co_opened_files", [])) & self._open_files)
        return min(0.2, overlap * 0.03)
