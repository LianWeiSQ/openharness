from __future__ import annotations

import asyncio
import os
import posixpath
import re
import shlex
import shutil
import subprocess
import sys
from datetime import timedelta
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import Any

from ..session.session import Session
from ..tool.utils import ensure_within_root, resolve_optional_path, resolve_path_in_root

WINDOWS_ABSOLUTE_RE = re.compile(r"^[A-Za-z]:[\\/]")
EXECUTION_METADATA_KEY = "execution"


@dataclass(frozen=True, slots=True)
class ExecutionBinding:
    mode: str = "local"
    sandbox_id: str | None = None
    remote_workdir: str | None = None
    connection: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class CommandResult:
    returncode: int
    stdout: str
    stderr: str
    cwd: str


@dataclass(frozen=True, slots=True)
class WorkspaceEntry:
    path: str
    is_dir: bool
    mtime: float = 0.0


def execution_binding_from_session(session: Session) -> ExecutionBinding:
    raw = session.metadata.get(EXECUTION_METADATA_KEY)
    if not isinstance(raw, dict):
        return ExecutionBinding(mode="local")

    mode = str(raw.get("mode") or "local").strip().lower()
    if not mode or mode == "local":
        return ExecutionBinding(mode="local")
    if mode != "opensandbox":
        raise ValueError(f"Unsupported execution mode: {mode}")

    sandbox_id = str(raw.get("sandbox_id") or "").strip()
    remote_workdir = str(raw.get("remote_workdir") or "").strip()
    if not sandbox_id:
        raise ValueError("execution.sandbox_id is required when mode=opensandbox")
    if not remote_workdir:
        raise ValueError("execution.remote_workdir is required when mode=opensandbox")
    if WINDOWS_ABSOLUTE_RE.match(remote_workdir) or "\\" in remote_workdir:
        raise ValueError("execution.remote_workdir must be an absolute POSIX path")
    if not remote_workdir.startswith("/"):
        raise ValueError("execution.remote_workdir must be an absolute POSIX path")

    normalized_remote = posixpath.normpath(remote_workdir)
    if not normalized_remote.startswith("/"):
        normalized_remote = "/" + normalized_remote.lstrip("/")

    connection = raw.get("connection")
    return ExecutionBinding(
        mode="opensandbox",
        sandbox_id=sandbox_id,
        remote_workdir=normalized_remote,
        connection=dict(connection) if isinstance(connection, dict) else {},
    )


def build_workspace_runtime(session: Session) -> "LocalWorkspaceRuntime | OpenSandboxWorkspaceRuntime":
    binding = execution_binding_from_session(session)
    if binding.mode == "opensandbox":
        return OpenSandboxWorkspaceRuntime(binding)
    return LocalWorkspaceRuntime(session.directory.resolve())


