#!/usr/bin/env bash
# Patch CUDA math_functions.h for glibc 2.41+ / CUDA 13.1 rsqrt noexcept mismatch.
# See: https://github.com/ggml-org/llama.cpp/issues/19100
set -euo pipefail

HDR="${1:-/usr/local/cuda/targets/x86_64-linux/include/crt/math_functions.h}"

if [[ ! -f "$HDR" ]]; then
  echo "CUDA header not found: $HDR" >&2
  exit 1
fi

if grep -q 'rsqrt(double x) noexcept' "$HDR"; then
  echo "Already patched: $HDR"
  exit 0
fi

echo "Patching $HDR (rsqrt/rsqrtf noexcept for glibc compat)"
cp -a "$HDR" "${HDR}.bak.$(date +%Y%m%d%H%M%S)"
sed -i \
  -e 's/double                 rsqrt(double x);/double                 rsqrt(double x) noexcept(true);/' \
  -e 's/float                  rsqrtf(float x);/float                  rsqrtf(float x) noexcept(true);/' \
  "$HDR"
echo "Patched OK"
