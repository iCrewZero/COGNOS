"""
Memory Agent for COGNOS/OS.

Handles all semantic memory operations: indexing files at idle time,
answering semantic queries, tracking session state, and respecting consent scope.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import time
from dataclasses import dataclass, field, asdict
from datetime import datetime, UTC
from pathlib import Path
from typing import Any

import chromadb
from chromadb.config import Settings
from sentence_transformers import SentenceTransformer

from shared.base_agent import BaseAgent
from shared.types import AgentMessage

log = logging.getLogger("cognos.memory")

COGNOS_DIR = Path.home() / ".cognos"
MEMORY_DIR = COGNOS_DIR / "memory"
CONTEXT_DIR = COGNOS_DIR / "context" / "sessions"
AUDIT_LOG = COGNOS_DIR / "audit.log"
INDEX_QUEUE = MEMORY_DIR / "index_queue"
INDEX_SCOPE_FILE = MEMORY_DIR / "index_scope.json"

# Files and dirs we never index
NEVER_INDEX_PATTERNS = [
    re.compile(p) for p in [
        r"\.cognos/",
        r"^/tmp",
        r"/node_modules/",
        r"/\.git/objects/",
        r"__pycache__",
        r"\.pyc$",
    ]
]

MAX_FILE_SIZE_BYTES = 10 * 1024 * 1024  # 10 MB for binary-guard
BINARY_EXTENSIONS = {
    ".exe", ".dll", ".so", ".dylib", ".bin", ".img",
    ".iso", ".tar", ".gz", ".zip", ".rar", ".7z",
    ".mp4", ".mp3", ".mkv", ".avi", ".mov", ".flac", ".wav",
    ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp",
    ".pdf",  # index separately if needed
}


# ─── Data structures ─────────────────────────────────────────────────────────

@dataclass
class MemoryResult:
    path: str
    relevance_score: float
    last_session_summary: str
    cursor_position: dict | None
    co_opened_files: list[str]


@dataclass
class SessionSummary:
    session_id: str
    domain: str
    files_touched: list[str]
    duration_seconds: float
    last_errors: list[str]
    git_status: str
    started_at: str
    ended_at: str


@dataclass
class FileMetadata:
    path: str
    filename: str
    modified_time: float
    size: int
    last_session_id: str | None
    co_opened_files: list[str]
    project_domain: str
    importance_score: float


# ─── Agent ───────────────────────────────────────────────────────────────────

class MemoryAgent(BaseAgent):
    """
    Semantic memory for COGNOS/OS. Indexes files, answers queries.

    Runs two concurrent loops:
      - _indexer_loop: watches the index queue and indexes files at idle
      - _session_tracker: tracks open files in the current session
    """

    def __init__(self):
        super().__init__("memory")
        MEMORY_DIR.mkdir(parents=True, exist_ok=True)
        CONTEXT_DIR.mkdir(parents=True, exist_ok=True)

        self._chroma = chromadb.PersistentClient(
            path=str(MEMORY_DIR / "chromadb"),
            settings=Settings(anonymized_telemetry=False),
        )
        self._collection = self._chroma.get_or_create_collection(
            name="cognos_files",
            metadata={"hnsw:space": "cosine"},
        )

        # Load embedding model (22 MB, fast)
        self._embedder = SentenceTransformer("all-MiniLM-L6-v2")

        # Current session state
        self._session_id: str = _new_uuid()
        self._session_files: list[str] = []
        self._session_start = time.time()
        self._cursor_positions: dict[str, dict] = {}

        # Index scope: which paths we're allowed to index
        self._index_scope: list[str] = self._load_scope()

        # No-index env overrides
        self._no_index = set(
            p.strip()
            for p in os.environ.get("COGNOS_NO_INDEX", "").split(":")
            if p.strip()
        )

    # ─── Public API ──────────────────────────────────────────────────────────

    async def query(self, intent_schema: dict) -> list[MemoryResult]:
        """
        Answer a semantic query from the orchestrator.
        Returns up to 5 ranked results with context.
        """
        goal = intent_schema.get("goal", "")
        domain = intent_schema.get("domain") or ""
        query_text = f"{goal} {domain}".strip()

        if not query_text:
            return []

        embedding = await asyncio.get_event_loop().run_in_executor(
            None, self._embedder.encode, query_text
        )

        results = self._collection.query(
            query_embeddings=[embedding.tolist()],
            n_results=10,
            include=["metadatas", "distances"],
        )

        items = []
        for meta, dist in zip(
            results["metadatas"][0], results["distances"][0]
        ):
            relevance = 1.0 - float(dist)  # cosine: 0=identical, 2=opposite
            relevance = max(0.0, min(1.0, relevance))

            # Temporal boost: recency bonus up to 0.15
            modified_time = float(meta.get("modified_time", 0))
            age_days = (time.time() - modified_time) / 86400
            recency_boost = max(0.0, 0.15 * (1.0 - min(age_days / 30, 1.0)))

            # Co-occurrence boost: was this file open in the same session?
            co_files = json.loads(meta.get("co_opened_files", "[]"))
            co_boost = 0.1 if any(f in self._session_files for f in co_files) else 0.0

            final_score = min(1.0, relevance + recency_boost + co_boost)

            items.append(MemoryResult(
                path=meta["path"],
                relevance_score=final_score,
                last_session_summary=meta.get("last_session_summary", ""),
                cursor_position=None,  # enriched below
                co_opened_files=co_files,
            ))

        items.sort(key=lambda r: r.relevance_score, reverse=True)
        top = items[:5]

        # Enrich with cursor positions if we have them
        for r in top:
            r.cursor_position = self._cursor_positions.get(r.path)

        self._audit("query", goal, {"count": len(top)})
        return top

    async def show_indexed(self) -> list[str]:
        """Return list of all indexed file paths for user inspection."""
        results = self._collection.get(include=["metadatas"])
        return [m["path"] for m in results["metadatas"]]

    async def forget(self, scope: str) -> None:
        """
        Delete embeddings for a given scope.
        scope can be a path prefix or domain name.
        """
        all_results = self._collection.get(include=["metadatas"])
        ids_to_delete = []
        for doc_id, meta in zip(all_results["ids"], all_results["metadatas"]):
            if meta.get("path", "").startswith(scope) or meta.get("project_domain") == scope:
                ids_to_delete.append(doc_id)

        if ids_to_delete:
            self._collection.delete(ids=ids_to_delete)
            self._audit("forget", scope, {"deleted_count": len(ids_to_delete)})
            log.info("Forgot %d embeddings for scope '%s'", len(ids_to_delete), scope)

    def record_file_open(self, path: str) -> None:
        """Called by the ANFS bridge or UI when a file is opened."""
        abs_path = str(Path(path).expanduser().resolve())
        if abs_path not in self._session_files:
            self._session_files.append(abs_path)

    def record_cursor_position(self, path: str, line: int, col: int) -> None:
        """Track cursor position for context restoration."""
        self._cursor_positions[path] = {"line": line, "col": col}

    async def end_session(self, domain: str = "unknown", errors: list[str] = None) -> None:
        """Write a session summary on session end."""
        duration = time.time() - self._session_start
        git_status = await _get_git_status()

        summary = SessionSummary(
            session_id=self._session_id,
            domain=domain,
            files_touched=list(self._session_files),
            duration_seconds=duration,
            last_errors=errors or [],
            git_status=git_status,
            started_at=datetime.fromtimestamp(self._session_start, UTC).isoformat(),
            ended_at=datetime.now(UTC).isoformat(),
        )

        summary_path = CONTEXT_DIR / f"{self._session_id}.json"
        summary_path.write_text(json.dumps(asdict(summary), indent=2))

        # Update co_opened_files in ChromaDB for all touched files
        await self._update_co_opened_files(self._session_files, self._session_id)

        self._session_id = _new_uuid()
        self._session_files = []
        self._session_start = time.time()

    # ─── Indexer loop ─────────────────────────────────────────────────────────

    async def _indexer_loop(self) -> None:
        """
        Watches the index queue and indexes files at idle time.
        Yields frequently to stay within the 10% CPU cgroup limit.
        """
        INDEX_QUEUE.parent.mkdir(parents=True, exist_ok=True)
        processed: set[str] = set()

        while True:
            await asyncio.sleep(5)  # check queue every 5s

            if not INDEX_QUEUE.exists():
                continue

            # Drain the queue
            try:
                lines = INDEX_QUEUE.read_text().splitlines()
                INDEX_QUEUE.write_text("")  # clear
            except OSError:
                continue

            for path_str in lines:
                path_str = path_str.strip()
                if not path_str or path_str in processed:
                    continue
                processed.add(path_str)

                await self._index_file(Path(path_str))
                await asyncio.sleep(0.05)  # yield — stay within CPU budget

    async def _index_file(self, path: Path) -> None:
        """Index a single file into ChromaDB. Skips excluded paths."""
        try:
            abs_path = str(path.expanduser().resolve())
        except OSError:
            return

        if not self._is_indexable(abs_path):
            return

        try:
            stat = path.stat()
        except OSError:
            return

        # Skip large or binary files
        if stat.st_size > MAX_FILE_SIZE_BYTES:
            return
        if path.suffix.lower() in BINARY_EXTENSIONS:
            return

        # Check if already indexed with same mtime
        existing = self._collection.get(
            ids=[abs_path], include=["metadatas"]
        )
        if existing["ids"]:
            stored_mtime = float(existing["metadatas"][0].get("modified_time", 0))
            if abs(stored_mtime - stat.st_mtime) < 1.0:
                return  # unchanged

        # Read content
        try:
            content = path.read_text(errors="replace")[:50_000]  # cap at 50K chars
        except OSError:
            return

        if not content.strip():
            return

        # Generate embedding in thread pool to not block event loop
        embedding = await asyncio.get_event_loop().run_in_executor(
            None, self._embedder.encode, content[:2000]  # embed first 2KB
        )

        importance = self._compute_importance(abs_path, stat)
        domain = self._infer_domain(abs_path, content)

        metadata: dict[str, Any] = {
            "path": abs_path,
            "filename": path.name,
            "modified_time": stat.st_mtime,
            "size": stat.st_size,
            "last_session_id": self._session_id,
            "co_opened_files": json.dumps([]),
            "project_domain": domain,
            "importance_score": importance,
            "last_session_summary": "",
        }

        self._collection.upsert(
            ids=[abs_path],
            embeddings=[embedding.tolist()],
            metadatas=[metadata],
        )

        self._audit("index_file", abs_path, {"domain": domain, "size": stat.st_size})

    def _is_indexable(self, abs_path: str) -> bool:
        """Check path against scope and exclusion rules."""
        # Must be within scope
        in_scope = any(abs_path.startswith(s) for s in self._index_scope)
        if not in_scope:
            return False

        # Must not be in no-index list
        for no_idx in self._no_index:
            if abs_path.startswith(no_idx):
                return False

        # Must not match never-index patterns
        for pattern in NEVER_INDEX_PATTERNS:
            if pattern.search(abs_path):
                return False

        return True

    def _compute_importance(self, path: str, stat: os.stat_result) -> float:
        """
        importance = 0.3*access_frequency + 0.4*recency + 0.3*session_depth
        """
        existing = self._collection.get(ids=[path], include=["metadatas"])
        if existing["ids"]:
            meta = existing["metadatas"][0]
            access_count = float(meta.get("access_count", 0))
            session_depth = float(meta.get("session_count", 0))
        else:
            access_count = 0.0
            session_depth = 0.0

        # Normalize frequency (log scale, cap at 100)
        freq_norm = min(1.0, access_count / 100.0)

        # Recency: 0 = just now, 1 = very old (>30 days)
        age_days = (time.time() - stat.st_mtime) / 86400
        recency = max(0.0, 1.0 - min(age_days / 30, 1.0))

        # Session depth (normalized, cap at 20)
        depth_norm = min(1.0, session_depth / 20.0)

        return 0.3 * freq_norm + 0.4 * recency + 0.3 * depth_norm

    def _infer_domain(self, path: str, content: str) -> str:
        """Heuristically infer the domain of a file from path and content."""
        path_lower = path.lower()
        if any(k in path_lower for k in ["robot", "motor", "pid", "arduino", "firmware"]):
            return "robotics"
        if any(k in path_lower for k in [".rs", ".py", ".js", ".ts", ".go", ".cpp", ".c", "src/"]):
            return "coding"
        if any(k in path_lower for k in [".md", ".txt", ".rst", "docs/", "notes/"]):
            return "writing"
        if any(k in path_lower for k in ["research", "paper", "journal", "study"]):
            return "research"
        return "other"

    async def _update_co_opened_files(
        self, session_files: list[str], session_id: str
    ) -> None:
        """Update the co_opened_files field for all files in a session."""
        for path in session_files:
            existing = self._collection.get(ids=[path], include=["metadatas"])
            if not existing["ids"]:
                continue

            meta = existing["metadatas"][0]
            co = set(json.loads(meta.get("co_opened_files", "[]")))
            co.update(f for f in session_files if f != path)
            co = list(co)[:20]  # cap list size

            self._collection.update(
                ids=[path],
                metadatas=[{**meta, "co_opened_files": json.dumps(co),
                            "last_session_id": session_id}],
            )

    # ─── Scope management ────────────────────────────────────────────────────

    def _load_scope(self) -> list[str]:
        """Load or create the index scope config."""
        default = [str(Path.home())]
        if INDEX_SCOPE_FILE.exists():
            try:
                data = json.loads(INDEX_SCOPE_FILE.read_text())
                return data.get("paths", default)
            except (json.JSONDecodeError, OSError):
                pass
        self._save_scope(default)
        return default

    def _save_scope(self, paths: list[str]) -> None:
        INDEX_SCOPE_FILE.parent.mkdir(parents=True, exist_ok=True)
        INDEX_SCOPE_FILE.write_text(json.dumps({"paths": paths}, indent=2))

    # ─── Agent message dispatch ───────────────────────────────────────────────

    async def handle_message(self, msg: AgentMessage) -> Any:
        """Route incoming orchestrator messages."""
        if msg.type == "MEMORY_QUERY":
            return await self.query(msg.payload)
        if msg.type == "SESSION_END":
            await self.end_session(
                domain=msg.payload.get("domain", "unknown"),
                errors=msg.payload.get("errors", []),
            )
        if msg.type == "FORGET":
            await self.forget(msg.payload["scope"])

    async def run(self) -> None:
        """Start the agent and its background indexer."""
        log.info("Memory Agent starting, session=%s", self._session_id)
        await asyncio.gather(
            super().run(),
            self._indexer_loop(),
        )

    # ─── Audit ───────────────────────────────────────────────────────────────

    def _audit(self, action: str, target: str, extra: dict | None = None) -> None:
        entry: dict[str, Any] = {
            "ts": datetime.now(UTC).isoformat(),
            "agent": "memory",
            "action": action,
            "target": target,
            "outcome": "success",
        }
        if extra:
            entry.update(extra)
        try:
            AUDIT_LOG.parent.mkdir(parents=True, exist_ok=True)
            with open(AUDIT_LOG, "a") as f:
                f.write(json.dumps(entry) + "\n")
        except OSError as e:
            log.error("Audit write failed: %s", e)


# ─── Helpers ─────────────────────────────────────────────────────────────────

def _new_uuid() -> str:
    import uuid
    return str(uuid.uuid4())


async def _get_git_status() -> str:
    try:
        proc = await asyncio.create_subprocess_exec(
            "git", "status", "--short",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
        )
        stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=3.0)
        lines = stdout.decode(errors="replace").splitlines()[:10]
        return "\n".join(lines)
    except (OSError, asyncio.TimeoutError):
        return ""
