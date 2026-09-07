"""示例任务清单：供 Phase 2 调度器联调使用。

两个任务均为无参可调用、幂等、零外部依赖，仅向日志目录追加时间戳。
日志目录默认在系统临时目录下，可用环境变量 ``MENCBO_LOG_DIR`` 覆盖。
"""

from __future__ import annotations

import os
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from mencbo.contracts.task_spec import TaskSpec

_DEFAULT_LOG_DIR = Path(tempfile.gettempdir()) / "mencbo"


def _log_dir() -> Path:
    directory = Path(os.environ.get("MENCBO_LOG_DIR", str(_DEFAULT_LOG_DIR)))
    directory.mkdir(parents=True, exist_ok=True)
    return directory


def _append_timestamp(filename: str) -> str:
    path = _log_dir() / filename
    timestamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    with path.open("a", encoding="utf-8") as fh:
        fh.write(f"{timestamp}\n")
    return str(path)


def write_heartbeat_log() -> str:
    """示例任务：追加一条心跳时间戳，返回日志文件路径。"""
    return _append_timestamp("example-heartbeat.log")


def write_maintenance_log() -> str:
    """示例任务：追加一条维护时间戳，返回日志文件路径。"""
    return _append_timestamp("example-maintenance.log")


def raise_demo_error() -> None:
    """示例失败任务：故意抛出 RuntimeError 演示 failed 状态。"""
    raise RuntimeError("示例失败：故意演示 failed 状态")


def get_tasks() -> list[TaskSpec]:
    """导出示例任务清单。

    Phase 2 的调度器将以同一签名动态加载各子项目（memory、infra-core）
    的 ``tasks.py``，本模块即该规范的最小参考实现。
    """
    return [
        TaskSpec(
            task_id="example:heartbeat",
            display_name="示例·心跳日志",
            schedule_type="interval",
            schedule_value={"minutes": 5},
            handler=write_heartbeat_log,
        ),
        TaskSpec(
            task_id="example:maintenance",
            display_name="示例·维护日志",
            schedule_type="cron",
            schedule_value={"hour": 3, "minute": 0},
            handler=write_maintenance_log,
            timeout_sec=60,
        ),
        TaskSpec(
            task_id="example:failure-demo",
            display_name="示例·失败演示",
            schedule_type="interval",
            schedule_value={"minutes": 10},
            handler=raise_demo_error,
        ),
    ]
