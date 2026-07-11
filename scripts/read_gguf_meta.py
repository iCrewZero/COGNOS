#!/usr/bin/env python3
"""Print key GGUF metadata fields."""
from __future__ import annotations

import sys

try:
    import gguf
except ImportError:
    print("gguf not installed", file=sys.stderr)
    sys.exit(1)

path = sys.argv[1] if len(sys.argv) > 1 else "/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
reader = gguf.GGUFReader(path)
keys = [
    "general.architecture",
    "general.name",
    "general.basename",
    "general.finetune",
    "general.size_label",
    "general.quantization_version",
]
for k in keys:
    if k in reader.fields:
        f = reader.fields[k]
        print(f"{k}: {f.parts[f.data[0]] if f.types[0].name == 'STRING' else f.data}")
