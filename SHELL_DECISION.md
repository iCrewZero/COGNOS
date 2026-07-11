# Shell Decision

## Decision
`GTK4-shell` is the chosen v0 shell implementation.

The `cognoS-shell` full compositor candidate is rejected for v0 and should not
receive new product work beyond archival or removal planning.

## Why
The decision is based on the requested criteria: toolkit maturity, delivery
effort, and Wayland integration risk.

### 1. Toolkit maturity
GTK4 is already a production-grade UI toolkit with stable widgets, input
handling, text entry, lists, dialogs, scrolling, styling, and accessibility.
Those primitives map directly to the required v0 features:

- top bar with status pills and resource stats
- intent bar with streaming output
- disambiguation dropdown
- toast notifications
- approval dialog
- memory browser with filters and destructive actions

By contrast, `cognoS-shell` currently implies building both shell behavior and a
large amount of widget/runtime infrastructure at the same time. In this repo,
the `ui/` Wayland client path and the `shell/` compositor path are both stubs,
but the GTK4 candidate already sketches concrete widget structure while the full
compositor path is still only lifecycle placeholders.

### 2. Effort to functional v0
The user asked for six functional items, each tested against the real local
services and demonstrated in QEMU. That favors the shortest path to a working
surface over architectural purity.

A GTK4 shell layered onto an existing Wayland compositor minimizes the amount of
new infrastructure we must own before the first feature works:

- no custom compositor bring-up
- no custom surface/input/focus policy for the whole desktop
- no compositor-grade damage, output, or seat management
- no need to debug shell protocol behavior and application behavior at once

This reduces time-to-first-integration for `DispatchIntent`, HAL approvals,
`StreamEvents`, and memory operations. It also makes the requested PR slicing
practical: each feature can ship as a user-visible, testable increment.

### 3. Wayland integration risk
For v0, COGNOS does not need to become the compositor. It needs trusted shell
surfaces on Wayland that can:

- stay visible as top/overlay UI
- accept text input
- show modal approval UI
- subscribe to local IPC/gRPC streams
- run inside the existing session in the VM/rootfs

That is better served by a Wayland client using layer-shell / compositor hooks
than by a full compositor rewrite. Running as a client keeps the integration
surface smaller and avoids making the entire desktop session depend on immature
compositor code.

### 4. Operational fit with the current repo
The repository already points in this direction:

- `ui/UI_DECISION.md` describes `ui/` as the lighter approach.
- `ui/wayland/shell.rs` is explicitly a Wayland client with layer-shell.
- `GTK4/all_widgets.rs` already models the top bar and memory-browser style of
  UI needed for the milestone, even if it is not yet wired to real services.
- `services/cognos-ui-agent.service` describes the UI agent as a session-facing
  shell surface host, not as the system compositor.

So the least disruptive path is:

1. keep the existing compositor/session
2. build the COGNOS shell UI as the `cognos-ui-agent`
3. wire it to real IPC/HAL/memory services

## What this means for implementation
For v0, "GTK4-shell" means:

- GTK4 provides the widget toolkit
- the UI agent runs as a Wayland client
- layer-shell / existing compositor integration is the Wayland strategy
- HAL approval UI and intent UI live in that client

It does **not** mean "static mockup app". Every requested item must still be
backed by the real local services.

## Explicit non-goal for v0
`cognoS-shell` as a full Wayland compositor is out of scope for this milestone.
It can be revisited later only if there is a concrete requirement that cannot be
met by the GTK4 + Wayland-client architecture.

## Decision summary
Choose `GTK4-shell` for v0 because it has the highest toolkit maturity, the
lowest implementation effort, and the smallest Wayland integration risk while
still satisfying the requirement for a real, service-backed shell in QEMU.
