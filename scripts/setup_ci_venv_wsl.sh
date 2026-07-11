#!/usr/bin/env bash
set -euo pipefail
cd "/mnt/f/Software Engineering/COGNOS"
python3 -m venv .venv
.venv/bin/pip install -q -U pip
.venv/bin/pip install -q -r agents/requirements.txt pytest
.venv/bin/pytest --version
.venv/bin/python -c "import grpc_tools.protoc; print('grpcio-tools ok')"
