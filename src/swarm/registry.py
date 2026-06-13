from __future__ import annotations

"""Runner registry for the swarm kernel."""

from collections.abc import Iterable

from .protocol import AgentRunner


class RunnerRegistry:
    def __init__(self) -> None:
        self._runners: dict[str, AgentRunner] = {}

    def register(self, runner: AgentRunner) -> None:
        runner_id = runner.descriptor.id.strip()
        if not runner_id:
            raise ValueError("runner descriptor id is required")
        self._runners[runner_id] = runner

    def get(self, runner_id: str) -> AgentRunner | None:
        return self._runners.get(runner_id)

    def require(self, runner_id: str) -> AgentRunner:
        runner = self.get(runner_id)
        if runner is None:
            raise KeyError(f'runner "{runner_id}" is not registered')
        return runner

    def all(self) -> list[AgentRunner]:
        return list(self._runners.values())

    def matching_role(self, role: str) -> list[AgentRunner]:
        return [runner for runner in self._runners.values() if runner.descriptor.supports_role(role)]

    def ids(self) -> Iterable[str]:
        return self._runners.keys()