class LocalWorkspaceRuntime:
    mode = "local"

    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        self.workspace_root = str(self.root)

    @property
    def execution_metadata(self) -> dict[str, Any]:
        return {"execution_mode": "local", "workspace_root": self.workspace_root}

    def display_path(self, path: str | Path) -> str:
        target = Path(path).resolve()
        try:
            return str(target.relative_to(self.root))
        except Exception:  # noqa: BLE001
            return str(target)

    def resolve_path(self, path: str | None, *, default_to_root: bool = True) -> str:
        if path is None:
            if not default_to_root:
                raise ValueError("path is required")
            return str(self.root)
        return str(resolve_optional_path(self.root, path))

    def resolve_file_path(self, path: str) -> str:
        return str(resolve_path_in_root(self.root, path))

    async def run_command(self, command: str, cwd: str | None, timeout_ms: int) -> CommandResult:
        resolved_cwd = Path(self.resolve_path(cwd, default_to_root=True)).resolve()

        def _run() -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                command,
                cwd=str(resolved_cwd),
                shell=True,
                capture_output=True,
                text=True,
                timeout=timeout_ms / 1000.0,
            )

        completed = await asyncio.to_thread(_run)
        return CommandResult(
            returncode=completed.returncode,
            stdout=completed.stdout or "",
            stderr=completed.stderr or "",
            cwd=str(resolved_cwd),
        )

    async def exists(self, path: str) -> bool:
        return Path(path).exists()

    async def is_dir(self, path: str) -> bool:
        return Path(path).is_dir()

    async def read_text(self, path: str) -> str:
        return Path(path).read_text(encoding="utf-8")

    async def write_text(self, path: str, content: str) -> None:
        target = Path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")

    async def edit_text(self, path: str, old_string: str, new_string: str, *, replace_all: bool) -> str:
        target = Path(path)
        text = target.read_text(encoding="utf-8")
        if old_string == "":
            new_text = new_string
        else:
            occurrences = text.count(old_string)
            if occurrences == 0:
                raise ValueError(f"old_string not found in file: {target}")
            if occurrences > 1 and not replace_all:
                raise ValueError(
                    "old_string found multiple times; pass replace_all=True to replace every match"
                )
            new_text = text.replace(old_string, new_string) if replace_all else text.replace(old_string, new_string, 1)
        target.write_text(new_text, encoding="utf-8")
        return new_text

    async def glob(self, base_path: str, pattern: str) -> list[str]:
        import glob as globlib

        matches = globlib.glob(str(Path(base_path) / pattern), recursive=True)
        results: list[str] = []
        seen: set[str] = set()
        for item in matches:
            candidate = Path(item).resolve()
            if not ensure_within_root(self.root, candidate):
                continue
            text = str(candidate)
            if text in seen:
                continue
            seen.add(text)
            results.append(text)
        return sorted(results)

    async def grep(self, base_path: str, pattern: str, include_glob: str | None) -> list[dict[str, str | int | float]]:
        import fnmatch

        regex = re.compile(pattern)
        matches: list[dict[str, str | int | float]] = []
        base = Path(base_path)
        for dirpath, _dirnames, filenames in os.walk(base, onerror=lambda _e: None):
            for filename in filenames:
                if include_glob and not fnmatch.fnmatch(filename, include_glob):
                    continue
                path = Path(dirpath) / filename
                try:
                    content = path.read_text(encoding="utf-8", errors="ignore")
                except OSError:
                    continue
                mtime = float(path.stat().st_mtime) if path.exists() else 0.0
                for index, line in enumerate(content.splitlines(), start=1):
                    if not regex.search(line):
                        continue
                    matches.append({"path": str(path), "line": index, "text": line, "mtime": mtime})
        return matches

    async def ls(self, base_path: str, ignore: list[str]) -> list[WorkspaceEntry]:
        import fnmatch

        base = Path(base_path)
        entries: list[WorkspaceEntry] = []
        for dirpath, dirnames, filenames in os.walk(base, onerror=lambda _e: None):
            dirpath_obj = Path(dirpath)
            rel_dir = "" if dirpath_obj == base else str(dirpath_obj.relative_to(base)).replace("\\", "/")
            filtered_dirnames: list[str] = []
            for dirname in dirnames:
                rel_path = f"{rel_dir}/{dirname}" if rel_dir else dirname
                if _should_ignore(rel_path, dirname, ignore):
                    continue
                filtered_dirnames.append(dirname)
                entry_path = dirpath_obj / dirname
                entries.append(
                    WorkspaceEntry(
                        path=str(entry_path),
                        is_dir=True,
                        mtime=float(entry_path.stat().st_mtime) if entry_path.exists() else 0.0,
                    )
                )
            dirnames[:] = filtered_dirnames
            for filename in filenames:
                rel_path = f"{rel_dir}/{filename}" if rel_dir else filename
                if _should_ignore(rel_path, filename, ignore):
                    continue
                entry_path = dirpath_obj / filename
                entries.append(
                    WorkspaceEntry(
                        path=str(entry_path),
                        is_dir=False,
                        mtime=float(entry_path.stat().st_mtime) if entry_path.exists() else 0.0,
                    )
                )
        return entries


