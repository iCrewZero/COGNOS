#!/usr/bin/env python3
"""DispatchIntent probe against cognos-intent (prints result_json IntentSchema)."""
from __future__ import annotations

import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "agents"))

import grpc  # noqa: E402
from proto import cognos_pb2, cognos_pb2_grpc  # noqa: E402


def main() -> int:
    utterance = sys.argv[1] if len(sys.argv) > 1 else "crée un dossier test dans /tmp"
    endpoint = os.environ.get("COGNOS_INTENT_ENDPOINT", "127.0.0.1:7445")
    channel = grpc.insecure_channel(endpoint)
    stub = cognos_pb2_grpc.CognosIpcStub(channel)
    req = cognos_pb2.Intent(utterance=utterance)
    resp = stub.DispatchIntent(req, timeout=120)
    print(f"status={resp.status}")
    print(f"message={resp.message}")
    if resp.result_json:
        try:
            schema = json.loads(resp.result_json)
            print("--- IntentSchema (result_json) ---")
            print(json.dumps(schema, indent=2, ensure_ascii=False))
        except json.JSONDecodeError:
            print(resp.result_json.decode("utf-8", errors="replace"))
    if resp.action_graph and resp.action_graph.nodes:
        print(f"action_graph_nodes={len(resp.action_graph.nodes)}")
    return 0 if resp.status == "ok" else 1


if __name__ == "__main__":
    raise SystemExit(main())
