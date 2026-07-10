# Harness Evaluation

See [PLAN-0010](../docs/active/PLAN-0010_harness_evaluation.md).

## Quick start

```bash
# Mock contract suite (CI gate)
./evals/run_contract.sh
# or:
cargo run -p agent-cli -- eval run --suite contract_v1 --mode mock --gate

# Live single model (needs OPENAI_API_KEY / EVAL_API_KEY)
cargo run -p agent-cli -- eval run --suite golden_v1 --mode live \
  --model openai/gpt-4.1 --price-profile evals/prices/openai_2026_07.toml

# Multi-model compare
cargo run -p agent-cli -- eval run --suite golden_v1 --mode live \
  --compare openai/gpt-4.1,deepseek/v3 --price-profile evals/prices/openai_2026_07.toml

# Harness ablation
cargo run -p agent-cli -- eval run --suite golden_v1 --mode live \
  --model openai/gpt-4.1 --ablate permission,compression,max_iterations
```

Reports land in `evals/out/<run>/` (`summary.json`, `report.md`, optional `matrix.md`).

## Adding a task

1. Create `evals/suites/<suite>/tasks/<id>/`
2. Add `task.toml` (+ optional `script.toml`, `workspace/`, `prompt.md`, `trace.jsonl`)
3. Re-run the suite

## Suites

| Suite | Mode | Purpose |
|-------|------|---------|
| `contract_v1` | mock | Harness lifecycle / tools / permission / steer contracts |
| `golden_v1` | live/mock | Product-shaped tasks with checkers + cost metrics |
