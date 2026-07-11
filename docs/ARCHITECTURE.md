# COGNOS/OS Architecture

This document describes the high-level architecture of COGNOS/OS, an
AI-governed Linux distribution where every privileged AI action must pass
through a human approval layer (HAL) before execution.

Owner: iCrewZero

## Core Components

### 1. HAL (Human Approval Layer) — `hal/`

The immutability root of the OS. HAL sits between AI agents and hardware.
Every privileged action — file deletion, network access, process spawning —
must pass through a HAL gate that computes a risk score and either auto-approves,
asks the user, or denies the action.

Key modules:
- `risk_scorer` — deterministic formula: R(A) = sum of weighted component scores
- `action_validator` — checks against dangerous-path rules
- `policy_engine` — produces the final granted/denied/approval_required decision
- `approval_flow` — Unix socket daemon for user approval UI
- `trust_calibration` — per-user interrupt thresholds that adapt over time
- `provenance` — GPG signature and hash-chain verification

### 2. Intent Engine — `intent-engine/`

Parses user intent (text, voice, structured) into a validated action graph.
The action graph is consumed by the orchestrator. The intent engine handles:
- Natural language parsing and keyword-based action classification
- Disambiguation detection (ambiguous references → ask user)
- Schema validation against the canonical IntentSchema
- Action graph construction with dependency ordering

### 3. Orchestrator — `orchestrator/`

Takes validated action graphs from the intent engine and expands them into
multi-agent task graphs. Each node in the graph is assigned to an agent,
has capability requirements, and has dependencies on other nodes.

Key modules:
- `task_graph` — DAG of TaskNodes with dependency edges
- `event_bus` — broadcast channel for task lifecycle events
- `runtime` — drives the graph: schedule ready nodes, wait for completion
- `scheduler` — maps task requirements to agent availability

### 4. IPC Layer — `ipc/grpc/`

gRPC communication backbone. All agents speak the **same wire protocol**
(`CognosIpc`, six RPCs, HMAC-SHA256 `Envelope` on every call) but v1 deploys
**several listeners** in a point-to-point mesh — not one process that
implements every RPC.

| Listener | Binary / unit | Default bind | Env (client) | Env (bind) |
|----------|---------------|--------------|--------------|------------|
| Central IPC bus | `cognos-ipc-server` | `127.0.0.1:7443` | `COGNOS_IPC_ENDPOINT` | `COGNOS_IPC_BIND` |
| HAL gate | `cognos-hal` | `127.0.0.1:7444` | `COGNOS_HAL_ENDPOINT` | `COGNOS_HAL_BIND` |
| Intent parsing | `cognos-intent` | `127.0.0.1:7445` | `COGNOS_INTENT_ENDPOINT` | `COGNOS_INTENT_BIND` |

RPC surface (shared proto `ipc/grpc/proto/cognos.proto`):

- `DispatchIntent` — parse user utterance → `IntentActionGraph` (**real impl:
  `cognos-intent` only**)
- `QueryMemory` — vector + tag search (**central bus**; echo responder until
  memory is wired)
- `HalGate` — risk score + grant/deny (**real impl: `cognos-hal` only**)
- `ResourceHint` — scheduling hints (**central bus** → event stream)
- `Heartbeat` — liveness + agent registration (**central bus**)
- `StreamEvents` — server-streaming event subscription (**central bus**)

Daemons register on the central bus (`COGNOS_IPC_ENDPOINT`) and heartbeat there.
Callers that need **real** gate decisions or parsed action graphs dial the
dedicated endpoints directly — the orchestrator uses `COGNOS_HAL_ENDPOINT` and
`COGNOS_INTENT_ENDPOINT` for this. Pulling HAL or the intent-engine into
`cognos-ipc-grpc` would create dependency cycles, so each crate hosts its own
`CognosServer` with an injected handler (`HalGateHandler`, `IntentHandler`).

