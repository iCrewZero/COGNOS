#!/usr/bin/env python3
"""Cross-auth fixture: print a token minted by the Python auth module.

Usage:  gen_token.py <agent_id> <expiry_unix_s> <secret>
Prints the token to stdout (no trailing newline) so the Rust cross_auth test
can feed it straight into `auth::verify_token`.
"""
import os
import sys

# agents/ lives four levels up from this fixtures dir:
#   ipc/grpc/tests/fixtures -> ipc/grpc/tests -> ipc/grpc -> ipc -> <root>
_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
sys.path.insert(0, os.path.join(_ROOT, "agents"))

import auth  # noqa: E402


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: gen_token.py <agent_id> <expiry_unix_s> <secret>", file=sys.stderr)
        return 2
    agent_id, expiry_s, secret = sys.argv[1], int(sys.argv[2]), sys.argv[3]
    sys.stdout.write(auth.create_token(agent_id, expiry_s, secret))
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
