#!/usr/bin/env python3
import struct
import sys

path = sys.argv[1] if len(sys.argv) > 1 else "/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
with open(path, "rb") as f:
    magic = f.read(4)
    version = struct.unpack("<I", f.read(4))[0]
    n_tensors = struct.unpack("<Q", f.read(8))[0]
    n_kv = struct.unpack("<Q", f.read(8))[0]
    print(f"magic={magic!r} version={version} n_tensors={n_tensors} n_kv={n_kv}")
    for _ in range(n_kv):
        klen = struct.unpack("<Q", f.read(8))[0]
        key = f.read(klen).decode("utf-8", errors="replace")
        vtype = struct.unpack("<I", f.read(4))[0]
        if vtype == 8:  # UINT32
            val = struct.unpack("<I", f.read(4))[0]
            print(f"{key} = {val}")
        elif vtype == 4:  # STRING
            slen = struct.unpack("<Q", f.read(8))[0]
            val = f.read(slen).decode("utf-8", errors="replace")
            print(f"{key} = {val}")
        elif vtype == 6:  # FLOAT32
            val = struct.unpack("<f", f.read(4))[0]
            print(f"{key} = {val}")
        elif vtype == 10:  # UINT64
            val = struct.unpack("<Q", f.read(8))[0]
            print(f"{key} = {val}")
        else:
            print(f"{key} = <type {vtype}>")
