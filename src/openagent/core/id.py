from __future__ import annotations

import itertools
import time

_counter = itertools.count(1)


def new_id(prefix: str) -> str:
    # Human-friendly, roughly sortable IDs.
    return f"{prefix}_{int(time.time() * 1000)}_{next(_counter)}"