**IPC mesh topology (v1 decision).** One proto, multiple gRPC listeners: the
central `cognos-ipc-server` is the event bus and agent registry (`Heartbeat`,
`StreamEvents`, `ResourceHint`, `QueryMemory`); `cognos-hal` on `:7444` is the
**only** authoritative `HalGate` implementation; `cognos-intent` on `:7445` is
the **only** authoritative `DispatchIntent` implementation. The central listener
still exposes `HalGate` and `DispatchIntent` for proto compatibility, but
without an injected handler it **must not** return plausible success-shaped
answers (`granted`, `pending`, `approval_required` that look like real work) —
it returns `status = "failed"` with an explicit `message` naming the correct
endpoint (`COGNOS_HAL_ENDPOINT` / `COGNOS_INTENT_ENDPOINT`). Systemd units,
the CLI, and UI code must target `:7444` / `:7445` for gate and intent
parsing; `:7443` is the bus, not a shortcut to HAL or the intent-engine.

### 5. Scheduler — `scheduler/`

Adaptive resource scheduler that controls how much CPU, memory, IO, and
GPU the AI processes can use. Uses cgroup v2 to enforce hard limits.

Key modules:
- `telemetry` — reads /proc and /sys for system metrics
- `resource_policy` — bounded policy adjustments (weights stay in allowed range)
- `daemon` — main loop: read telemetry → detect scenario → apply policy
- `predictor` — LSTM model for workload prediction (v1)

### 6. Memory — `memory/`

Consent-scoped, inspectable, deletable semantic memory. Anti-Recall rules
mean the user can always see, edit, or wipe their memory.

Key modules:
- `embedder` — trait for embedding models (v0: hash-based; v1: sentence-transformers)
- `indexer` — consumes the file index queue, embeds, stores
- `query` — cosine similarity top-k search
- `chromadb/` — ChromaDB integration for vector search
- `fabric/` — secure memory allocator (mlock, no swap for secrets)

### 7. Python Agents — `agents/`

Agent framework running in a Python venv. Key agents:
- `coordinator.py` — central orchestrator, routes intents to agents
- `planner.py` — decomposes intents into action sequences
- `memory.py` — semantic memory operations (ChromaDB + embeddings)
- `security.py` — security scanning and trust verification
- `scheduler.py` — scheduling hints and resource requests
- `coding_agent.py` — code generation and modification
- `file_agent.py` — filesystem operations
- `ui_agent.py` — system metrics and desktop integration

### 8. CLI — `cli/`

User-facing command-line interface with subcommands:
- `cognos intent` — submit a natural-language intent
- `cognos approval` — list/approve/deny HAL approvals
- `cognos memory` — search, list, edit, or forget memories
- `cognos status` — show agent statuses and system metrics
- `cognos tui` — interactive terminal UI

### 9. Security Infrastructure — `security/`

- `nftables/` — kernel-level network isolation for AI agents
- `apparmor/` — process confinement profiles
- `cgroups/` — resource limit slice for all AI processes

## Data Flow

```
User utterance
    → Intent Engine (:7445 DispatchIntent) — parse + validate → action graph
    → Orchestrator — expand graph into TaskNodes, schedule agents
    → Agents (planner, memory, security, file, …)
    → HAL Gate (:7444 HalGate) — risk score + approve/deny
    → File agent / system (execute with grant token)
```

Agent registration and cross-cutting events flow through the central IPC bus
(`:7443`): heartbeats, `StreamEvents`, `ResourceHint`, `QueryMemory`.

## Security Model

Three layers of defense:
1. **nftables** — kernel drops unauthorized AI network egress
2. **AppArmor** — process confinement (no direct filesystem/network)
3. **HAL** — human-in-the-loop for any dangerous action

Even if an agent is fully compromised, it cannot:
- Access the network (nftables drops it)
- Modify HAL (AppArmor denies + HAL is immutable)
- Execute arbitrary code (AppArmor restricts + HAL gates)
