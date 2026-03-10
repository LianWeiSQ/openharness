from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable


class RetryManager:
    def __init__(self, *, max_retry: int = 2, base_delay_s: float = 1.0) -> None:
        self.max_retry = max_retry
        self.base_delay_s = base_delay_s

    async def run(self, func: Callable[[], Awaitable[object]]) -> object:
        attempt = 0
        while True:
            try:
                return await func()
            except Exception:  # noqa: BLE001
                attempt += 1
                if attempt > self.max_retry:
                    raise
                await asyncio.sleep(self.base_delay_s * (2 ** (attempt - 1)))

