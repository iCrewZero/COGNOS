#!/usr/bin/env python3
"""
One-shot file agent executor invoked by the Rust orchestrator.

Reads a JSON request on stdin:
  {"action": "create_dir", "target": "/tmp/test", "grant_token": "...", "trace_id": "..."}

Writes JSON to stdout:
  {"success": true, "message": "...", "hal_status": "granted", "hal_risk_score": 0.1}
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import sys
import time

logging.basicConfig(level=logging.INFO, format="%(message)s")
log = logging.getLogger("cognos.file_task")


class HalIpcWrapper:
    """Adapt AgentIpcClient.hal_gate() to the dict shape FileAgent expects."""

    def __init__(self, endpoint: str, secret: str):
        self._endpoint = endpoint
        self._secret = secret
        self._client = None

    async def _client_or_connect(self):
        if self._client is None:
            from shared.ipc import AgentIpcClient

            self._client = AgentIpcClient(
                "agent.file",
                endpoint=self._endpoint,
                signing_secret=self._secret,
            )
            await self._client.connect()
        return self._client

    async def gate(
        self,
        agent: str,
        action: str,
        target: str,
        pre_score: float,
        is_ai_generated: bool,
    ) -> dict:
        client = await self._client_or_connect()
        resp = await client.hal_gate(
            op=action,
            device=target,
            capability="file.write",
            allow_approval=True,
        )
        status = resp.get("status", "failed")
        return {
            "approved": status == "granted",
            "status": status,
            "risk_score": resp.get("risk_score", 0.0),
            "grant_token": resp.get("grant_token", ""),
        }


async def run_task(req: dict) -> dict:
    from file_agent import FileAgent

    trace_id = req.get("trace_id", "")
    action = req.get("action", "")
    target = req.get("target", "")
    started = time.perf_counter()

    hal = None
    hal_endpoint = os.environ.get("COGNOS_HAL_ENDPOINT", "http://127.0.0.1:7444")
    secret = os.environ.get("COGNOS_IPC_SECRET", "")
    if os.environ.get("COGNOS_FILE_AGENT_HAL", "0") not in ("0", "false", "False"):
        hal = HalIpcWrapper(hal_endpoint, secret)

    agent = FileAgent(hal_client=hal)

    if action == "create_dir":
        result = await agent.create_directory(target)
    elif action == "create_file":
        result = await agent.create_file(target)
    else:
        return {"success": False, "message": f"unsupported action: {action}"}

    out = {
        "success": result.success,
        "message": result.message,
    }
    if result.extra.get("hal_status"):
        out["hal_status"] = result.extra["hal_status"]
        out["hal_risk_score"] = result.extra.get("hal_risk_score")

    elapsed_ms = round((time.perf_counter() - started) * 1000.0, 2)
    log.info(
        json.dumps(
            {
                "event": "pipeline_stage",
                "stage": "execution",
                "trace_id": trace_id,
                "action": action,
                "target": target,
                "latency_ms": elapsed_ms,
                "success": out.get("success", False),
            }
        )
    )
    return out


def main() -> int:
    raw = sys.stdin.read()
    req = json.loads(raw) if raw.strip() else {}
    try:
        result = asyncio.run(run_task(req))
    except Exception as e:
        log.exception("file_task failed")
        result = {"success": False, "message": str(e)}
    print(json.dumps(result))
    return 0 if result.get("success") else 1


if __name__ == "__main__":
    raise SystemExit(main())
