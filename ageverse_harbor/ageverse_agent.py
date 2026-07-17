"""Harbor adapter for the Ageverse agent CLI.

Installs the ``ageverse`` binary into the trial environment and runs it in
one-shot mode against ``~/.agverse/config.toml`` (same schema as the desktop app).

Usage::

    export PYTHONPATH="/path/to/agent_core:$PYTHONPATH"
    harbor run -d terminal-bench@2.0 \\
      --agent ageverse_harbor.ageverse_agent:AgeverseAgent \\
      --model volces/deepseek-v4-pro \\
      --ak binary_path=/path/to/ageverse \\
      --ak config_path=/path/to/config.toml

Environment variables referenced by the installed config (e.g. API keys) must
be available in the trial via ``--ae KEY=value`` or the host environment.
"""

from __future__ import annotations

import os
import shlex
from pathlib import Path

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

REMOTE_CONFIG = "/root/.agverse/config.toml"


def _normalize_model_key(name: str) -> str:
    return "".join(
        ch for ch in name if ch not in "\u200b\ufeff\u200c\u200d"
    )


def _load_config_toml(config_path: Path) -> dict:
    import tomllib

    return tomllib.loads(config_path.read_text(encoding="utf-8"))


def _list_config_model_keys(config_path: Path) -> dict[str, str]:
    data = _load_config_toml(config_path)
    keys: dict[str, str] = {}
    for provider_key, provider in data.get("providers", {}).items():
        for model_key in provider.get("models", {}):
            full = f"{provider_key}/{model_key}"
            keys[_normalize_model_key(full)] = full
    default_model = data.get("default_model")
    if isinstance(default_model, str):
        keys.setdefault(
            _normalize_model_key(default_model),
            default_model,
        )
    return keys


def _resolve_model_key(config_path: Path, requested: str | None) -> str:
    keys = _list_config_model_keys(config_path)
    if requested:
        if requested in keys.values():
            return requested
        normalized = _normalize_model_key(requested)
        if normalized in keys:
            return keys[normalized]
        available = ", ".join(sorted(keys.values()))
        raise RuntimeError(
            f"model {requested!r} not found in {config_path}. "
            f"Available: {available}"
        )

    default_model = tomllib_default_model(config_path)
    if default_model:
        normalized = _normalize_model_key(default_model)
        if normalized in keys:
            return keys[normalized]
        return default_model

    available = ", ".join(sorted(keys.values()))
    raise RuntimeError(
        f"no Harbor model specified and default_model missing in {config_path}. "
        f"Available: {available}"
    )


def tomllib_default_model(config_path: Path) -> str | None:
    data = _load_config_toml(config_path)
    value = data.get("default_model")
    return value if isinstance(value, str) else None


def _assert_linux_elf_binary(path: Path) -> None:
    """Validate the host binary before upload (containers often lack `file`)."""
    try:
        header = path.read_bytes()[:20]
    except OSError as exc:
        raise RuntimeError(f"cannot read ageverse binary: {path}") from exc

    if len(header) < 5 or header[:4] != b"\x7fELF":
        hint = (
            "On macOS, build with ./ageverse_harbor/build-linux.sh and use "
            "target/linux-amd64/release/ageverse."
        )
        raise RuntimeError(
            f"ageverse binary is not Linux ELF: {path}. {hint}"
        )

    # ELF class 2 = 64-bit; data 1 = little-endian (x86_64)
    if header[4:6] != b"\x02\x01":
        raise RuntimeError(
            f"ageverse binary is ELF but not x86_64 little-endian: {path}"
        )


class AgeverseAgent(BaseInstalledAgent):
    """Installed-agent wrapper around the Ageverse one-shot CLI."""

    def __init__(
        self,
        *args,
        binary_path: str | None = None,
        config_path: str | None = None,
        permission: str = "yolo",
        **kwargs,
    ):
        super().__init__(*args, **kwargs)
        self._binary_path = binary_path or os.environ.get("AGEVERSE_BINARY")
        self._config_path = config_path or os.environ.get(
            "AGEVERSE_CONFIG", str(Path.home() / ".agverse" / "config.toml")
        )
        self._permission = permission
        self._resolved_model: str | None = None

    @staticmethod
    def name() -> str:
        return "ageverse"

    async def install(self, environment: BaseEnvironment) -> None:
        if not self._binary_path:
            raise RuntimeError(
                "AgeverseAgent requires binary_path kwarg or AGEVERSE_BINARY env "
                "pointing at a built `ageverse` binary"
            )
        src = Path(self._binary_path).expanduser().resolve()
        if not src.is_file():
            raise RuntimeError(f"ageverse binary not found: {src}")
        _assert_linux_elf_binary(src)

        cfg = Path(self._config_path).expanduser().resolve()
        if not cfg.is_file():
            raise RuntimeError(
                f"config.toml not found: {cfg} "
                "(must match desktop ~/.agverse/config.toml schema)"
            )
        self._resolved_model = _resolve_model_key(cfg, self.model_name or None)

        await self.exec_as_root(
            environment,
            command="mkdir -p /usr/local/bin /root/.agverse /home/agent/.agverse 2>/dev/null || true",
        )

        upload = getattr(environment, "upload_file", None) or getattr(
            environment, "copy_to", None
        )
        if callable(upload):
            await upload(str(src), "/usr/local/bin/ageverse")
            await upload(str(cfg), "/root/.agverse/config.toml")
            await self.exec_as_root(
                environment,
                command=(
                    "chmod +x /usr/local/bin/ageverse && "
                    "cp /root/.agverse/config.toml /home/agent/.agverse/config.toml 2>/dev/null || true"
                ),
            )
        else:
            await self.exec_as_root(
                environment,
                command=(
                    f"test -x {shlex.quote(str(src))} && "
                    f"cp {shlex.quote(str(src))} /usr/local/bin/ageverse && "
                    f"chmod +x /usr/local/bin/ageverse && "
                    f"cp {shlex.quote(str(cfg))} /root/.agverse/config.toml"
                ),
            )

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        model = self._resolved_model or _resolve_model_key(
            Path(self._config_path).expanduser().resolve(),
            self.model_name or None,
        )
        cmd = (
            f"ageverse --config {shlex.quote(REMOTE_CONFIG)} "
            f"--model {shlex.quote(model)} "
            f"--permission {shlex.quote(self._permission)} "
            f"-p {shlex.quote(instruction)} "
            f"2>&1 | tee /logs/agent/ageverse.txt"
        )
        await self.exec_as_agent(environment, command=cmd)

    def populate_context_post_run(self, context: AgentContext) -> None:
        log_path = self.logs_dir / "agent" / "ageverse.txt"
        if log_path.is_file():
            context.metadata["ageverse_log"] = log_path.read_text(
                encoding="utf-8", errors="replace"
            )[:50_000]
