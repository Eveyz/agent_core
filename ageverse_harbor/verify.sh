#!/usr/bin/env bash
# Verify the Harbor adapter imports correctly.
# Usage: ./ageverse_harbor/verify.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PYTHONPATH="$REPO_ROOT:${PYTHONPATH:-}"

HARBOR_PYTHON="${HARBOR_PYTHON:-$HOME/.local/share/uv/tools/harbor/bin/python}"

if [[ ! -x "$HARBOR_PYTHON" ]]; then
  echo "error: Harbor Python not found at $HARBOR_PYTHON" >&2
  echo "Install Harbor with: uv tool install harbor" >&2
  exit 1
fi

"$HARBOR_PYTHON" -c \
  "from ageverse_harbor.ageverse_agent import AgeverseAgent; print(AgeverseAgent.name())"