class OpenSandboxWorkspaceRuntime:
    mode = "opensandbox"

    def __init__(self, binding: ExecutionBinding) -> None:
        if not binding.sandbox_id or not binding.remote_workdir:
            raise ValueError("sandbox_id and remote_workdir are required for opensandbox mode")
        self.binding = binding
        self.workspace_root = binding.remote_workdir
        self._sandbox: Any | None = None

    @property
    def execution_metadata(self) -> dict[str, Any]:
        payload = {
            "execution_mode": "opensandbox",
            "sandbox_id": self.binding.sandbox_id,
            "remote_workdir": self.binding.remote_workdir,
        }
        if self.binding.connection:
            payload["connection"] = dict(self.binding.connection)
        return payload

    async def _sandbox_client(self) -> Any:
        if self._sandbox is not None:
            return self._sandbox

        Sandbox, ConnectionConfig, _WriteEntry, _SearchEntry = _load_opensandbox_sdk()
        kwargs: dict[str, Any] = {}
        api_key = str(self.binding.connection.get("api_key") or os.getenv("OPEN_SANDBOX_API_KEY") or "").strip()
        if api_key:
            kwargs["api_key"] = api_key
        domain = str(self.binding.connection.get("domain") or os.getenv("OPEN_SANDBOX_DOMAIN") or "").strip()
        if domain:
            kwargs["domain"] = domain
        protocol = str(self.binding.connection.get("protocol") or "").strip()
        if protocol:
            kwargs["protocol"] = protocol
        if "use_server_proxy" in self.binding.connection:
            kwargs["use_server_proxy"] = bool(self.binding.connection.get("use_server_proxy"))
        headers = self.binding.connection.get("headers")
        if isinstance(headers, dict):
            kwargs["headers"] = dict(headers)
        request_timeout_seconds = self.binding.connection.get("request_timeout_seconds")
        if request_timeout_seconds is not None:
            timeout_seconds = float(request_timeout_seconds)
            if timeout_seconds <= 0:
                raise ValueError("execution.connection.request_timeout_seconds must be positive")
            kwargs["request_timeout"] = timedelta(seconds=timeout_seconds)

        connection_config = ConnectionConfig(**kwargs)
        connected = Sandbox.connect(self.binding.sandbox_id, connection_config=connection_config)
        self._sandbox = await _maybe_await(connected)
        return self._sandbox

    def display_path(self, path: str) -> str:
        root = self.binding.remote_workdir or "/"
        if path == root:
            return "."
        prefix = root.rstrip("/") + "/"
        if path.startswith(prefix):
            return path[len(prefix) :]
        return path

    def resolve_path(self, path: str | None, *, default_to_root: bool = True) -> str:
        if path is None or path == "":
            if not default_to_root:
                raise ValueError("path is required")
            return self.binding.remote_workdir or "/"
        text = str(path).strip()
        if WINDOWS_ABSOLUTE_RE.match(text) or "\\" in text:
            raise ValueError("Sandbox paths must use POSIX syntax.")
        raw = PurePosixPath(text)
        base = PurePosixPath(self.binding.remote_workdir or "/")
        target = raw if raw.is_absolute() else base / raw
        normalized = posixpath.normpath(str(target))
        if not normalized.startswith("/"):
            normalized = "/" + normalized.lstrip("/")
        root = posixpath.normpath(self.binding.remote_workdir or "/")
        if normalized != root and not normalized.startswith(root.rstrip("/") + "/"):
            raise ValueError("Path escapes remote_workdir")
        return normalized

    def resolve_file_path(self, path: str) -> str:
        return self.resolve_path(path, default_to_root=False)

    async def run_command(self, command: str, cwd: str | None, timeout_ms: int) -> CommandResult:
        sandbox = await self._sandbox_client()
        resolved_cwd = self.resolve_path(cwd, default_to_root=True)
        wrapped = f"cd {shlex.quote(resolved_cwd)} && {command}"
        execution_call = sandbox.commands.run(wrapped)
        execution = await asyncio.wait_for(_maybe_await(execution_call), timeout=max(timeout_ms / 1000.0, 1.0))
        stdout, stderr, returncode = _extract_command_result(execution)
        return CommandResult(returncode=returncode, stdout=stdout, stderr=stderr, cwd=resolved_cwd)

    async def exists(self, path: str) -> bool:
        result = await self.run_command(f"test -e {shlex.quote(path)}", cwd=self.workspace_root, timeout_ms=10_000)
        return result.returncode == 0

    async def is_dir(self, path: str) -> bool:
        result = await self.run_command(f"test -d {shlex.quote(path)}", cwd=self.workspace_root, timeout_ms=10_000)
        return result.returncode == 0

    async def read_text(self, path: str) -> str:
        sandbox = await self._sandbox_client()
        content = await _maybe_await(sandbox.files.read_file(path))
        return str(content if content is not None else "")

    async def write_text(self, path: str, content: str) -> None:
        sandbox = await self._sandbox_client()
        _Sandbox, _ConnectionConfig, WriteEntry, _SearchEntry = _load_opensandbox_sdk()
        parent = posixpath.dirname(path.rstrip("/"))
        if parent and parent != "/":
            await self.run_command(f"mkdir -p {shlex.quote(parent)}", cwd=self.workspace_root, timeout_ms=10_000)
        await _maybe_await(sandbox.files.write_files([WriteEntry(path=path, data=content)]))

    async def edit_text(self, path: str, old_string: str, new_string: str, *, replace_all: bool) -> str:
        text = await self.read_text(path)
        if old_string == "":
            new_text = new_string
        else:
            occurrences = text.count(old_string)
            if occurrences == 0:
                raise ValueError(f"old_string not found in file: {path}")
            if occurrences > 1 and not replace_all:
                raise ValueError(
                    "old_string found multiple times; pass replace_all=True to replace every match"
                )
            new_text = text.replace(old_string, new_string) if replace_all else text.replace(old_string, new_string, 1)
        await self.write_text(path, new_text)
        return new_text

    async def glob(self, base_path: str, pattern: str) -> list[str]:
        sandbox = await self._sandbox_client()
        _Sandbox, _ConnectionConfig, _WriteEntry, SearchEntry = _load_opensandbox_sdk()
        results = await _maybe_await(sandbox.files.search(SearchEntry(path=base_path, pattern=pattern)))
        return sorted(_extract_search_paths(results))

    async def grep(self, base_path: str, pattern: str, include_glob: str | None) -> list[dict[str, str | int | float]]:
        regex = re.compile(pattern)
        candidates = await self.glob(base_path, include_glob or "**/*")
        matches: list[dict[str, str | int | float]] = []
        for path in candidates:
            if await self.is_dir(path):
                continue
            try:
                content = await self.read_text(path)
            except Exception:  # noqa: BLE001
                continue
            for index, line in enumerate(content.splitlines(), start=1):
                if not regex.search(line):
                    continue
                matches.append({"path": path, "line": index, "text": line, "mtime": 0.0})
        return matches

    async def ls(self, base_path: str, ignore: list[str]) -> list[WorkspaceEntry]:
        find_cmd = "find . -mindepth 1 -printf '%y\\t%P\\n'"
        result = await self.run_command(find_cmd, cwd=base_path, timeout_ms=30_000)
        entries: list[WorkspaceEntry] = []
        if result.returncode == 0 and result.stdout.strip():
            for raw_line in result.stdout.splitlines():
                if "\t" not in raw_line:
                    continue
                kind, rel = raw_line.split("\t", 1)
                rel = rel.strip()
                if not rel:
                    continue
                name = PurePosixPath(rel).name
                if _should_ignore(rel, name, ignore):
                    continue
                entries.append(
                    WorkspaceEntry(
                        path=self.resolve_path(rel),
                        is_dir=kind == "d",
                        mtime=0.0,
                    )
                )
            return entries

        file_paths = await self.glob(base_path, "**/*")
        seen_dirs: set[str] = set()
        for file_path in file_paths:
            rel = self.display_path(file_path)
            parts = PurePosixPath(rel).parts[:-1]
            current_rel = ""
            for part in parts:
                current_rel = posixpath.join(current_rel, part) if current_rel else part
                current_abs = self.resolve_path(current_rel)
                if current_abs not in seen_dirs:
                    entries.append(WorkspaceEntry(path=current_abs, is_dir=True, mtime=0.0))
                    seen_dirs.add(current_abs)
            entries.append(WorkspaceEntry(path=file_path, is_dir=False, mtime=0.0))
        return entries


