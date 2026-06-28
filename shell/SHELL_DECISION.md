# shell/ — COGNOS Wayland Shell (Candidate A)

This directory contains a Wayland compositor and shell implementation
built directly against wayland-server / smithay.

## Status
Not in workspace. Needs Cargo.toml + dependencies before it can compile.

## Relationship to ui/
`ui/` is a competing implementation that uses compositor hooks instead of
a full compositor. See `ui/UI_DECISION.md` for the alternative approach.

**One of these must be chosen and the other removed before v0.1 ships.**
