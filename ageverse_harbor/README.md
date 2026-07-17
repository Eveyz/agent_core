# Ageverse + Harbor

Thin adapter so [Harbor](https://github.com/harbor-framework/harbor) can run the
`ageverse` one-shot CLI inside containerized benchmarks (e.g. Terminal-Bench).

**Note:** this package is named `ageverse_harbor` (not `harbor`) so it does not
shadow the installed Harbor Python framework when run from the repo root.

## Prerequisites

1. Build the CLI:

```bash
cargo build -p agent-cli --release
# binary: target/release/ageverse
```

2. Ensure `~/.agverse/config.toml` exists (same file the desktop app uses) and
   contains the model key you will pass to Harbor.

## Smoke test (local, no Harbor)

```bash
tmpdir=$(mktemp -d)
cd "$tmpdir"
/path/to/target/release/ageverse \
  -p "Create a file hello.txt with hi" \
  --permission yolo \
  --model "hunyuan/tencent/hy3:free"
cat hello.txt
```

## Verify adapter import

```bash
./ageverse_harbor/verify.sh
# should print: ageverse
```

Uses Harbor's Python (`~/.local/share/uv/tools/harbor/bin/python`). Conda's `python`
does not include the `harbor` package unless installed separately.

## Harbor trial

```bash
./ageverse_harbor/run.sh
```

Or manually:

```bash
cd /path/to/agent_core
export AGEVERSE_BINARY="$(pwd)/target/release/ageverse"
export AGEVERSE_CONFIG="$HOME/.agverse/config.toml"
export PYTHONPATH="$(pwd):$PYTHONPATH"

harbor run -d terminal-bench@2.0 \
  -a "ageverse_harbor.ageverse_agent:AgeverseAgent" \
  -m "hunyuan/tencent/hy3:free" \
  --ak "binary_path=$AGEVERSE_BINARY" \
  --ak "config_path=$AGEVERSE_CONFIG" \
  -n 1
```

Optional env overrides for `run.sh`:

```bash
AGEVERSE_MODEL="hunyuan/tencent/hy3:free" N_CONCURRENT=1 ./ageverse_harbor/run.sh --debug
```

Pass API keys with `--ae` if your config references `${VAR}` placeholders.

## CLI surface

```
ageverse -p "instruction" --model <config-key> --permission yolo
ageverse --instruction "..." --model <config-key> [--workdir DIR] [--config PATH]
```

Defaults: config `~/.agverse/config.toml`, permission `yolo` in one-shot mode.
