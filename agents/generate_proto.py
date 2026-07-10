#!/usr/bin/env python3
"""
Generate Python gRPC stubs from the canonical cognos.proto.

Output (an importable package, NEVER committed by hand — always generated):
    agents/proto/__init__.py
    agents/proto/cognos_pb2.py
    agents/proto/cognos_pb2_grpc.py

Run directly:
    python agents/generate_proto.py
or via the build system:
    make proto        # from build/

Requires grpcio-tools (bundles protoc):
    pip install -r agents/requirements.txt

The generated `*_pb2_grpc.py` normally contains an absolute
`import cognos_pb2 ...` which only resolves when the output directory is on
sys.path. Because we emit into a package (proto/), we rewrite that line to a
package-relative import so `from proto import cognos_pb2_grpc` works from the
agents/ path. This is a well-known grpc_tools limitation.

Owner: iCrewZero
"""
import importlib.util
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PROTO_DIR = os.path.abspath(os.path.join(HERE, "..", "ipc", "grpc", "proto"))
PROTO_FILE = "cognos.proto"
OUT_DIR = os.path.join(HERE, "proto")
GRPC_STUB = "cognos_pb2_grpc.py"


def _fail(msg: str) -> None:
    """Print a clear error and exit non-zero so the build fails loudly."""
    print(f"ERROR: {msg}", file=sys.stderr)
    sys.exit(1)


def _check_toolchain() -> None:
    # grpcio-tools bundles its own protoc, so this single check covers both
    # "protoc missing" and "grpcio-tools missing".
    if importlib.util.find_spec("grpc_tools") is None:
        _fail(
            "grpcio-tools is not installed (protoc unavailable).\n"
            "  Install it with:  pip install -r agents/requirements.txt\n"
            "  (or:              pip install grpcio-tools)"
        )


def _ensure_package_dir() -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    init_path = os.path.join(OUT_DIR, "__init__.py")
    if not os.path.exists(init_path):
        with open(init_path, "w", encoding="utf-8") as f:
            f.write(
                '"""Generated gRPC stubs for cognos.proto.\n\n'
                'Do not edit by hand — regenerate with `python agents/generate_proto.py`\n'
                'or `make proto`.\n"""\n'
            )


def _fix_relative_imports(grpc_path: str) -> None:
    """Rewrite `import cognos_pb2 as cognos__pb2` into a package-relative
    `from . import cognos_pb2 as cognos__pb2`."""
    with open(grpc_path, "r", encoding="utf-8") as f:
        src = f.read()
    fixed = re.sub(
        r"^import (\w+_pb2) as (\w+)$",
        r"from . import \1 as \2",
        src,
        flags=re.MULTILINE,
    )
    if fixed != src:
        with open(grpc_path, "w", encoding="utf-8") as f:
            f.write(fixed)
        print(f"Patched relative imports in {os.path.relpath(grpc_path, HERE)}")


def main() -> None:
    _check_toolchain()

    proto_abs = os.path.join(PROTO_DIR, PROTO_FILE)
    if not os.path.exists(proto_abs):
        _fail(f"proto file not found at {proto_abs}")

    _ensure_package_dir()

    cmd = [
        sys.executable, "-m", "grpc_tools.protoc",
        f"-I{PROTO_DIR}",
        f"--python_out={OUT_DIR}",
        f"--grpc_python_out={OUT_DIR}",
        proto_abs,
    ]
    print("Running:", " ".join(cmd))
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        _fail(f"protoc failed:\n{result.stderr}")

    _fix_relative_imports(os.path.join(OUT_DIR, GRPC_STUB))
    print(f"Generated stubs in {OUT_DIR}:")
    print("  - cognos_pb2.py")
    print("  - cognos_pb2_grpc.py")


if __name__ == "__main__":
    main()
