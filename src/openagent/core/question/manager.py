from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from threading import RLock
from time import time
from typing import Callable

from ..id import new_id


@dataclass(frozen=True, slots=True)
class QuestionOption:
    label: str
    description: str


@dataclass(frozen=True, slots=True)
class QuestionInfo:
    header: str
    question: str
    options: list[QuestionOption] = field(default_factory=list)
    multiple: bool = False


@dataclass(frozen=True, slots=True)
class QuestionRequest:
    request_id: str
    session_id: str
    questions: list[QuestionInfo]
    tool_call_id: str | None = None
    created_at: float = field(default_factory=time)


@dataclass(frozen=True, slots=True)
class QuestionReply:
    request_id: str
    answers: list[list[str]]


class QuestionRejectedError(RuntimeError):
    def __init__(self) -> None:
        super().__init__("The user dismissed this question")


@dataclass(slots=True)
class _PendingQuestion:
    request: QuestionRequest
    loop: asyncio.AbstractEventLoop
    future: asyncio.Future[list[list[str]]]


class QuestionManager:
    def __init__(
        self,
        *,
        on_requested: Callable[[QuestionRequest], None] | None = None,
        on_resolved: Callable[[QuestionRequest], None] | None = None,
    ) -> None:
        self._pending: dict[str, _PendingQuestion] = {}
        self._requests: asyncio.Queue[QuestionRequest] = asyncio.Queue()
        self._last_request_ids: dict[str, str] = {}
        self._lock = RLock()
        self._on_requested = on_requested
        self._on_resolved = on_resolved

    def set_hooks(
        self,
        *,
        on_requested: Callable[[QuestionRequest], None] | None = None,
        on_resolved: Callable[[QuestionRequest], None] | None = None,
    ) -> None:
        self._on_requested = on_requested
        self._on_resolved = on_resolved

    async def ask(
        self,
        *,
        session_id: str,
        questions: list[QuestionInfo],
        tool_call_id: str | None = None,
    ) -> list[list[str]]:
        loop = asyncio.get_running_loop()
        request = QuestionRequest(
            request_id=new_id("question"),
            session_id=session_id,
            questions=list(questions),
            tool_call_id=tool_call_id,
        )
        future: asyncio.Future[list[list[str]]] = loop.create_future()

        with self._lock:
            self._pending[request.request_id] = _PendingQuestion(request=request, loop=loop, future=future)
            self._last_request_ids[session_id] = request.request_id

        if self._on_requested is not None:
            self._on_requested(request)
        self._requests.put_nowait(request)

        try:
            return await future
        finally:
            with self._lock:
                self._pending.pop(request.request_id, None)

    async def next_request(self) -> QuestionRequest:
        return await self._requests.get()

    def reply(self, request_id: str, answers: list[list[str]]) -> None:
        normalized_answers = _normalize_answers(answers)
        pending = self._pop_pending(request_id)
        if pending is None:
            raise ValueError("Unknown question request")

        def _resolve() -> None:
            if self._on_resolved is not None:
                self._on_resolved(pending.request)
            if not pending.future.done():
                pending.future.set_result(normalized_answers)

        pending.loop.call_soon_threadsafe(_resolve)

    def reject(self, request_id: str) -> None:
        pending = self._pop_pending(request_id)
        if pending is None:
            raise ValueError("Unknown question request")

        def _reject() -> None:
            if self._on_resolved is not None:
                self._on_resolved(pending.request)
            if not pending.future.done():
                pending.future.set_exception(QuestionRejectedError())

        pending.loop.call_soon_threadsafe(_reject)

    def list_pending(self, session_id: str | None = None) -> list[QuestionRequest]:
        with self._lock:
            requests = [pending.request for pending in self._pending.values()]
        if session_id is None:
            return requests
        return [request for request in requests if request.session_id == session_id]

    def last_request_id(self, session_id: str) -> str | None:
        with self._lock:
            return self._last_request_ids.get(session_id)

    def _pop_pending(self, request_id: str) -> _PendingQuestion | None:
        with self._lock:
            return self._pending.pop(request_id, None)


def _normalize_answers(answers: list[list[str]]) -> list[list[str]]:
    normalized: list[list[str]] = []
    for answer_group in answers:
        if not isinstance(answer_group, list):
            raise TypeError("Question answers must be arrays of strings")
        normalized.append([str(answer).strip() for answer in answer_group if str(answer).strip()])
    return normalized


__all__ = [
    "QuestionInfo",
    "QuestionManager",
    "QuestionOption",
    "QuestionRejectedError",
    "QuestionReply",
    "QuestionRequest",
]
