# Linux development (WSL2 / VM / bare metal)

COGNOS/OS is developed and tested on **native Linux**. Paths gated with
`#[cfg(unix)]` (HAL approval socket, ANFS/FUSE, bash e2e scripts) do not
compile or run on Windows without a Unix layer.

This document lists the exact **apt** packages and the commands to reproduce a
green workspace on Ubuntu 24.04 / Debian bookworm.

---

## APT packages

Install these once on a fresh system:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential \
  pkg-config \
  libssl-dev \
  libfuse3-dev \
  protobuf-compiler \
  python3 \
  python3-venv \
  python3-pip \
  make \
  curl
```

| Package | Purpose |
|---------|---------|
| `build-essential` | Native C/C++ toolchain (`gcc`, `g++`, `ld`) for Rust `cc` crates |
| `pkg-config` | Locate system libraries at build time (`fuser` → libfuse3) |
| `libssl-dev` | OpenSSL headers/libs for TLS (tonic, reqwest) |
| `libfuse3-dev` | FUSE 3 headers for `memory/anfs` (`fuser` crate) |
| `protobuf-compiler` | `protoc` for `tonic-build` / `grpc_tools.protoc` |
| `python3`, `python3-venv`, `python3-pip` | Agent framework + pytest |
| `make` | Top-level `build/Makefile` (`make proto`, kernel pipeline) |
| `curl` | Rustup installer (below) |

> **Note:** Ubuntu 24.04 ships FUSE 3; the correct dev package is `libfuse3-dev`
> (not the legacy `libfuse-dev` from FUSE 2). `fuser` probes `fuse3` via
> pkg-config.

---

## Rust toolchain

Use the **stable** toolchain with the **native** linker. Unlike the Windows
GNU workaround, you do **not** need:

- `stable-x86_64-pc-windows-gnu`
- a portable `PROTOC` zip (system `protoc` suffices)
- `CARGO_TARGET_DIR` outside a path with spaces

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup default stable
rustc --version
cargo --version
protoc --version   # libprotoc 3.x from protobuf-compiler
```

---

## Python virtual environment

```bash
cd /path/to/COGNOS
python3 -m venv .venv
source .venv/bin/activate
pip install -r agents/requirements.txt
pip install pytest ruff
```

---

## Repro: full green workspace

Run from the repository root:

```bash
bash scripts/verify_linux.sh
```

Or step by step:

```bash
# 1. Regenerate Python gRPC stubs (first time make is available)
make -C build proto
# equivalent: python3 agents/generate_proto.py

# 2. Type-check every crate, binary, test, and example (FUSE + unix cfgs)
cargo check --workspace --all-targets

# 3. Rust tests (includes HAL approval_flow unit tests, cross_auth Python subprocesses)
cargo test --workspace

# 4. Python agent tests
source .venv/bin/activate   # if not already active
pytest agents/ -v
```

Expected on Linux (that was skipped or failed on Windows):

| Area | Windows | Linux |
|------|---------|-------|
| `memory/anfs` (`fuser`) | **Fails** — no FUSE/pkg-config | **Compiles** with `libfuse3-dev` |
| `hal::approval_flow` + 7 unit tests | **Absent** (`#[cfg(unix)]`) | **Compiled + run** |
| `cross_auth` subprocess tests (3–4) | Often **skipped** (no `python3`) | **Run** (`python3` on PATH) |
| `orchestrator/tests/hal_gate_integration` | May skip if bins missing | **Runs** after `cargo build --bins` |
| `make proto` | N/A (no `make` by default) | **Works** via `build/Makefile` |

---

## WSL2 quick start

On Windows, install WSL2 and an Ubuntu distribution (requires an **elevated**
PowerShell — admin — and usually a reboot), then follow the apt + Rust steps
inside the distro (project path via `/mnt/f/...` or a clone under `~/src`):

```powershell
# Run in PowerShell as Administrator
wsl --install -d Ubuntu
```

After reboot, open Ubuntu and run the **APT packages** block above.

---

## Troubleshooting

### `Package fuse3 was not found` / `fuser` build failure

Install FUSE dev headers:

```bash
sudo apt-get install -y libfuse3-dev pkg-config
```

### `dev-dependencies are not allowed to be optional: cognos-anfs`

Optional crates referenced by `[features]` must live under `[dependencies]`, not
`[dev-dependencies]`. `memory/Cargo.toml` already places `cognos-anfs` under
`[dependencies]` with `optional = true`.

### Python 3.14 / pip 25 on Ubuntu 26.04

If `pip install -r agents/requirements.txt` fails with an internal pip error,
pin pip 24.x or use Python 3.12 (matches CI):

```bash
python3.12 -m venv .venv   # apt: python3.12-venv
source .venv/bin/activate
pip install -U 'pip<25'
pip install -r agents/requirements.txt pytest
```

`scripts/verify_linux.sh` retries with `grpcio-tools` + `pytest` only when the
full requirements install fails.

### `grpcio-tools is not installed`

```bash
source .venv/bin/activate
pip install -r agents/requirements.txt
```

### HAL daemon socket (`/run/cognos/hal.sock`)

The Unix approval daemon is compiled only on `#[cfg(unix)]`. Wiring lives in
`hal/src/main.rs` (transport only); policy/scoring remains in `hal/src/`.
See `docs/HAL_AUDIT.md` for review notes.

---

## What you can drop from the Windows setup

| Windows workaround | Linux native |
|--------------------|--------------|
| `rustup default stable-gnu` | `rustup default stable` |
| WinLibs MinGW on `PATH` | `build-essential` |
| `$env:PROTOC = '...\protoc.exe'` | `protobuf-compiler` apt package |
| `$env:CARGO_TARGET_DIR = 'C:\cognos-tgt'` | Default `target/` in repo |