def _load_opensandbox_sdk():
    try:
        from opensandbox.models.filesystem import SearchEntry, WriteEntry
        from opensandbox.sandbox import ConnectionConfig, Sandbox
    except ImportError as error:  # pragma: no cover - exercised via failure branch
        sdk_src = _find_local_opensandbox_sdk_src()
        if sdk_src is not None:
            sys.path.insert(0, str(sdk_src))
            try:
                from opensandbox.models.filesystem import SearchEntry, WriteEntry
                from opensandbox.sandbox import ConnectionConfig, Sandbox
            except ImportError:
                pass
            else:
                return Sandbox, ConnectionConfig, WriteEntry, SearchEntry
        raise RuntimeError(
            "OpenSandbox execution requires the 'opensandbox' package. Install it, or keep the OpenSandbox SDK repo at "
            "'<workspace>/OpenSandbox/sdks/sandbox/python/src' for local development."
        ) from error
    return Sandbox, ConnectionConfig, WriteEntry, SearchEntry


def _find_local_opensandbox_sdk_src() -> Path | None:
    current = Path(__file__).resolve()
    for parent in current.parents:
        candidates = [
            parent / "OpenSandbox" / "sdks" / "sandbox" / "python" / "src",
            parent / "sdks" / "sandbox" / "python" / "src",
        ]
        for candidate in candidates:
            if (candidate / "opensandbox" / "__init__.py").exists():
                return candidate
    return None


