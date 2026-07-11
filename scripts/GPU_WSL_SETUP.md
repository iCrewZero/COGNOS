# GPU / CUDA setup for COGNOS intent engine (WSL2 + RTX 4090)

## 1. WSL must be running

If `wsl` fails with `Wsl/0x80070422`, start WSL from an **elevated** PowerShell:

```powershell
Set-Service wslservice -StartupType Manual
Start-Service wslservice
Start-Service vmcompute
wsl echo OK
```

## 2. Verify GPU in WSL

```bash
nvidia-smi
```

Expected: `NVIDIA GeForce RTX 4090` (driver exposed by Windows, no separate Linux driver).

## 3. Install CUDA toolkit (nvcc) in WSL

```bash
sudo apt-get update
sudo apt-get install -y cuda-nvcc-13-1 cuda-cudart-dev-13-1 gcc-12 g++-12
export PATH=/usr/local/cuda/bin:$PATH
nvcc --version   # CUDA 13.1
```

Versions observed on this machine:
- Driver: 610.43.02 (CUDA UMD 13.3)
- nvcc: 13.1.115 (`/usr/local/cuda/bin/nvcc`)
- Host compiler for CUDA: **gcc-12** (gcc-15 glibc headers conflict with CUDA 13.1)

## 4. Patch CUDA headers (glibc 2.43 rsqrt fix)

If cmake fails with `rsqrt exception specification is incompatible`:

```bash
sudo bash scripts/patch_cuda_glibc_wsl.sh
```

## 5. Build llama-server with CUDA

```bash
cd "/mnt/f/Software Engineering/COGNOS"
bash scripts/build_llama_cuda_wsl.sh
```

Build output: `/root/cognos-build/llama-cuda/bin/llama-server`  
Copied to: `build/cache/llama.cpp/build-cuda/bin/llama-server`

## 6. Run llama-server on GPU

```bash
fuser -k 8080/tcp 2>/dev/null || true
build/cache/llama.cpp/build-cuda/bin/llama-server \
  -m /root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf \
  --host 127.0.0.1 --port 8080 \
  -ngl 99 -c 4096 --jinja --reasoning off \
  2>&1 | tee /tmp/llama-gpu.log
```

Confirm in log: `offloaded N/N layers to GPU` or CUDA device lines.

## 7. Benchmark latencies + golden JSON

```bash
bash scripts/benchmark_gpu_intent_wsl.sh
```

Targets: **cached <500 ms**, **uncached <3 s** (wall clock on `/completion`).

## 8. Full E2E

```bash
# terminal 1: llama-server (above)
# terminal 2:
bash scripts/dev_e2e.sh real
```

Check `build/e2e_logs/intent.log` for `source=llm` and `parse_ms`.
