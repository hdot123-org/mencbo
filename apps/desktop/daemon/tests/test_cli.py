"""测试：CLI 子命令（run/status/run-task）。

覆盖断言：
- VAL-DAEMON-011：status 打印中文摘要
- VAL-DAEMON-012：run-task 立即执行单任务并写盘
- VAL-DAEMON-013：run-task 未知 id 非零退出 + stderr 中文提示
- VAL-DAEMON-014：run 启动即注册全部 example 任务 + 优雅退出
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

import pytest

from mencbo.core.example_tasks import get_tasks
from mencbo.core.state_writer import _blank_state, write_state


import os


def _run_cli(*args, env_overrides: dict[str, str] | None = None, cwd: Path | None = None):
    """辅助函数：运行 python -m mencbo 子命令。"""
    env = os.environ.copy()
    if env_overrides:
        env.update(env_overrides)

    result = subprocess.run(
        [sys.executable, "-m", "mencbo", *args],
        capture_output=True,
        text=True,
        env=env,
        cwd=cwd,
        timeout=10,
    )
    return result


# ---------------------------------------------------------------------------
# status 子命令
# ---------------------------------------------------------------------------


class TestStatusCommand:
    def test_status_prints_chinese_summary(self, tmp_path, monkeypatch):
        """VAL-DAEMON-011：status 打印中文摘要。"""
        state_path = tmp_path / "state.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        # 预置状态：两个任务，一成功一失败
        state = _blank_state()
        state["project_id"] = "test/project"
        state["health"] = "degraded"
        state["tasks"] = {
            "test:success": {
                "display_name": "测试成功",
                "status": "success",
                "last_run": "2026-09-07T10:00:00+00:00",
                "duration_ms": 10,
                "error": None,
            },
            "test:failed": {
                "display_name": "测试失败",
                "status": "failed",
                "last_run": "2026-09-07T10:05:00+00:00",
                "duration_ms": 5,
                "error": "演示失败",
            },
        }
        write_state(state, state_path)

        result = _run_cli("status", env_overrides={"MENCBO_STATE_PATH": str(state_path)})

        assert result.returncode == 0
        assert "测试成功" in result.stdout
        assert "测试失败" in result.stdout
        assert "成功" in result.stdout
        assert "失败" in result.stdout
        assert "降级" in result.stdout

    def test_status_missing_file(self, tmp_path, monkeypatch):
        """VAL-DAEMON-011：缺文件时打印「暂无状态文件」。"""
        state_path = tmp_path / "nonexistent.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        result = _run_cli("status", env_overrides={"MENCBO_STATE_PATH": str(state_path)})

        assert result.returncode == 0
        assert "暂无状态文件" in result.stdout

    def test_status_bad_json(self, tmp_path, monkeypatch):
        """VAL-DAEMON-011：坏 JSON 时打印「状态文件损坏」。"""
        state_path = tmp_path / "state.json"
        state_path.write_text("not valid json {", encoding="utf-8")
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        result = _run_cli("status", env_overrides={"MENCBO_STATE_PATH": str(state_path)})

        assert result.returncode == 0
        assert "状态文件损坏" in result.stdout


# ---------------------------------------------------------------------------
# run-task 子命令
# ---------------------------------------------------------------------------


class TestRunTaskCommand:
    def test_run_task_executes_and_preserves_state(self, tmp_path, monkeypatch):
        """VAL-DAEMON-012：run-task 立即执行单任务并写盘。"""
        state_path = tmp_path / "state.json"
        log_dir = tmp_path / "logs"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))
        monkeypatch.setenv("MENCBO_LOG_DIR", str(log_dir))

        # 预置状态：另一个任务条目
        state = _blank_state()
        state["tasks"] = {
            "test:existing": {
                "display_name": "既有任务",
                "status": "success",
                "last_run": "2026-09-07T09:00:00+00:00",
                "duration_ms": 5,
                "error": None,
            }
        }
        write_state(state, state_path)

        # 记录执行前的日志行数
        heartbeat_log = log_dir / "example-heartbeat.log"
        lines_before = 0
        if heartbeat_log.exists():
            lines_before = len(heartbeat_log.read_text(encoding="utf-8").strip().split("\n"))

        result = _run_cli(
            "run-task", "example:heartbeat",
            env_overrides={"MENCBO_STATE_PATH": str(state_path), "MENCBO_LOG_DIR": str(log_dir)},
        )

        assert result.returncode == 0

        # 验证任务已执行
        data = json.loads(state_path.read_text(encoding="utf-8"))
        assert "example:heartbeat" in data["tasks"]
        task_data = data["tasks"]["example:heartbeat"]
        assert task_data["status"] == "success"
        assert isinstance(task_data["duration_ms"], int)
        assert task_data["duration_ms"] >= 0
        assert datetime.fromisoformat(task_data["last_run"]).tzinfo is not None

        # 验证既有任务保留
        assert "test:existing" in data["tasks"]
        assert data["tasks"]["test:existing"]["status"] == "success"

        # 验证日志文件新增
        assert heartbeat_log.exists()
        lines_after = len(heartbeat_log.read_text(encoding="utf-8").strip().split("\n"))
        assert lines_after > lines_before

    def test_run_task_unknown_id_nonzero_exit(self, tmp_path, monkeypatch):
        """VAL-DAEMON-013：run-task 未知 id 非零退出 + stderr 中文提示。"""
        state_path = tmp_path / "state.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        result = _run_cli(
            "run-task", "nope:missing",
            env_overrides={"MENCBO_STATE_PATH": str(state_path)},
        )

        assert result.returncode != 0
        assert "未找到任务" in result.stderr
        assert "nope:missing" in result.stderr

        # 验证 state.json 未新增该 id
        if state_path.exists():
            data = json.loads(state_path.read_text(encoding="utf-8"))
            assert "nope:missing" not in data.get("tasks", {})


# ---------------------------------------------------------------------------
# run 子命令
# ---------------------------------------------------------------------------


class TestRunCommand:
    def test_run_registers_all_example_tasks_and_sigint_exits(
        self, tmp_path, monkeypatch
    ):
        """VAL-DAEMON-014：run 启动即注册全部 example 任务 + 优雅退出。"""
        state_path = tmp_path / "state.json"
        log_dir = tmp_path / "logs"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))
        monkeypatch.setenv("MENCBO_LOG_DIR", str(log_dir))

        # 后台启动 daemon
        run_env = os.environ.copy()
        run_env["MENCBO_STATE_PATH"] = str(state_path)
        run_env["MENCBO_LOG_DIR"] = str(log_dir)

        proc = subprocess.Popen(
            [sys.executable, "-m", "mencbo", "run"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=run_env,
        )

        try:
            # 等待启动横幅
            time.sleep(1)

            # 发送 SIGINT
            proc.send_signal(subprocess.signal.SIGINT)

            # 等待退出（≤5s）
            try:
                stdout, stderr = proc.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                stdout, stderr = proc.communicate()
                pytest.fail("SIGINT 后 5s 内未退出")

            # 验证启动横幅列出全部任务
            expected_tasks = get_tasks()
            for task in expected_tasks:
                assert task.task_id in stdout, f"启动横幅未列出 {task.task_id}"
                assert task.display_name in stdout

            # 验证无 traceback
            assert "Traceback" not in stderr

        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
