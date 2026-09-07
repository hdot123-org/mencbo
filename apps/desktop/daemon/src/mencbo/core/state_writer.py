"""契约 v1 状态读写器。

state.json 是 daemon 与 client 的唯一契约面。本模块负责：

- 路径解析（MENCBO_STATE_PATH 环境变量优先，默认 ~/.mencbo/state.json）
- 原子写盘（写 tmp 同目录文件 → os.replace）
- project_id 解析（复用 resolve_git_scope，失败降级 "unknown"）
- health 汇总计算（纯函数）
- 读-改-写保留既有任务条目
"""

from __future__ import annotations

import json
import os
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from mencbo.contracts.git_scope import resolve_git_scope


def resolve_state_path() -> Path:
    """解析状态文件路径。

    环境变量 ``MENCBO_STATE_PATH`` 优先；未设置时回退到
    ``~/.mencbo/state.json``。
    """
    override = os.environ.get("MENCBO_STATE_PATH")
    if override:
        return Path(override)
    return Path.home() / ".mencbo" / "state.json"


def resolve_project_id(cwd: str | Path | None = None) -> str:
    """解析 project_id；无法解析时降级为 ``"unknown"``。"""
    scope = resolve_git_scope(cwd)
    return scope.project_id or "unknown"


def read_state(state_path: Path | None = None) -> dict:
    """读取状态文件，返回契约 v1 结构。

    文件不存在或 JSON 损坏时返回空白骨架（空 tasks、health=ok）。
    绝不抛出异常。
    """
    path = state_path if state_path is not None else resolve_state_path()
    try:
        text = path.read_text(encoding="utf-8")
        data = json.loads(text)
        if not isinstance(data, dict):
            return _blank_state()
        # 确保四个顶层键存在
        data.setdefault("updated_at", datetime.now(timezone.utc).isoformat())
        data.setdefault("project_id", "unknown")
        data.setdefault("health", "ok")
        data.setdefault("tasks", {})
        return data
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return _blank_state()


def _blank_state() -> dict:
    return {
        "updated_at": datetime.now(timezone.utc).isoformat(),
        "project_id": "unknown",
        "health": "ok",
        "tasks": {},
    }


def compute_health(tasks: dict) -> str:
    """纯函数：根据任务状态集合计算 health。

    - 空 tasks → "ok"（真空语义）
    - 任一 failed → "degraded"
    - 其余（全 success / running 混合）→ "ok"
    """
    if not tasks:
        return "ok"
    for task_data in tasks.values():
        if isinstance(task_data, dict) and task_data.get("status") == "failed":
            return "degraded"
    return "ok"


def write_state(state: dict, state_path: Path | None = None) -> Path:
    """原子写盘：先写 tmp 再 os.replace。

    写盘前自动更新 ``updated_at`` 并重算 ``health``。
    """
    path = state_path if state_path is not None else resolve_state_path()
    path.parent.mkdir(parents=True, exist_ok=True)

    state["updated_at"] = datetime.now(timezone.utc).isoformat()
    state["health"] = compute_health(state.get("tasks", {}))

    payload = json.dumps(state, ensure_ascii=False, indent=2)

    # 原子写：写 tmp 同目录 → os.replace
    fd, tmp_path_str = tempfile.mkstemp(
        dir=str(path.parent), prefix=".state-", suffix=".tmp"
    )
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(payload)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp_path_str, str(path))
    except BaseException:
        # 清理 tmp 文件（os.replace 成功后 tmp_path_str 已不存在）
        try:
            os.unlink(tmp_path_str)
        except OSError:
            pass
        raise

    return path


def set_task_state(
    state: dict,
    task_id: str,
    *,
    display_name: str | None = None,
    status: str,
    duration_ms: int | None = None,
    error: str | None = None,
) -> None:
    """读-改-写：更新单个任务的状态字段。

    保留既有任务条目（不丢失其他任务数据）。
    """
    tasks = state.setdefault("tasks", {})
    entry = tasks.get(task_id, {})
    if display_name is not None:
        entry["display_name"] = display_name
    entry["status"] = status
    entry["last_run"] = datetime.now(timezone.utc).isoformat()
    entry["duration_ms"] = duration_ms
    entry["error"] = error
    tasks[task_id] = entry
