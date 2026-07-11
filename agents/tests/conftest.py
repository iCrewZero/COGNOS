"""Pytest bootstrap for the agents test suite.

Ensures the gRPC stubs are freshly generated (never committed by hand) and
that `agents/` is importable so tests can do `from proto import ...` and
`from shared.ipc import ...`.

Runs at conftest import time — i.e. before any test module is imported — so
the `from proto import ...` at the top of test modules resolves.
"""
import os
import subprocess
import sys

AGENTS_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

if AGENTS_DIR not in sys.path:
    sys.path.insert(0, AGENTS_DIR)


def _generate_stubs() -> None:
    generator = os.path.join(AGENTS_DIR, "generate_proto.py")
    result = subprocess.run(
        [sys.executable, generator],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            "generate_proto.py failed — cannot run proto stub tests.\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )


_generate_stubs()
