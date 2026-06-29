"""
COGNOS/OS — shared utilities for the Python agent framework.

This package provides:
- base_agent: BaseAgent class all agents inherit from.
- types: Shared type definitions (AgentMessage, IntentSchema, etc.)
- ipc: AgentIpcClient — the gRPC client that talks to the Rust IPC server.
- capability_lattice: Capability lattice for agent permission checks.

Owner: iCrewZero
"""
