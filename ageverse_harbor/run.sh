#!/usr/bin/env bash
# Run Ageverse against Harbor (Terminal-Bench).
# Usage: ./ageverse_harbor/run.sh
# Optional: N_CONCURRENT=4 ./ageverse_harbor/run.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export AGEVERSE_BINARY="${AGEVERSE_BINARY:-$REPO_ROOT/target/release/ageverse}"
export AGEVERSE_CONFIG="${AGEVERSE_CONFIG:-$HOME/.agverse/config.toml}"
export PYTHONPATH="$REPO_ROOT:${PYTHONPATH:-}"

MODEL="${AGEVERSE_MODEL:-hunyuan/tencent/hy3:free}"
DATASET="${AGEVERSE_DATASET:-terminal-bench@2.0}"
N_CONCURRENT="${N_CONCURRENT:-1}"

if [[ ! -x "$AGEVERSE_BINARY" ]]; then
  echo "error: ageverse binary not found at $AGEVERSE_BINARY" >&2
  echo "Build with: cargo build -p agent-cli --release" >&2
  exit 1
fi

if [[ ! -f "$AGEVERSE_CONFIG" ]]; then
  echo "error: config not found at $AGEVERSE_CONFIG" >&2
  exit 1
fi

echo "Config:      $AGEVERSE_CONFIG"
echo "Binary:      $AGEVERSE_BINARY"
echo "Model:       $MODEL"
echo "Dataset:     $DATASET"
echo "Concurrent:  $N_CONCURRENT"
echo

harbor run -d "$DATASET" \
  -a "ageverse_harbor.ageverse_agent:AgeverseAgent" \
  -m "$MODEL" \
  --ak "binary_path=$AGEVERSE_BINARY" \
  --ak "config_path=$AGEVERSE_CONFIG" \
  -n "$N_CONCURRENT" \
  "$@"
