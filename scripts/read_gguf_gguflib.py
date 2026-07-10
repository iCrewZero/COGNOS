#!/usr/bin/env python3
from gguf import GGUFReader

path = "/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
r = GGUFReader(path, "r")
keys = [
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
]
FILE_TYPE = {
    15: "Q4_K_M",
}
import os

print("file_size_bytes", os.path.getsize(path))
for k in keys:
    if k not in r.fields:
        print(k, "= MISSING")
        continue
    f = r.fields[k]
    if f.types[0].name == "STRING":
        val = f.parts[f.data[0]]
    else:
        val = f.data[0]
    if k == "general.file_type":
        print(k, "=", val, f"({FILE_TYPE.get(val, '?')})")
    else:
        print(k, "=", val)
