# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| main (HEAD) | Yes |
| All others | No |

COGNOS/OS is pre-release software. Only the `main` branch receives security patches.

## Reporting a Vulnerability

If you discover a security vulnerability in COGNOS/OS, please report it responsibly.

**Do NOT open a public GitHub issue for security vulnerabilities.**

### Contact

- Email: security@cognos-os.dev
- Response time: within 72 hours
- Disclosure timeline: 90 days from initial report

### What to Include

- Description of the vulnerability
- Steps to reproduce
- Affected component (HAL, IPC, kernel, agents, etc.)
- Potential impact assessment
- Suggested fix (if any)

### Scope

The following components are in-scope for security reports:

| Component | Priority |
|-----------|----------|
| `hal/` — Human Approval Layer | Critical |
| `agents/shared/ipc/` — gRPC IPC, auth, TLS | Critical |
| `kernel/` — Custom kernel, eBPF programs | High |
| `security/` — nftables, AppArmor, cgroups | High |
| `intent-engine/` — Intent parsing | Medium |
| `agents/` — Agent framework | Medium |
| `.github/workflows/` — CI/CD pipelines | Medium |

### Out of Scope

- Vulnerabilities in upstream dependencies (report to the upstream project)
- Issues in the GTK4 UI that do not escalate privileges
- Documentation typos

## Security Design Principles

1. **HAL is human-only**: The Human Approval Layer must never be authored or modified by AI. This is enforced by CI checks and CODEOWNERS.
2. **Zero-trust IPC**: All inter-agent communication uses mTLS with Ed25519 envelope signing. No unauthenticated channels exist.
3. **Capability lattice**: Agents operate under a least-privilege model. Capabilities are granted dynamically and can be revoked at any time.
4. **Audit chain**: Every intent dispatch produces an immutable audit trail with cryptographic hash chaining.
