from __future__ import annotations

from dataclasses import dataclass, field

from ...question import QuestionInfo, QuestionManager, QuestionOption, QuestionRejectedError
from ..definition import ToolContext, ToolExecutionSchema, ToolOutput
from ..registry import ToolRegistry


@dataclass
class QuestionOptionInput:
    label: str = field(metadata={"description": "Display text (1-5 words, concise)"})
    description: str = field(metadata={"description": "Explanation of choice"})


@dataclass
class QuestionInfoInput:
    header: str = field(metadata={"description": "Very short label (max 12 chars)"})
    question: str = field(metadata={"description": "Complete question"})
    options: list[QuestionOptionInput] = field(metadata={"description": "Available choices"})
    multiple: bool = field(default=False, metadata={"description": "Allow selecting multiple choices"})


@dataclass
class QuestionParameters:
    questions: list[QuestionInfoInput] = field(metadata={"description": "Questions to ask"})


async def question_tool(args: QuestionParameters, ctx: ToolContext) -> ToolOutput:
    manager = ctx.extra.get("question_manager")
    if not isinstance(manager, QuestionManager):
        raise RuntimeError("question tool requires a configured QuestionManager")

    questions = [
        QuestionInfo(
            header=item.header,
            question=item.question,
            options=[QuestionOption(label=option.label, description=option.description) for option in item.options],
            multiple=bool(item.multiple),
        )
        for item in args.questions
    ]
    request_id = manager.last_request_id(ctx.session_id)
    try:
        answers = await manager.ask(
            session_id=ctx.session_id,
            questions=questions,
            tool_call_id=ctx.call_id,
        )
        request_id = manager.last_request_id(ctx.session_id)
    except QuestionRejectedError as error:
        request_id = manager.last_request_id(ctx.session_id)
        return ToolOutput(
            title=_title_for_count(len(questions)),
            output="",
            error=str(error),
            metadata={
                "questions": _questions_to_metadata(questions),
                "request_id": request_id,
                "count": len(questions),
                "error_kind": "question_rejected",
            },
        )

    formatted = ', '.join(
        f'"{question.question}"="{_format_answer(answers[index] if index < len(answers) else [])}"'
        for index, question in enumerate(questions)
    )
    return ToolOutput(
        title=_title_for_count(len(questions)),
        output=f"User has answered your questions: {formatted}. You can now continue with the user's answers in mind.",
        metadata={
            "answers": answers,
            "questions": _questions_to_metadata(questions),
            "request_id": request_id,
            "count": len(questions),
        },
    )


def _title_for_count(count: int) -> str:
    return f"Asked {count} question{'s' if count != 1 else ''}"


def _format_answer(answer: list[str]) -> str:
    return ', '.join(answer) if answer else 'Unanswered'


def _questions_to_metadata(questions: list[QuestionInfo]) -> list[dict[str, object]]:
    return [
        {
            "header": question.header,
            "question": question.question,
            "multiple": question.multiple,
            "options": [
                {"label": option.label, "description": option.description}
                for option in question.options
            ],
        }
        for question in questions
    ]


def register(registry: ToolRegistry) -> None:
    registry.define_tool(
        tool_id="question",
        parameters=QuestionParameters,
        description_md="question.md",
        group="interactive",
        dangerous=False,
        execution_scope="agnostic",
        execution_schema=ToolExecutionSchema.exclusive(
            batch_group="interactive",
            mutates_session=True,
            requires_user_interaction=True,
        ),
    )(
        question_tool
    )


__all__ = ["register"]
