#!/usr/bin/env bash
# Fail if any Rust source file exceeds MAX_LINES (unless allowlisted).
# Allowlisted files are tracked for decomposition — remove entries as they're split.

set -euo pipefail

MAX_LINES=1500

# Files queued for decomposition — remove as they're split below the limit.
ALLOW=()

# Check if a file is in the allowlist (bash 3.2 compatible).
is_allowed() {
  local needle="$1"
  for f in "${ALLOW[@]}"; do
    if [[ "$f" == "$needle" ]]; then
      return 0
    fi
  done
  return 1
}

violations=0

while IFS=$'\t' read -r lines file; do
  rel="${file#./}"
  if is_allowed "$rel"; then
    continue
  fi
  echo "FAIL: $rel ($lines lines > $MAX_LINES)"
  violations=$((violations + 1))
done < <(
  find . -name '*.rs' \
    -not -path './target/*' \
    -not -path './.claude/*' \
    -print0 \
  | xargs -0 wc -l \
  | awk -v max="$MAX_LINES" '$2 != "total" && $1 > max { printf "%d\t%s\n", $1, $2 }' \
  | sort -rn
)

if [[ $violations -gt 0 ]]; then
  echo ""
  echo "$violations file(s) exceed $MAX_LINES lines."
  echo "Split them into smaller modules or add to the allowlist in $0."
  exit 1
fi

echo "All Rust files within $MAX_LINES-line limit (${#ALLOW[@]} allowlisted)."
