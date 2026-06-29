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

gRPC communication backbone. All inter-agent communication flows through
a single gRPC service (`CognosIpc`) with 6 RPCs:
- `DispatchIntent` — route parsed intents to target agents
- `QueryMemory` — vector + tag search against the memory fabric
- `HalGate` — request hardware actions through HAL
- `ResourceHint` — push scheduling hints to the scheduler daemon
- `Heartbeat` — liveness ping
- `StreamEvents` — server-streaming event subscription

Authentication uses HMAC-SHA256 tokens. Every RPC carries an `Envelope`
with trace_id, source, target, capability, and signature.

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
    → Intent Engine (parse + validate)
    → Coordinator (route to agents)
    → Agents (planner, memory, security, etc.)
    → HAL Gate (risk score + approve/deny)
    → File agent / system (execute with grant token)
```

## Security Model

Three layers of defense:
1. **nftables** — kernel drops unauthorized AI network egress
2. **AppArmor** — process confinement (no direct filesystem/network)
3. **HAL** — human-in-the-loop for any dangerous action

Even if an agent is fully compromised, it cannot:
- Access the network (nftables drops it)
- Modify HAL (AppArmor denies + HAL is immutable)
- Execute arbitrary code (AppArmor restricts + HAL gates)
