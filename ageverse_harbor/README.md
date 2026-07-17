# Ageverse + Harbor

Thin adapter so [Harbor](https://github.com/harbor-framework/harbor) can run the
`ageverse` one-shot CLI inside containerized benchmarks (e.g. Terminal-Bench).

**Note:** this package is named `ageverse_harbor` (not `harbor`) so it does not
shadow the installed Harbor Python framework when run from the repo root.

## Prerequisites

1. Build the CLI for Harbor's Linux containers.

On **macOS**, Harbor trials run inside Linux Docker images. The native Mac
binary fails with `Exec format error`; build a Linux x86_64 binary instead.
The Harbor CLI build omits ONNX embeddings (`fastembed`) to keep linking simple:

```bash
./ageverse_harbor/build-linux.sh
# binary: target/linux-amd64/release/ageverse
```

On **Linux**, a normal release build is enough:

```bash
cargo build -p agent-cli --release
# binary: target/release/ageverse
```

For local smoke tests on your host (outside Harbor), you can still use the
native binary at `target/release/ageverse`.

2. Ensure `~/.agverse/config.toml` exists (same file the desktop app uses) and
   contains the model key you will pass to Harbor. `run.sh` uploads this file into
   each trial container at `/root/.agverse/config.toml`.

## Smoke test (local, no Harbor)

```bash
tmpdir=$(mktemp -d)
cd "$tmpdir"
/path/to/target/release/ageverse \
  -p "Create a file hello.txt with hi" \
  --permission yolo \
  --model "volces/deepseek-v4-pro"
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
./ageverse_harbor/build-linux.sh   # macOS only; skip on Linux
./ageverse_harbor/run.sh
```

Or manually:

```bash
cd /path/to/agent_core
# macOS: use the Docker-built Linux binary
export AGEVERSE_BINARY="$(pwd)/target/linux-amd64/release/ageverse"
export AGEVERSE_CONFIG="$HOME/.agverse/config.toml"
export PYTHONPATH="$(pwd):$PYTHONPATH"

harbor run -d terminal-bench@2.0 \
  -a "ageverse_harbor.ageverse_agent:AgeverseAgent" \
  -m "volces/deepseek-v4-pro" \
  --ak "binary_path=$AGEVERSE_BINARY" \
  --ak "config_path=$AGEVERSE_CONFIG" \
  -n 1
```

Optional env overrides for `run.sh`:

```bash
AGEVERSE_MODEL="volces/deepseek-v4-pro" N_CONCURRENT=1 ./ageverse_harbor/run.sh --debug
```

On macOS, `run.sh` uses `target/linux-amd64/release/ageverse` automatically.
If you previously exported `AGEVERSE_BINARY=target/release/ageverse`, either
`unset AGEVERSE_BINARY` or point it at the Linux binary explicitly.

Pass API keys with `--ae` if your config references `${VAR}` placeholders.

## CLI surface

```
ageverse -p "instruction" --model <config-key> --permission yolo
ageverse --instruction "..." --model <config-key> [--workdir DIR] [--config PATH]
```

Defaults: config `~/.agverse/config.toml`, permission `yolo` in one-shot mode.
