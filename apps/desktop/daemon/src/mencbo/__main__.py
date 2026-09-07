"""CLI 入口：python -m mencbo {run|status|run-task <task_id>}。

子命令：
- run：启动调度循环，打印启动横幅
- status：读取状态文件并打印中文摘要
- run-task <task_id>：立即执行单任务并写盘
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

from mencbo.core.example_tasks import get_tasks
from mencbo.core.scheduler import run_forever, _execute_task
from mencbo.core.state_writer import (
    read_state,
    resolve_project_id,
    resolve_state_path,
    set_task_state,
    write_state,
)


def cmd_run(args: argparse.Namespace) -> None:
    """启动调度循环，打印启动横幅。"""
    tasks = get_tasks()

    # 打印启动横幅：逐一列出注册任务
    print(f"[daemon] 启动调度器，注册 {len(tasks)} 个任务：")
    for spec in tasks:
        status = "已注册" if spec.enabled else "已禁用"
        print(f"  - {spec.task_id} ({spec.display_name}) [{status}]")

    asyncio.run(run_forever(tasks))


def cmd_status(args: argparse.Namespace) -> None:
    """读取状态文件并打印中文摘要。

    中文词表：success→成功、running→运行中、failed→失败、ok→正常、degraded→降级
    缺文件→「暂无状态文件」，坏 JSON→「状态文件损坏」，均退出码 0
    """
    state_path = resolve_state_path()

    # 检查文件是否存在
    if not state_path.exists():
        print("暂无状态文件")
        sys.exit(0)

    # 尝试读取并解析
    try:
        text = state_path.read_text(encoding="utf-8")
        state = json.loads(text)
        if not isinstance(state, dict):
            raise ValueError("非对象")
    except (json.JSONDecodeError, ValueError):
        print("状态文件损坏")
        sys.exit(0)

    # 中文词表映射
    status_map = {
        "success": "成功",
        "running": "运行中",
        "failed": "失败",
    }
    health_map = {
        "ok": "正常",
        "degraded": "降级",
    }

    # 打印摘要
    project_id = state.get("project_id", "unknown")
    health = state.get("health", "ok")
    tasks = state.get("tasks", {})

    print(f"项目：{project_id}")
    print(f"健康：{health_map.get(health, health)}")
    print(f"任务：{len(tasks)} 个")

    for task_id, task_data in tasks.items():
        if not isinstance(task_data, dict):
            continue
        display_name = task_data.get("display_name", task_id)
        status = task_data.get("status", "unknown")
        last_run = task_data.get("last_run", "")
        duration_ms = task_data.get("duration_ms")

        status_cn = status_map.get(status, status)
        print(f"\n  [{task_id}] {display_name}")
        print(f"    状态：{status_cn}")
        if last_run:
            print(f"    最后运行：{last_run}")
        if duration_ms is not None:
            print(f"    耗时：{duration_ms}ms")
        error = task_data.get("error")
        if error:
            print(f"    错误：{error}")

    sys.exit(0)


def cmd_run_task(args: argparse.Namespace) -> None:
    """立即执行单任务并写盘。

    找不到任务则非零退出 + stderr 中文提示。
    """
    task_id = args.task_id
    tasks = get_tasks()

    # 查找目标任务
    target = None
    for spec in tasks:
        if spec.task_id == task_id:
            target = spec
            break

    if target is None:
        print(f"未找到任务 {task_id}", file=sys.stderr)
        sys.exit(1)

    # 同步执行并写盘
    state_path = resolve_state_path()
    project_id = resolve_project_id()

    # 初始化 state.json（若不存在）
    state = read_state(state_path)
    state["project_id"] = project_id
    write_state(state, state_path)

    # 执行任务
    asyncio.run(_execute_task(target, state_path, project_id))
    sys.exit(0)


def main() -> None:
    """CLI 主入口。"""
    parser = argparse.ArgumentParser(
        prog="mencbo",
        description="MenCbo daemon：统一任务调度宿主",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    # run 子命令
    sub_run = subparsers.add_parser("run", help="启动调度循环")
    sub_run.set_defaults(func=cmd_run)

    # status 子命令
    sub_status = subparsers.add_parser("status", help="打印状态摘要")
    sub_status.set_defaults(func=cmd_status)

    # run-task 子命令
    sub_run_task = subparsers.add_parser("run-task", help="立即执行单任务")
    sub_run_task.add_argument("task_id", help="任务 ID")
    sub_run_task.set_defaults(func=cmd_run_task)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
