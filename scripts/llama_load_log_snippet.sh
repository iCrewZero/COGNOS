#!/usr/bin/env bash
set -euo pipefail
BIN="/mnt/f/Software Engineering/COGNOS/build/cache/llama.cpp/build-cuda/bin/llama-server"
MODEL="/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
timeout 45s "$BIN" -m "$MODEL" --host 127.0.0.1 --port 18080 -ngl 99 -c 512 --log-timestamps -v 2>&1 | head -80 || true