async def _maybe_await(value: Any) -> Any:
    if asyncio.iscoroutine(value) or hasattr(value, "__await__"):
        return await value
    return value


def _extract_command_result(execution: Any) -> tuple[str, str, int]:
    stdout_parts: list[str] = []
    stderr_parts: list[str] = []
    logs = getattr(execution, "logs", None)
    if logs is not None:
        stdout_parts.extend(_coerce_log_lines(getattr(logs, "stdout", None)))
        stderr_parts.extend(_coerce_log_lines(getattr(logs, "stderr", None)))
    else:
        stdout_parts.extend(_coerce_log_lines(getattr(execution, "stdout", None)))
        stderr_parts.extend(_coerce_log_lines(getattr(execution, "stderr", None)))

    returncode = _coerce_return_code(execution)
    return "".join(stdout_parts), "".join(stderr_parts), returncode


def _coerce_log_lines(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        parts: list[str] = []
        for item in value:
            text = getattr(item, "text", None)
            if text is not None:
                parts.append(str(text))
            elif isinstance(item, str):
                parts.append(item)
            elif isinstance(item, dict) and "text" in item:
                parts.append(str(item["text"]))
            else:
                parts.append(str(item))
        return parts
    return [str(value)]


def _coerce_return_code(value: Any) -> int:
    for key in ("returncode", "return_code", "exit_code", "code"):
        candidate = getattr(value, key, None)
        if isinstance(candidate, int):
            return candidate
        if isinstance(candidate, str) and candidate.strip().lstrip("-").isdigit():
            return int(candidate)
    result = getattr(value, "result", None)
    if result is not None:
        return _coerce_return_code(result)
    status = str(getattr(value, "status", "") or "").lower()
    if status in {"completed", "success", "succeeded"}:
        return 0
    if status in {"failed", "error"}:
        return 1
    return 0


def _extract_search_paths(value: Any) -> list[str]:
    results = getattr(value, "results", value)
    if not isinstance(results, list):
        return []
    paths: list[str] = []
    for item in results:
        path = getattr(item, "path", None)
        if path is None and isinstance(item, dict):
            path = item.get("path")
        text = str(path or "").strip()
        if text and text not in paths:
            paths.append(text)
    return paths


def _should_ignore(relative_path: str, name: str, patterns: list[str]) -> bool:
    import fnmatch

    normalized = relative_path.replace("\\", "/")
    for pattern in patterns:
        candidate = pattern.replace("\\", "/")
        if fnmatch.fnmatch(normalized, candidate):
            return True
        if fnmatch.fnmatch(name, candidate):
            return True
        if candidate.endswith("/") and normalized.startswith(candidate):
            return True
    return False
