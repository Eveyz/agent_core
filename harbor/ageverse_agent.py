"""Harbor adapter for the Ageverse agent CLI.

Installs the ``ageverse`` binary into the trial environment and runs it in
one-shot mode against ``~/.agverse/config.toml`` (same schema as the desktop app).

Usage::

    harbor run -d terminal-bench@2.0 \\
      --agent harbor.ageverse_agent:AgeverseAgent \\
      --model hunyuan/tencent/hy3:free \\
      --ak binary_path:/path/to/ageverse \\
      --ak config_path:/path/to/config.toml

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

        cfg = Path(self._config_path).expanduser().resolve()
        if not cfg.is_file():
            raise RuntimeError(
                f"config.toml not found: {cfg} "
                "(must match desktop ~/.agverse/config.toml schema)"
            )

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
        model = self.model_name or ""
        model_flag = f"--model {shlex.quote(model)} " if model else ""
        cmd = (
            f"ageverse {model_flag}"
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
