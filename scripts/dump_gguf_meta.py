#!/usr/bin/env python3
"""Dump GGUF metadata key-values (read-only, minimal parser)."""
from __future__ import annotations

import struct
import sys

VTYPE_STRING = 4
VTYPE_UINT32 = 8
VTYPE_INT32 = 7
VTYPE_FLOAT32 = 6
VTYPE_UINT64 = 10

FILE_TYPE_NAMES = {
    0: "F32",
    1: "F16",
    2: "Q4_0",
    3: "Q4_1",
    6: "Q5_0",
    7: "Q5_1",
    8: "Q8_0",
    9: "Q8_1",
    10: "Q2_K",
    11: "Q3_K_S",
    12: "Q3_K_M",
    13: "Q3_K_L",
    14: "Q4_K_S",
    15: "Q4_K_M",
    16: "Q5_K_S",
    17: "Q5_K_M",
    18: "Q6_K",
}


def read_kv(f) -> tuple[str, object]:
    klen = struct.unpack("<Q", f.read(8))[0]
    key = f.read(klen).decode("utf-8", "replace")
    vtype = struct.unpack("<I", f.read(4))[0]
    if vtype == VTYPE_STRING:
        slen = struct.unpack("<Q", f.read(8))[0]
        return key, f.read(slen).decode("utf-8", "replace")
    if vtype == VTYPE_UINT32:
        return key, struct.unpack("<I", f.read(4))[0]
    if vtype == VTYPE_INT32:
        return key, struct.unpack("<i", f.read(4))[0]
    if vtype == VTYPE_FLOAT32:
        return key, struct.unpack("<f", f.read(4))[0]
    if vtype == VTYPE_UINT64:
        return key, struct.unpack("<Q", f.read(8))[0]
    return key, f"<unparsed type {vtype}>"


def main() -> int:
    path = sys.argv[1] if len(sys.argv) > 1 else "/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
    with open(path, "rb") as f:
        magic = f.read(4)
        if magic != b"GGUF":
            print(f"not GGUF: magic={magic!r}")
            return 1
        version = struct.unpack("<I", f.read(4))[0]
        n_tensors = struct.unpack("<Q", f.read(8))[0]
        n_kv = struct.unpack("<Q", f.read(8))[0]
        print(f"path={path}")
        print(f"file_size_bytes={open(path, 'rb').seek(0, 2) or 0}")  # wrong, fix below

    size = open(path, "rb").seek(0, 2) or 0
    import os

    print(f"file_size_bytes={os.path.getsize(path)}")
    with open(path, "rb") as f:
        f.read(4)
        version = struct.unpack("<I", f.read(4))[0]
        n_tensors = struct.unpack("<Q", f.read(8))[0]
        n_kv = struct.unpack("<Q", f.read(8))[0]
        print(f"gguf_version={version} n_tensors={n_tensors} n_kv={n_kv}")
        keys_of_interest = (
            "general.architecture",
            "general.name",
            "general.basename",
            "general.finetune",
            "general.size_label",
            "general.file_type",
            "general.quantization_version",
            "qwen2.block_count",
            "qwen2.context_length",
            "qwen2.embedding_length",
        )
        all_kv: dict[str, object] = {}
        for _ in range(n_kv):
            k, v = read_kv(f)
            all_kv[k] = v
        for k in keys_of_interest:
            if k in all_kv:
                v = all_kv[k]
                if k == "general.file_type" and isinstance(v, int):
                    label = FILE_TYPE_NAMES.get(v, "?")
                    print(f"{k}={v} ({label})")
                else:
                    print(f"{k}={v!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
