#!/usr/bin/env bash
# Build a Linux x86_64 ageverse binary for Harbor Docker trials.
# On macOS, the native target/release/ageverse binary cannot run inside
# Terminal-Bench containers (Exec format error).
#
# Usage: ./ageverse_harbor/build-linux.sh
# Output: target/linux-amd64/release/ageverse

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

IMAGE="${AGEVERSE_BUILD_IMAGE:-rust:1-bookworm}"
PLATFORM="${AGEVERSE_LINUX_PLATFORM:-linux/amd64}"
TARGET_DIR="target/linux-amd64"
OUTPUT="$REPO_ROOT/$TARGET_DIR/release/ageverse"

if ! command -v docker >/dev/null 2>&1; then
  echo "error: docker is required to build a Linux binary on macOS" >&2
  exit 1
fi

echo "Image:    $IMAGE"
echo "Platform: $PLATFORM"
echo "Output:   $OUTPUT"
echo

docker run --rm --platform "$PLATFORM" \
  -v "$REPO_ROOT:/work" -w /work \
  -e CARGO_TARGET_DIR="/work/$TARGET_DIR" \
  -e CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse \
  "$IMAGE" \
  bash -c '
    set -euo pipefail
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq pkg-config libssl-dev clang cmake build-essential
    cargo build -p agent-cli --release
    file "$CARGO_TARGET_DIR/release/ageverse"
  '

echo
echo "Linux binary ready:"
echo "  $OUTPUT"
