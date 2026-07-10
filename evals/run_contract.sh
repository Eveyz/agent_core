#!/usr/bin/env bash
# CI gate: mock contract suite must pass with harness_fail_rate == 0
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo run -p agent-cli --quiet -- eval run \
  --suite contract_v1 \
  --mode mock \
  --gate \
  -o evals/out/ci_contract
