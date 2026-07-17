#!/usr/bin/env bash
# Run Ageverse against Harbor (Terminal-Bench).
# Usage: ./ageverse_harbor/run.sh
# Optional: N_CONCURRENT=4 ./ageverse_harbor/run.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

LINUX_BINARY="$REPO_ROOT/target/linux-amd64/release/ageverse"
NATIVE_BINARY="$REPO_ROOT/target/release/ageverse"

resolve_ageverse_binary() {
  local host_os
  host_os="$(uname -s)"

  if [[ "$host_os" == "Darwin" ]]; then
    if [[ -n "${AGEVERSE_BINARY:-}" && "$AGEVERSE_BINARY" != "$NATIVE_BINARY" ]]; then
      printf '%s' "$AGEVERSE_BINARY"
      return
    fi
    if [[ -x "$LINUX_BINARY" ]]; then
      if [[ -n "${AGEVERSE_BINARY:-}" && "$AGEVERSE_BINARY" == "$NATIVE_BINARY" ]]; then
        echo "note: ignoring macOS binary; using $LINUX_BINARY for Harbor" >&2
      fi
      printf '%s' "$LINUX_BINARY"
      return
    fi
    printf '%s' "${AGEVERSE_BINARY:-$LINUX_BINARY}"
    return
  fi

  printf '%s' "${AGEVERSE_BINARY:-$NATIVE_BINARY}"
}

AGEVERSE_BINARY="$(resolve_ageverse_binary)"
export AGEVERSE_BINARY
export AGEVERSE_CONFIG="${AGEVERSE_CONFIG:-$HOME/.agverse/config.toml}"
export PYTHONPATH="$REPO_ROOT:${PYTHONPATH:-}"

MODEL="${AGEVERSE_MODEL:-volces/deepseek-v4-pro}"
DATASET="${AGEVERSE_DATASET:-terminal-bench@2.0}"
N_CONCURRENT="${N_CONCURRENT:-1}"

if [[ ! -x "$AGEVERSE_BINARY" ]]; then
  echo "error: ageverse binary not found at $AGEVERSE_BINARY" >&2
  if [[ "$(uname -s)" == "Darwin" ]]; then
    echo "Harbor runs Linux containers; build a Linux binary with:" >&2
    echo "  ./ageverse_harbor/build-linux.sh" >&2
  else
    echo "Build with: cargo build -p agent-cli --release" >&2
  fi
  exit 1
fi

if command -v file >/dev/null 2>&1; then
  BINARY_KIND="$(file -b "$AGEVERSE_BINARY")"
  if [[ "$BINARY_KIND" != *"ELF"* ]]; then
    if [[ "$(uname -s)" == "Darwin" && -x "$LINUX_BINARY" ]]; then
      echo "note: $AGEVERSE_BINARY is not Linux ELF; using $LINUX_BINARY" >&2
      AGEVERSE_BINARY="$LINUX_BINARY"
      export AGEVERSE_BINARY
      BINARY_KIND="$(file -b "$AGEVERSE_BINARY")"
    fi
    if [[ "$BINARY_KIND" != *"ELF"* ]]; then
      echo "error: $AGEVERSE_BINARY is not a Linux ELF binary ($BINARY_KIND)" >&2
      if [[ "$(uname -s)" == "Darwin" ]]; then
        echo "Run: ./ageverse_harbor/build-linux.sh" >&2
        echo "Or unset a stale env override: unset AGEVERSE_BINARY" >&2
      fi
      exit 1
    fi
  fi
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
