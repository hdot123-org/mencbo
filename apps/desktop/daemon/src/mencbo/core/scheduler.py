"""asyncio 调度器：interval 循环 + cron 每分钟匹配。

核心职责：
- interval 任务按秒数循环，先等一轮再首跑
- cron 任务单协程每分钟 tick，字段白名单 AND 匹配
- handler 经 asyncio.to_thread + wait_for(timeout_sec)
- 执行前写 running 中间态，终态 success/failed
- enabled=False 不注册
- SIGINT/SIGTERM 优雅退出
"""

from __future__ import annotations

import asyncio
import signal
from datetime import datetime, timezone
from pathlib import Path

from mencbo.contracts.task_spec import TaskSpec
from mencbo.core.state_writer import (
    read_state,
    resolve_project_id,
    resolve_state_path,
    set_task_state,
    write_state,
)


def interval_to_seconds(schedule_value: dict) -> int:
    """将 interval schedule_value 换算为秒数。"""
    return (
        schedule_value.get("days", 0) * 86400
        + schedule_value.get("hours", 0) * 3600
        + schedule_value.get("minutes", 0) * 60
        + schedule_value.get("seconds", 0)
    )


def matches_cron(schedule_value: dict, now: datetime) -> bool:
    """纯函数：判定给定 datetime 是否匹配 cron schedule_value。

    字段白名单：minute/hour/day/month/weekday/year（全部可选，AND 语义）。
    cron 惯例：weekday 0=周日..6=周六（代码注释显式声明该映射）。
    Python datetime.weekday(): 0=周一..6=周日，需转换为 cron 惯例。
    """
    # cron 惯例：0=周日..6=周六
    # Python weekday(): 0=周一, 1=周二, ..., 5=周五, 6=周日
    # 转换公式：cron_weekday = (python_weekday + 1) % 7
    #   周一 → 1, 周二 → 2, ..., 周六 → 6, 周日 → 0
    py_weekday = now.weekday()
    cron_weekday = (py_weekday + 1) % 7

    field_values = {
        "minute": now.minute,
        "hour": now.hour,
        "day": now.day,
        "month": now.month,
        "weekday": cron_weekday,
        "year": now.year,
    }

    # 所有指定字段必须全部匹配（AND 语义）
    for field, expected in schedule_value.items():
        if field in field_values and field_values[field] != expected:
            return False
    return True


async def _execute_task(
    spec: TaskSpec,
    state_path: Path,
    project_id: str,
) -> None:
    """执行单个任务并写盘。

    1. 写 running 中间态（duration_ms=None, error=None）
    2. 用 asyncio.to_thread 包裹同步 handler
    3. wait_for(timeout_sec) 超时截断
    4. 写终态 success/failed（含 duration_ms、error）
    """
    # 读-改-写：保留既有任务条目
    state = read_state(state_path)
    state["project_id"] = project_id

    # 写 running 中间态
    set_task_state(
        state,
        spec.task_id,
        display_name=spec.display_name,
        status="running",
        duration_ms=None,
        error=None,
    )
    write_state(state, state_path)

    # 执行 handler（to_thread 避免阻塞事件循环）
    start_time = datetime.now(timezone.utc)
    
    try:
        # wait_for 会在超时后自动取消内部的 coroutine
        await asyncio.wait_for(
            asyncio.to_thread(spec.handler),
            timeout=spec.timeout_sec,
        )
        duration_ms = int(
            (datetime.now(timezone.utc) - start_time).total_seconds() * 1000
        )
        status = "success"
        error = None
    except asyncio.TimeoutError:
        # 超时后记录失败状态
        # 注意：asyncio.to_thread 启动的线程无法强制终止，会继续在后台运行
        # 但 wait_for 已经取消了协程，asyncio.run() 会等待所有未完成任务
        duration_ms = int(
            (datetime.now(timezone.utc) - start_time).total_seconds() * 1000
        )
        status = "failed"
        error = f"任务超时（>{spec.timeout_sec}s）"
    except Exception as exc:
        duration_ms = int(
            (datetime.now(timezone.utc) - start_time).total_seconds() * 1000
        )
        status = "failed"
        error = repr(exc)

    # 读-改-写终态
    state = read_state(state_path)
    state["project_id"] = project_id
    set_task_state(
        state,
        spec.task_id,
        display_name=spec.display_name,
        status=status,
        duration_ms=duration_ms,
        error=error,
    )
    write_state(state, state_path)


async def _interval_loop(
    spec: TaskSpec, state_path: Path, project_id: str
) -> None:
    """interval 任务的循环：先等一轮再首跑。"""
    seconds = interval_to_seconds(spec.schedule_value)
    while True:
        await asyncio.sleep(seconds)  # 先等一轮
        await _execute_task(spec, state_path, project_id)


async def _cron_tick_loop(
    specs: list[TaskSpec], state_path: Path, project_id: str
) -> None:
    """cron 任务单协程：每分钟 tick，匹配则触发。"""
    while True:
        now = datetime.now(timezone.utc)
        # 等待到下一分钟
        seconds_until_next = 60 - now.second
        await asyncio.sleep(seconds_until_next)

        now = datetime.now(timezone.utc)
        for spec in specs:
            if matches_cron(spec.schedule_value, now):
                await _execute_task(spec, state_path, project_id)


async def run_forever(tasks: list[TaskSpec]) -> None:
    """启动调度循环，注册全部 enabled 任务。

    - interval 任务：每任务一个 asyncio.create_task 循环
    - cron 任务：单协程每分钟 tick
    - SIGINT/SIGTERM 优雅退出
    """
    state_path = resolve_state_path()
    project_id = resolve_project_id()

    # 初始化 state.json（若不存在）
    state = read_state(state_path)
    state["project_id"] = project_id
    write_state(state, state_path)

    # 分组：interval vs cron（enabled=False 跳过）
    interval_specs: list[TaskSpec] = []
    cron_specs: list[TaskSpec] = []
    for spec in tasks:
        if not spec.enabled:
            continue
        if spec.schedule_type == "interval":
            interval_specs.append(spec)
        elif spec.schedule_type == "cron":
            cron_specs.append(spec)

    # 创建循环任务
    loop_tasks: list[asyncio.Task] = []
    for spec in interval_specs:
        t = asyncio.create_task(_interval_loop(spec, state_path, project_id))
        loop_tasks.append(t)

    if cron_specs:
        ct = asyncio.create_task(
            _cron_tick_loop(cron_specs, state_path, project_id)
        )
        loop_tasks.append(ct)

    # 优雅退出：SIGINT/SIGTERM
    stop_event = asyncio.Event()

    def _signal_handler() -> None:
        stop_event.set()

    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, _signal_handler)

    # 等待退出信号
    await stop_event.wait()

    # 取消所有循环任务
    for t in loop_tasks:
        t.cancel()
    await asyncio.gather(*loop_tasks, return_exceptions=True)
