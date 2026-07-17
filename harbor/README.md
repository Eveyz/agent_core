# Ageverse + Harbor

Thin adapter so [Harbor](https://github.com/harbor-framework/harbor) can run the
`ageverse` one-shot CLI inside containerized benchmarks (e.g. Terminal-Bench).

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

## Harbor trial

```bash
export AGEVERSE_BINARY="$(pwd)/target/release/ageverse"
export AGEVERSE_CONFIG="$HOME/.agverse/config.toml"

harbor run -d terminal-bench@2.0 \
  --agent harbor.ageverse_agent:AgeverseAgent \
  --model "hunyuan/tencent/hy3:free" \
  --ak binary_path:$AGEVERSE_BINARY \
  --ak config_path:$AGEVERSE_CONFIG
```

Pass API keys with `--ae` if your config references `${VAR}` placeholders.

## CLI surface

```
ageverse -p "instruction" --model <config-key> --permission yolo
ageverse --instruction "..." --model <config-key> [--workdir DIR] [--config PATH]
```

Defaults: config `~/.agverse/config.toml`, permission `yolo` in one-shot mode.
