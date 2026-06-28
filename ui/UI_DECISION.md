# ui/ — COGNOS Wayland UI (Candidate B)

This directory contains a Wayland shell that hooks into an existing
compositor (e.g., sway) rather than being a full compositor itself.

## Status
Not in workspace. Needs Cargo.toml + dependencies before it can compile.

## Relationship to shell/
`shell/` implements a full Wayland compositor from scratch. This directory
takes a lighter approach — hook into an existing compositor and provide
COGNOS-specific surfaces (intent bar, agent status, memory browser).

**One of these must be chosen and the other removed before v0.1 ships.**
