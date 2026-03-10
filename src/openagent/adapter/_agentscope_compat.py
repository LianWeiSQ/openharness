from __future__ import annotations

"""
AgentScope 兼容层（feature detection）。

背景：
- AgentScope 作为第三方 SDK，其模块路径/类名在不同版本中可能变化
- 我们希望 OpenAgent 代码在“未安装 agentscope”时依然可用（可选依赖）

本模块只做：
1) 延迟导入 agentscope（运行时探测）
2) 尝试从多个候选路径解析出关键类/函数（ReActAgent / 模型 Wrapper 等）
3) 给出清晰的错误信息，指导用户安装/配置
"""

import importlib
import importlib.util
from dataclasses import dataclass
from typing import Any


def agentscope_available() -> bool:
    """判断 agentscope 是否已安装（不触发真正 import）。"""

    return importlib.util.find_spec("agentscope") is not None


def import_agentscope() -> Any:
    """导入 agentscope；未安装时抛出带指导信息的错误。"""

    if not agentscope_available():
        raise RuntimeError(
            "未安装 agentscope（可选依赖）。\n"
            "请执行：pip install -e \"openagent[agentscope]\"  或  pip install agentscope"
        )
    return importlib.import_module("agentscope")


def _resolve_dotted(paths: list[str]) -> Any | None:
    """
    从若干候选 dotted-path 中解析对象。

    例如：["agentscope.agents.ReActAgent", "agentscope.agents.react.ReActAgent"]
    """

    for dotted in paths:
        mod_name, _, attr = dotted.rpartition(".")
        if not mod_name:
            continue
        try:
            mod = importlib.import_module(mod_name)
        except Exception:
            continue
        try:
            return getattr(mod, attr)
        except Exception:
            continue
    return None


@dataclass(frozen=True, slots=True)
class AgentScopeSymbols:
    """收敛我们关心的符号集合，便于上层使用。"""

    ReActAgent: Any | None
    DashScopeModel: Any | None
    OpenAIModel: Any | None


def resolve_symbols() -> AgentScopeSymbols:
    """
    解析 AgentScope 关键符号。

    注意：这里使用“候选路径列表 + 逐个 import”的方式，尽量兼容不同版本。
    如果你的 agentscope 是custom分支，可在此处追加候选路径。
    """

    # 1) ReActAgent：常见路径猜测（以兼容“跟随最新”的需求）
    react_agent = _resolve_dotted(
        [
            "agentscope.agents.ReActAgent",
            "agentscope.agents.react.ReActAgent",
            "agentscope.agents.react_agent.ReActAgent",
            "agentscope.agent.ReActAgent",
        ]
    )

    # 2) DashScope 模型 Wrapper：若 agentscope 自带 DashScope 适配优先使用
    dashscope_model = _resolve_dotted(
        [
            "agentscope.models.DashScopeChatWrapper",
            "agentscope.models.DashScopeChatModel",
            "agentscope.models.dashscope.DashScopeChatWrapper",
            "agentscope.models.dashscope.DashScopeChatModel",
        ]
    )

    # 3) OpenAI 兼容模型 Wrapper：作为 fallback（DashScope 也提供 OpenAI compatible endpoint）
    openai_model = _resolve_dotted(
        [
            "agentscope.models.OpenAIChatWrapper",
            "agentscope.models.OpenAIChatModel",
            "agentscope.models.openai.OpenAIChatWrapper",
            "agentscope.models.openai.OpenAIChatModel",
        ]
    )

    return AgentScopeSymbols(ReActAgent=react_agent, DashScopeModel=dashscope_model, OpenAIModel=openai_model)


def require_react_agent() -> Any:
    """必须解析出 ReActAgent，否则抛出明确错误。"""

    import_agentscope()
    symbols = resolve_symbols()
    if symbols.ReActAgent is None:
        raise RuntimeError(
            "已安装 agentscope，但未能解析 ReActAgent。\n"
            "可能原因：agentscope 版本 API 变化或为自定义分支。\n"
            "请在 `openagent/src/openagent/adapter/_agentscope_compat.py` 中补充候选路径。"
        )
    return symbols.ReActAgent


def make_dashscope_model(*, api_key: str, model: str, base_url: str | None = None, **kwargs: Any) -> Any:
    """
    创建 AgentScope 的模型 wrapper（优先 DashScope wrapper，其次 OpenAI wrapper 走兼容接口）。

    重要：
    - 该逻辑属于“尽力而为”的兼容策略；不同 agentscope 版本参数名可能不同
    - 如遇到不兼容，请在此处按你的 agentscope 实际 API 调整
    """

    import_agentscope()
    symbols = resolve_symbols()

    if symbols.DashScopeModel is not None:
        # 常见参数猜测：api_key / model_name / model
        try:
            return symbols.DashScopeModel(api_key=api_key, model=model, base_url=base_url, **kwargs)
        except TypeError:
            try:
                return symbols.DashScopeModel(api_key=api_key, model_name=model, base_url=base_url, **kwargs)
            except TypeError:
                return symbols.DashScopeModel(api_key=api_key, model_name=model, **kwargs)

    if symbols.OpenAIModel is not None:
        # DashScope OpenAI compatible 默认 base_url：
        # https://dashscope.aliyuncs.com/compatible-mode/v1
        # agentscope 的 OpenAI wrapper 参数名可能为 api_key/base_url/model
        compat_url = base_url or "https://dashscope.aliyuncs.com/compatible-mode/v1"
        try:
            return symbols.OpenAIModel(api_key=api_key, base_url=compat_url, model=model, **kwargs)
        except TypeError:
            try:
                return symbols.OpenAIModel(api_key=api_key, base_url=compat_url, model_name=model, **kwargs)
            except TypeError:
                return symbols.OpenAIModel(api_key=api_key, base_url=compat_url, **kwargs)

    raise RuntimeError(
        "已安装 agentscope，但未找到可用的 DashScope/OpenAI 模型 wrapper。\n"
        "请检查 agentscope 的 models 模块，或在 compat 中补充候选路径。"
    )

