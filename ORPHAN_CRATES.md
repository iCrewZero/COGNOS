# Orphan Crates — Not Yet in Workspace

These directories contain Rust source code but are NOT members of the
Cargo workspace. They need Cargo.toml files, dependency resolution, and
integration testing before they can be added.

Adding a broken crate to the workspace would break `cargo check` for
everyone, so they stay out until someone adopts them.

| Directory | Purpose | What it needs to join |
|-----------|---------|----------------------|
| `shell/` | Wayland shell (compositor, intent bar, widgets) | smithay/wayland deps, binary target |
| `ui/` | Alternative Wayland UI (compositor hooks, widgets) | Same as shell — pick ONE |
| `hypervisor/` | Authority VM isolation | kvm, vm-memory, likely a separate binary |
| `installer/` | Package/install sandboxing | seccomp deps, binary target |
| `governance/` | Formal verification + policy compilation | SMT solver dep (z3) |
| `mesh/` | Agent mesh networking | libp2p or quic deps |
| `cluster/` | Multi-node consensus | raft/consensus dep |
| `llm/` | llama.cpp inference bindings | llama-cpp-sys bindings |
| `unipkg/` | Universal package manager | Multiple backend deps |
| `tokenizer/` | Tokenizer utilities | tiktoken or huggingface tokenizers |
| `anfs/` | Agent-native filesystem | FUSE3 dep |
| `cognos/` | CLI binary (duplicate of `cli/`) | Likely should be REMOVED — cli/ is canonical |
| `terminal/` | Shell assist | Separate binary or merge into shell/ |
| `GTK4/` | GTK4 widget demos | gtk4-rs deps |

## Decision needed
- `shell/` vs `ui/`: Two competing Wayland shells. Pick one and delete the other.
- `cognos/` vs `cli/`: `cli/` is the canonical CLI. `cognos/cli/main.rs` is a duplicate.

Owner: iCrewZero
