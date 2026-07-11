# shell/ — COGNOS Wayland Shell (Candidate A)

This directory contains the rejected v0 candidate: a full Wayland compositor
and shell implementation built directly against wayland-server / smithay.

## Decision
The v0 decision is documented in the repository root at `SHELL_DECISION.md`.
That document selects `GTK4-shell` as the shipping direction for v0.

## Status
Do not build new product features here for the current milestone.

This candidate remains useful only as historical context or as a later research
track if the GTK4 Wayland-client architecture proves insufficient.

## Relationship to ui/
`ui/` is the chosen direction for v0: a lighter shell layered onto an existing
Wayland compositor rather than replacing the compositor itself.
