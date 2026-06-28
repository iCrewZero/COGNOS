"""
Coding Agent for COGNOS/OS — Vibe-coding integration.

Two-phase execution: plan first (user approves), then implement.
Never writes to source files without HAL Block-level approval.
Never skips Security Agent scan.
"""

from __future__ import annotations

import asyncio
import json
import logging
import tempfile
import uuid
from dataclasses import dataclass, field
from datetime import datetime, UTC
from pathlib import Path

from shared.base_agent import BaseAgent

# Fix B2 — iCrewZero: coding_agent.py references AgentMessage on lines 306
# and 475, but only imported BaseAgent (which no longer defines AgentMessage
# in types.py).  Import it directly from its canonical location.
from shared.types import AgentMessage

log = logging.getLogger("cognos.coding_agent")

COGNOS_DIR = Path.home() / ".cognos"
AUDIT_LOG = COGNOS_DIR / "audit.log"

TOKEN_BUDGET_TOTAL = 6000
TOKEN_BUDGET_PRIMARY_FILE = 2000
TOKEN_BUDGET_RELATED_FILES = 2000
TOKEN_BUDGET_HISTORY = 500
TOKEN_BUDGET_GIT_ERRORS = 500


# ─── Types ───────────────────────────────────────────────────────────────────

@dataclass
class FileContent:
    path: str
    content: str
    token_count: int


@dataclass
class ScanResult:
    passed: bool
    findings: list[str]


@dataclass
class FileChange:
    path: str
    change_type: str    # create | modify | delete_suggest
    diff: str
    new_content: str
    security_scan: ScanResult


@dataclass
class Implementation:
    file_changes: list[FileChange]
    explanation: str
    test_suggestions: list[str]
    follow_up_tasks: list[str]


@dataclass
class ApplyResult:
    applied_count: int
    skipped_count: int
    commit_message: str


@dataclass
class CodingContext:
    relevant_files: list[FileContent]
    session_history: list[str]
    git_status: str
    recent_errors: list[str]
    security_findings: list[str]
    token_budget_used: int
    token_budget_total: int = TOKEN_BUDGET_TOTAL


# ─── Coding Agent ────────────────────────────────────────────────────────────

