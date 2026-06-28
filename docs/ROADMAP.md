# COGNOS/OS — Roadmap to First Boot
#
# This document lays out the phases needed to get COGNOS/OS from
# its current state (65 fixes applied, all core code compiles) to
# an actual running system on real hardware.
#
# Owner: iCrewZero
# Last updated: Phase 2 audit complete (65 total fixes)

## Current Status

- 9 core Rust crates compile (cargo check passes)
- 7 Python agents run (pytest passes)
- gRPC IPC layer is wired (6 RPCs, HMAC auth, health checks)
- Security hardening configs exist (nftables, AppArmor, cgroups)
- 8 systemd service units defined
- Test suite: 7 threat + integration tests

## Phase 1 — Foundation (DONE)

All 65 issues from Phase 1 (38 fixes) and Phase 2 (27 fixes) are resolved:
- Compile crashes eliminated
- gRPC wiring complete
- Python agent framework working
- Security configs real (not stubs)
- Binary entrypoints for all 5 core Rust services

## Phase 2 — IPC Integration (NEXT, 1-2 weeks)

These are the highest-priority items to make agents actually talk to each other.

### 2.1 Wire Python agents to gRPC server
- [ ] Compile cognos.proto to Python stubs (`python agents/generate_proto.py`)
- [ ] Test AgentIpcClient with real gRPC stubs (not fallback mode)
- [ ] Add retry/reconnect logic to Python client
- [ ] Add HMAC token creation to Python client (matches Rust auth module)
- [ ] Test coordinator → memory agent round-trip via IPC

### 2.2 Wire Rust services to IPC
- [ ] Add `cognos-ipc-grpc` as a dependency to orchestrator, scheduler, HAL
- [ ] Create CognosClient instances in each service's main.rs
- [ ] Register each service as an agent via IpcRuntime::register_agent
- [ ] Test orchestrator → HAL gate round-trip via IPC

### 2.3 End-to-end intent pipeline
- [ ] User types intent in shell
- [ ] Shell sends DispatchIntent to IPC server
- [ ] IPC server routes to orchestrator
- [ ] Orchestrator decomposes into DAG, dispatches to agents
- [ ] Each agent does its work, returns result through IPC
- [ ] Result flows back to shell/UI

### 2.4 Proto compilation in CI/build
- [ ] Add proto compilation step to build/Makefile
- [ ] Generate Python stubs as part of `make install`
- [ ] Verify Cargo.lock matches across all crates

## Phase 3 — LLM Integration (parallel with Phase 2, by other person)

The other team member is working on intent-module → LLM. Key integration points:

- [ ] LLM endpoint in intent-engine for natural language parsing
- [ ] Embedding model in memory agent (currently uses sentence-transformers)
- [ ] AI-assisted coding agent (currently stub)
- [ ] Cognitive preloader predictions for file pre-warming
- [ ] Fallback: what happens when LLM is unavailable (offline mode)

## Phase 4 — OS Image Build (2-3 weeks)

### 4.1 Kernel
- [ ] Verify kernel config against cognos_defconfig
- [ ] Build custom kernel with eBPF, cgroup v2, ANFS patches
- [ ] Package as .deb for the rootfs

### 4.2 Root filesystem
- [ ] Fix scripts/rootfs_builder.sh to install all service binaries
- [ ] Install Python agent framework and dependencies
- [ ] Install gRPC proto stubs (Python)
- [ ] Set up /etc/cognos/ configuration directory
- [ ] Set up /var/lib/cognos/ state directory
- [ ] Create `cognos` user/group with correct permissions

### 4.3 Systemd target
- [ ] Create cognos.target with correct dependency ordering
- [ ] Verify all 8 service units start in correct order:
  1. cognos-ipc.service (first — all agents depend on it)
  2. cognos-hal.service (second — gates all privileged actions)
  3. cognos-scheduler.service (resource management)
  4. cognos-memory.service (embedding index)
  5. cognos-orchestrator.service (task execution)
  6. cognos-intent.service (NLP parsing)
  7. cognos-agents.service (Python agent pool)
  8. cognos-ui-agent.service (display layer)

### 4.4 ISO
- [ ] Build bootable ISO with kernel + rootfs + GRUB
- [ ] Test in QEMU/KVM first
- [ ] Test on real hardware

## Phase 5 — Desktop Experience (3-4 weeks)

### 5.1 Shell (decide: cognoS-shell vs GTK4-shell)
- [ ] Resolve SHELL_DECISION.md — pick one
- [ ] Implement shell top bar (agent status dots, resource stats)
- [ ] Implement intent bar (user input area)
- [ ] Implement disambiguation UI (dropdown in intent bar)
- [ ] Implement notification toasts

### 5.2 Approval UI
- [ ] HAL approval dialog (block/allow/allow-with-notice)
- [ ] Coding agent diff viewer
- [ ] Security finding display

### 5.3 Memory Browser
- [ ] File list with domain filters
- [ ] Importance score sorting
- [ ] Session history viewer
- [ ] Forget/delete controls (consent management)

## Phase 6 — Hardening (ongoing)

- [ ] AppArmor profiles for each service (currently 2 of 8)
- [ ] nftables rules verified on real network
- [ ] eBPF scheduler telemetry (currently C stubs)
- [ ] LSTM predictor integration (currently C++ stubs)
- [ ] ANFS FUSE filesystem mount
- [ ] GPG provenance chain verification
- [ ] Audit chain immutability test
- [ ] CAPTCHA-style AI pause on first boot

## Risk Register

| Risk | Impact | Mitigation |
|------|--------|------------|
| LLM offline mode | Agents can't parse intents | Fallback to keyword classifier (already implemented in planner.py) |
| ChromaDB not available | No semantic memory | Fall back to grep-based file search |
| gRPC server down | All agents disconnected | Each agent has local fallback; systemd restarts IPC |
| Kernel modules missing | No eBPF telemetry | Scheduler falls back to /proc parsing (already implemented) |
| Disk full from embeddings | Memory agent bloat | 10MB file cap + consent scope limits |

## Metrics to Track

- Intent-to-response latency (target: <500ms for cached, <3s for uncached)
- Memory index size per domain
- Agent health (failure rate, restart count)
- HAL approval rate (what % of actions need human approval)
- Scheduler scenario switches per hour
