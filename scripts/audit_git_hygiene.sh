#!/usr/bin/env bash
# Audit tracked large binaries and secret-like patterns (read-only).
set -euo pipefail
cd "/mnt/f/Software Engineering/COGNOS"

echo "=== tracked weight/binary extensions ==="
git ls-files | grep -E '\.(gguf|safetensors|bin|iso|pt|pth|ckpt)$' || echo "(none)"

echo "=== tracked files >1 MiB ==="
while IFS= read -r f; do
  [[ -z "$f" ]] && continue
  s=$(git cat-file -s ":$f" 2>/dev/null || echo 0)
  if [[ "$s" -gt 1048576 ]]; then
    printf '%s %s\n' "$s" "$f"
  fi
done < <(git ls-files) | sort -rn | head -30 || echo "(none)"

echo "=== secret pattern scan (tracked text) ==="
patterns='COGNOS_IPC_SECRET=|api[_-]?key|password\s*=|BEGIN (RSA |OPENSSH )?PRIVATE KEY|hf_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9]{20,}'
hits=$(git grep -n -i -E "$patterns" -- ':!*.lock' 2>/dev/null || true)
if [[ -z "$hits" ]]; then
  echo "(no literal secret patterns in tracked files)"
else
  echo "$hits"
fi