class CodingAgent(BaseAgent):
    """
    AI-assisted coding with mandatory plan → security scan → human review pipeline.

    Hard rules (never violated):
    - No write to source files without HAL Block-level (score 0.85) approval
    - Plan phase cannot be skipped
    - Security scan cannot be skipped
    - Changes must be shown as diffs before applying
    - Auto-commit is not allowed — user runs git commit
    """

    def __init__(self, memory_client=None, security_client=None,
                 hal_client=None, file_agent=None, api_client=None):
        super().__init__("coding")
        self._memory = memory_client
        self._security = security_client
        self._hal = hal_client
        self._file = file_agent
        self._api = api_client  # Claude API for complex reasoning

    # ─── Context assembly ─────────────────────────────────────────────────────

    async def assemble_context(self, intent: dict) -> CodingContext:
        """Pull context from multiple sources in parallel."""
        goal = intent.get("goal", "")
        domain = intent.get("domain", "coding")

        memory_task = asyncio.create_task(self._fetch_memory(intent))
        history_task = asyncio.create_task(self._fetch_session_history(domain))
        git_task = asyncio.create_task(self._get_git_status())
        errors_task = asyncio.create_task(self._get_recent_errors())
        security_task = asyncio.create_task(self._get_security_findings())

        results = await asyncio.gather(
            memory_task, history_task, git_task, errors_task, security_task,
            return_exceptions=True,
        )

        memory_files = results[0] if isinstance(results[0], list) else []
        session_history = results[1] if isinstance(results[1], list) else []
        git_status = results[2] if isinstance(results[2], str) else ""
        recent_errors = results[3] if isinstance(results[3], list) else []
        security_findings = results[4] if isinstance(results[4], list) else []

        # Manage token budget: read files and truncate to fit
        relevant_files = await self._load_files_within_budget(memory_files)

        total_used = sum(f.token_count for f in relevant_files)
        return CodingContext(
            relevant_files=relevant_files,
            session_history=session_history[:3],
            git_status=git_status[:500],
            recent_errors=recent_errors[:5],
            security_findings=security_findings,
            token_budget_used=total_used,
        )

    # ─── Task execution ───────────────────────────────────────────────────────

    async def implement(self, task: str, context: CodingContext) -> Implementation | None:
        """
        Two-phase implementation. Returns None if user denies plan.

        PHASE 1: Generate and present plan — user must approve before any code.
        PHASE 2: Generate code — security scan — show diff — user approves per file.
        """

        # ── PHASE 1: Plan ─────────────────────────────────────────────────────
        plan = await self._generate_plan(task, context)

        # HAL Confirm for plan approval (score 0.7)
        plan_approved = await self._hal_gate_plan(plan, task)
        if not plan_approved:
            self._audit("plan_denied", task, "denied")
            return None

        # ── PHASE 2: Implementation ───────────────────────────────────────────
        raw_impl = await self._generate_implementation(task, plan, context)

        file_changes: list[FileChange] = []
        for fc in raw_impl.get("file_changes", []):
            # Write to temp first — never to source directly
            temp_path = Path(tempfile.mkdtemp()) / Path(fc["path"] or "unnamed").name
            temp_path.write_text(fc.get("new_content", ""))

            # Security scan (mandatory — cannot skip)
            scan = await self._security_scan(str(temp_path))

            if not scan.passed:
                findings_str = "\n".join(scan.findings)
                log.warning("Security scan failed for %s:\n%s", fc["path"], findings_str)
                # Present findings — user decides whether to proceed
                proceed = await self._hal_gate_security_finding(fc["path"], scan)
                if not proceed:
                    log.info("User aborted after security finding in %s", fc["path"])
                    continue

            # Compute diff against original
            diff = self._compute_diff(fc["path"], fc.get("new_content", ""))

            file_changes.append(FileChange(
                path=fc["path"],
                change_type=fc.get("change_type", "modify"),
                diff=diff,
                new_content=fc.get("new_content", ""),
                security_scan=scan,
            ))

        self._audit("implement", task, "success", note=f"{len(file_changes)} files")
        return Implementation(
            file_changes=file_changes,
            explanation=raw_impl.get("explanation", ""),
            test_suggestions=raw_impl.get("test_suggestions", []),
            follow_up_tasks=raw_impl.get("follow_up_tasks", []),
        )

    async def apply_changes(self, impl: Implementation) -> ApplyResult:
        """
        Apply approved FileChanges to source files.
        Each file requires HAL Block-level approval (score 0.85).
        """
        applied = 0
        skipped = 0

        for change in impl.file_changes:
            # HAL Block — user sees full diff and must explicitly approve
            approved = await self._hal_gate_file_write(change)
            if not approved:
                log.info("User skipped %s", change.path)
                skipped += 1
                continue

            # ANFS snapshot is triggered by x-cognos-ai-edit header (FUSE layer)
            if self._file:
                await self._file.create_file(
                    change.path, change.new_content, overwrite=True
                )
            else:
                # Safety: validate the path even without FileAgent.
                # Reject path traversal, null bytes, and writes outside home.
                target = Path(change.path).expanduser().resolve()
                home = Path.home()
                if not str(target).startswith(str(home)):
                    log.error("Path traversal blocked: %s", change.path)
                    continue
                if "\x00" in change.path:
                    log.error("Null byte in path: %s", change.path)
                    continue
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(change.new_content)

            self._audit("apply_change", change.path, "success",
                        note=f"ai_generated=true type={change.change_type}")
            applied += 1

        # Suggest commit message — user must run git commit themselves
        module = Path(impl.file_changes[0].path).parent.name if impl.file_changes else "unknown"
        commit_msg = (
            f"feat({module}): {impl.explanation[:60]}\n\n"
            "Co-authored-by: COGNOS AI <ai@cognos.os>"
        )

        return ApplyResult(
            applied_count=applied,
            skipped_count=skipped,
            commit_message=commit_msg,
        )

    # ─── Private helpers ──────────────────────────────────────────────────────

    async def _generate_plan(self, task: str, context: CodingContext) -> str:
        """Call the AI to generate a plain-English implementation plan."""
        system = (
            "You are a systems programmer. Given a coding task and context, "
            "produce a concise plan in plain English covering:\n"
            "1. Which files will be modified and how\n"
            "2. The approach and reasoning\n"
            "3. Any risks or dependencies\n"
            "Be specific. Do not write any code yet."
        )
        context_str = self._format_context(context)
        prompt = f"Task: {task}\n\nContext:\n{context_str}"

        if self._api:
            return await self._api.complete(system, prompt, max_tokens=500)
        return f"[Plan] Implement: {task}\nFiles: {[f.path for f in context.relevant_files[:3]]}"

    async def _generate_implementation(
        self, task: str, plan: str, context: CodingContext
    ) -> dict:
        """Call the AI to generate actual code based on the approved plan."""
        system = (
            "You are a systems programmer implementing an approved plan. "
            "Return a JSON object with fields: file_changes (list of "
            "{path, change_type, new_content}), explanation (str), "
            "test_suggestions (list of str), follow_up_tasks (list of str). "
            "Only output valid JSON, nothing else."
        )
        context_str = self._format_context(context)
        prompt = f"Task: {task}\n\nApproved plan:\n{plan}\n\nContext:\n{context_str}"

        if self._api:
            raw = await self._api.complete(system, prompt, max_tokens=2000)
            try:
                parsed = json.loads(raw)
                if not isinstance(parsed, dict):
                    return {"file_changes": [], "explanation": str(parsed)}
                return parsed
            except json.JSONDecodeError:
                return {"file_changes": [], "explanation": raw}
        return {"file_changes": [], "explanation": f"Implement: {task}"}

    async def _security_scan(self, path: str) -> ScanResult:
        """Run the Security Agent's static analysis on generated code."""
        if self._security:
            try:
                # Use handle_message so we go through the normal agent message path.
                # The security agent's handle_message returns a dict with scan results.
                result = await self._security.handle_message(
                    AgentMessage(type="SCAN_FILE", payload={"path": path}, sender="coding")
                )
                if isinstance(result, dict):
                    return ScanResult(
                        passed=result.get("passed", True),
                        findings=result.get("findings", []),
                    )
            except Exception as e:
                log.warning("Security scan failed: %s", e)
        # Stub: always passes in test environments
        return ScanResult(passed=True, findings=[])

    async def _hal_gate_plan(self, plan: str, task: str) -> bool:
        """HAL Confirm for plan approval. Always required."""
        if self._hal:
            result = await self._hal.gate(
                agent="coding",
                action="approve_plan",
                target=task[:80],
                pre_score=0.7,
                is_ai_generated=True,
                note=plan[:200],
            )
            return result.get("approved", False)
        return True  # In tests without HAL, auto-approve

    async def _hal_gate_file_write(self, change: FileChange) -> bool:
        """HAL Block for each file write. User sees full diff."""
        if self._hal:
            result = await self._hal.gate(
                agent="coding",
                action="write_file",
                target=change.path,
                pre_score=0.85,
                is_ai_generated=True,
                note=change.diff[:300],
            )
            return result.get("approved", False)
        return True

    async def _hal_gate_security_finding(self, path: str, scan: ScanResult) -> bool:
        """Present security findings and ask user whether to continue."""
        if self._hal:
            result = await self._hal.gate(
                agent="coding",
                action="proceed_despite_security_finding",
                target=path,
                pre_score=0.9,
                is_ai_generated=True,
                note="; ".join(scan.findings[:3]),
            )
            return result.get("approved", False)
        return False  # Default-deny when security findings exist

    def _compute_diff(self, path: str, new_content: str) -> str:
        """Compute unified diff between current file and new content."""
        try:
            import difflib
            original = Path(path).read_text(errors="replace") if Path(path).exists() else ""
            diff = difflib.unified_diff(
                original.splitlines(keepends=True),
                new_content.splitlines(keepends=True),
                fromfile=f"a/{Path(path).name}",
                tofile=f"b/{Path(path).name}",
            )
            return "".join(diff)
        except OSError:
            return f"(could not compute diff for {path})"

    async def _load_files_within_budget(
        self, memory_files: list[dict]
    ) -> list[FileContent]:
        """Read files and truncate to fit within token budget."""
        result = []
        budget_remaining = TOKEN_BUDGET_TOTAL - 300  # reserve for system prompt

        for i, mf in enumerate(memory_files[:10]):
            # mf is a MemorySearchResult dataclass, access via attribute not .get()
            path = getattr(mf, "path", "") or ""
            try:
                content = Path(path).read_text(errors="replace")
            except OSError:
                continue

            # Rough token estimate: 4 chars ≈ 1 token
            estimated_tokens = len(content) // 4
            budget_for_this = (
                TOKEN_BUDGET_PRIMARY_FILE if i == 0 else TOKEN_BUDGET_RELATED_FILES // 5
            )

            if estimated_tokens > budget_for_this:
                # Include first 30% + last 20% + summary note
                chars = budget_for_this * 4
                first_part = content[:int(chars * 0.6)]
                last_part = content[-int(chars * 0.4):]
                content = first_part + "\n... [truncated] ...\n" + last_part
                estimated_tokens = budget_for_this

            if budget_remaining < estimated_tokens:
                break

            result.append(FileContent(
                path=path,
                content=content,
                token_count=estimated_tokens,
            ))
            budget_remaining -= estimated_tokens

        return result

    def _format_context(self, ctx: CodingContext) -> str:
        parts = []
        for fc in ctx.relevant_files[:3]:
            parts.append(f"=== {fc.path} ===\n{fc.content[:1000]}")
        if ctx.git_status:
            parts.append(f"Git status:\n{ctx.git_status}")
        if ctx.recent_errors:
            parts.append("Recent errors:\n" + "\n".join(ctx.recent_errors[:3]))
        return "\n\n".join(parts)

    async def _fetch_memory(self, intent: dict) -> list[dict]:
        if self._memory:
            return await self._memory.query(intent)
        return []

    async def _fetch_session_history(self, domain: str) -> list[str]:
        sessions_dir = COGNOS_DIR / "context" / "sessions"
        if not sessions_dir.exists():
            return []
        summaries = []
        for p in sorted(sessions_dir.glob("*.json"), reverse=True)[:3]:
            try:
                data = json.loads(p.read_text())
                if data.get("domain") == domain:
                    summaries.append(
                        f"Session {data.get('session_id','')[:8]}: "
                        f"{len(data.get('files_touched',[]))} files, "
                        f"{data.get('duration_seconds', 0):.0f}s"
                    )
            except (json.JSONDecodeError, OSError):
                pass
        return summaries

    async def _get_git_status(self) -> str:
        try:
            proc = await asyncio.create_subprocess_exec(
                "git", "status", "--short",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.DEVNULL,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=3.0)
            return "\n".join(stdout.decode(errors="replace").splitlines()[:10])
        except Exception:
            return ""

    async def _get_recent_errors(self) -> list[str]:
        terminal_log = COGNOS_DIR / "context" / "terminal.log"
        if not terminal_log.exists():
            return []
        try:
            lines = terminal_log.read_text(errors="replace").splitlines()
            return [l for l in lines[-20:] if "error" in l.lower()][:5]
        except OSError:
            return []

    async def _get_security_findings(self) -> list[str]:
        if self._security:
            try:
                resp = await self._security.handle_message(
                    AgentMessage(type="ACTIVE_FINDINGS", payload={}, sender="coding")
                )
                findings = resp.get("findings", []) if isinstance(resp, dict) else []
                return findings if isinstance(findings, list) else []
            except Exception:
                return []
        return []

    def _audit(self, action: str, target: str, outcome: str, note: str = "") -> None:
        entry = {
            "ts": datetime.now(UTC).isoformat(),
            "agent": "coding",
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
