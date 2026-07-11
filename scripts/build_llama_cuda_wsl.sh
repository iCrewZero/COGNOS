#!/usr/bin/env bash
# Build llama-server with CUDA in WSL (RTX 4090).
#
# Prereqs (documented versions from a working WSL2 setup):
#   - Windows driver exposes GPU to WSL (nvidia-smi in WSL, no Linux driver needed)
#   - NVIDIA driver: 610.x, CUDA UMD 13.3, GPU: RTX 4090 Laptop (16 GiB VRAM)
#   - CUDA toolkit in WSL: cuda-nvcc-13-1 + cuda-cudart-dev-13-1 (nvcc 13.1.115)
#   - Host compiler for nvcc: gcc-12 / g++-12 (GCC 15 headers break CUDA 13.1 without patch)
#   - Patch: scripts/patch_cuda_glibc_wsl.sh (rsqrt noexcept vs glibc 2.43)
#
# Install once:
#   sudo apt-get install -y cuda-nvcc-13-1 cuda-cudart-dev-13-1 libcublas-dev-13-1 gcc-12 g++-12
#   sudo bash scripts/patch_cuda_glibc_wsl.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LLAMA_SRC="${LLAMA_SRC:-$ROOT/build/cache/llama.cpp}"
BUILD_DIR="${BUILD_DIR:-$LLAMA_SRC/build-cuda}"
CUDA_HOME="${CUDA_HOME:-/usr/local/cuda}"

export PATH="$CUDA_HOME/bin:$PATH"
export CUDACXX="${CUDACXX:-$CUDA_HOME/bin/nvcc}"
export LD_LIBRARY_PATH="${CUDA_HOME}/lib64:${LD_LIBRARY_PATH:-}"

echo "==> CUDA prereqs"
nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv,noheader
"$CUDACXX" --version

if [[ -f "$ROOT/scripts/patch_cuda_glibc_wsl.sh" ]]; then
  sudo bash "$ROOT/scripts/patch_cuda_glibc_wsl.sh" || true
fi

if [[ ! -d "$LLAMA_SRC/.git" ]]; then
  echo "llama.cpp source missing at $LLAMA_SRC" >&2
  exit 1
fi

HOST_CC="${HOST_CC:-gcc-12}"
HOST_CXX="${HOST_CXX:-g++-12}"
command -v "$HOST_CC" >/dev/null || { echo "install gcc-12"; exit 1; }

echo "==> Configure (GGML_CUDA=ON, host=$HOST_CC)"
cmake -S "$LLAMA_SRC" -B "$BUILD_DIR" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_C_COMPILER="$HOST_CC" \
  -DCMAKE_CXX_COMPILER="$HOST_CXX" \
  -DGGML_CUDA=ON \
  -DLLAMA_BUILD_SERVER=ON \
  -DCMAKE_CUDA_HOST_COMPILER="$HOST_CC" \
  -DCMAKE_CUDA_FLAGS="-allow-unsupported-compiler"

echo "==> Build llama-server"
cmake --build "$BUILD_DIR" -j"$(nproc)" --target llama-server

BIN="$BUILD_DIR/bin/llama-server"
echo "==> Built: $BIN"
ldd "$BIN" 2>/dev/null | grep -iE 'cuda|cudart|ggml-cuda' || true
