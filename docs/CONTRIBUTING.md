
---

## Contributing

The barrier to contribution is intentionally low. You do not need to be a kernel hacker.

**How to contribute:**

1. Pick a module from the [roadmap](docs/SPEC.md#development-roadmap)
2. Read the relevant prompt in [docs/PROMPTS.md](docs/PROMPTS.md)
3. Describe the intended behavior in plain English in an issue
4. Use Claude or your preferred AI to generate an implementation
5. Run the test suite
6. Open a PR — human review required before merge

**Hard rules:**
- HAL changes: maintainer review only, no exceptions
- No AI co-author tags on any file in `hal/`
- Every PR needs a GPG-signed by iCrewZero description of what changed and why
- Do not merge with failing CI checks

**Current contributors:**
- [iCrewZero] (https://github.com/iCrewZero) — founder, architecture, HAL
- [yannbellec] (https://github.com/yannbellec) — LSTM/ONNX, safety systems, CI

---

## Documentation

- [Full Specification](docs/SPEC.md) — complete architecture and design
- [Formal Models](docs/FORMAL_MODELS.md) — HAL risk model, intent schema, capability lattice, threat model
- [Contributing Guide](docs/CONTRIBUTING.md) — vibe-coding workflow
- [HAL Audit Log](docs/HAL_AUDIT.md) — record of all HAL changes and reviews

---

## License

GPL-3.0 — this project is and will remain open source.

---

> *"Your computer should serve you completely, transparently, and exclusively."*
