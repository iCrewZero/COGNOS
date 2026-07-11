#!/usr/bin/env python3
import json
import sys
from pathlib import Path

p = Path(sys.argv[1])
d = json.loads(p.read_text())
for c in d["cases"]:
    if "14_empty" in c["name"]:
        print(c.get("content", ""))
        break
