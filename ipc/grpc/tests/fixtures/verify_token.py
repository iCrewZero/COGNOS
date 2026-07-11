#!/usr/bin/env python3
"""Cross-auth fixture: verify a token with the Python auth module.

Usage:  verify_token.py <token> <expected_agent> <secret>
Exit 0 if the token verifies, 1 otherwise (reason on stderr). Used by the Rust
cross_auth test to prove a Rust-minted token is accepted by Python.
"""
import os
import sys

_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
sys.path.insert(0, os.path.join(_ROOT, "agents"))

import auth  # noqa: E402


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: verify_token.py <token> <expected_agent> <secret>", file=sys.stderr)
        return 2
    token, expected_agent, secret = sys.argv[1], sys.argv[2], sys.argv[3]
    try:
        ctx = auth.verify_token(token, expected_agent, secret)
    except auth.AuthError as exc:
        print(f"verification failed: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 1
    print(f"ok: {ctx['agent_id']} expiry={ctx['expiry']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
