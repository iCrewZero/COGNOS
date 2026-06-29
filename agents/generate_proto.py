#!/usr/bin/env python3
"""
Generate Python gRPC stubs from the canonical cognos.proto.

Run from the agents/ directory:
    python generate_proto.py

Requires: grpcio-tools (pip install grpcio-tools)

Owner: iCrewZero
"""
import subprocess
import sys
import os

# The canonical proto file lives here
PROTO_PATH = os.path.join(os.path.dirname(__file__), "..", "ipc", "grpc", "proto")
PROTO_FILE = "cognos.proto"
OUT_DIR = os.path.dirname(__file__)  # agents/

def main():
    proto_abs = os.path.abspath(os.path.join(PROTO_PATH, PROTO_FILE))
    if not os.path.exists(proto_abs):
        print(f"ERROR: Proto file not found at {proto_abs}")
        sys.exit(1)

    cmd = [
        sys.executable, "-m", "grpc_tools.protoc",
        f"-I{os.path.abspath(PROTO_PATH)}",
        f"--python_out={OUT_DIR}",
        f"--grpc_python_out={OUT_DIR}",
        proto_abs,
    ]
    print(f"Running: {' '.join(cmd)}")
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"FAILED:\n{result.stderr}")
        sys.exit(1)
    print(f"Generated: {OUT_DIR}/cognos_pb2.py and cognos_pb2_grpc.py")

if __name__ == "__main__":
    main()
