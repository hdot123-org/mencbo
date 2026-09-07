"""测试：scheduler（asyncio 调度器）。

覆盖断言：
- VAL-DAEMON-001：interval 调度按配置触发并写契约 v1 状态
- VAL-DAEMON-002：执行前置 running 状态可观测
- VAL-DAEMON-003：失败 handler 记 failed 且 error 非空
- VAL-DAEMON-004：超时任务被截断并记 failed
- VAL-DAEMON-009：enabled=False 的任务不注册不执行
- VAL-DAEMON-010：cron 字段匹配逻辑
"""

from __future__ import annotations

import asyncio
import json
import time
from datetime import datetime, timezone
from pathlib import Path

import pytest

from mencbo.contracts.task_spec import TaskSpec
from mencbo.core.scheduler import (
    _cron_tick_loop,
    _execute_task,
    _interval_loop,
    interval_to_seconds,
    matches_cron,
    run_forever,
)
from mencbo.core.state_writer import read_state, resolve_state_path


def _spec(**overrides) -> TaskSpec:
    defaults: dict = dict(
        task_id="test:demo",
        display_name="测试任务",
        schedule_type="interval",
        schedule_value={"seconds": 1},
        handler=lambda: None,
        timeout_sec=300,
        enabled=True,
    )
    defaults.update(overrides)
    return TaskSpec(**defaults)


# ---------------------------------------------------------------------------
# interval_to_seconds
# ---------------------------------------------------------------------------


class TestIntervalToSeconds:
    def test_seconds_only(self):
        assert interval_to_seconds({"seconds": 30}) == 30

    def test_minutes_and_seconds(self):
        assert interval_to_seconds({"minutes": 2, "seconds": 15}) == 135

    def test_hours_minutes_seconds(self):
        assert interval_to_seconds({"hours": 1, "minutes": 30, "seconds": 45}) == 5445

    def test_days(self):
        assert interval_to_seconds({"days": 1}) == 86400

    def test_all_fields(self):
        assert (
            interval_to_seconds(
                {"days": 1, "hours": 2, "minutes": 30, "seconds": 15}
            )
            == 86400 + 7200 + 1800 + 15
        )


# ---------------------------------------------------------------------------
# matches_cron
# ---------------------------------------------------------------------------


class TestMatchesCron:
    def test_cron_match_hit(self):
        """VAL-DAEMON-010：匹配。"""
        now = datetime(2026, 9, 6, 3, 0, 0, tzinfo=timezone.utc)  # 周日 03:00
        assert matches_cron({"hour": 3, "minute": 0}, now) is True

    def test_cron_match_miss(self):
        """VAL-DAEMON-010：不匹配。"""
        now = datetime(2026, 9, 6, 3, 1, 0, tzinfo=timezone.utc)  # 周日 03:01
        assert matches_cron({"hour": 3, "minute": 0}, now) is False

    def test_cron_match_multi_field(self):
        """VAL-DAEMON-010：多字段 AND。"""
        now_match = datetime(2026, 9, 15, 3, 0, 0, tzinfo=timezone.utc)
        assert matches_cron({"hour": 3, "minute": 0, "day": 15}, now_match) is True

        now_mismatch = datetime(2026, 9, 16, 3, 0, 0, tzinfo=timezone.utc)
        assert matches_cron({"hour": 3, "minute": 0, "day": 15}, now_mismatch) is False

    def test_cron_match_wildcard(self):
        """VAL-DAEMON-010：通配（只给 minute）。"""
        now = datetime(2026, 9, 6, 5, 30, 0, tzinfo=timezone.utc)  # 任意小时 :30
        assert matches_cron({"minute": 30}, now) is True

    def test_cron_match_weekday(self):
        """VAL-DAEMON-010：weekday（cron 惯例 0=周日）。"""
        # 2026-09-06 是周日（cron weekday=0）
        now_sunday = datetime(2026, 9, 6, 3, 0, 0, tzinfo=timezone.utc)
        assert matches_cron({"weekday": 0, "hour": 3, "minute": 0}, now_sunday) is True

        # 2026-09-07 是周一（cron weekday=1）
        now_monday = datetime(2026, 9, 7, 3, 0, 0, tzinfo=timezone.utc)
        assert matches_cron({"weekday": 0, "hour": 3, "minute": 0}, now_monday) is False
        assert matches_cron({"weekday": 1, "hour": 3, "minute": 0}, now_monday) is True


# ---------------------------------------------------------------------------
# interval 调度
# ---------------------------------------------------------------------------


class TestIntervalScheduling:
    def test_interval_task_writes_success_state(self, tmp_path, monkeypatch):
        """VAL-DAEMON-001：interval 调度按配置触发并写契约 v1 状态。"""
        state_path = tmp_path / "state.json"
        marker = tmp_path / "marker.txt"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))
        monkeypatch.setenv("MENCBO_LOG_DIR", str(tmp_path))

        def handler():
            marker.write_text("executed", encoding="utf-8")
            return "ok"

        spec = _spec(
            task_id="test:interval",
            display_name="测试 interval",
            schedule_value={"seconds": 1},
            handler=handler,
        )

        async def run():
            task = asyncio.create_task(_interval_loop(spec, state_path, "test/project"))
            await asyncio.sleep(1.5)  # 等待 1.5s，应触发一次
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass

        asyncio.run(run())

        assert marker.exists()
        data = json.loads(state_path.read_text(encoding="utf-8"))
        assert "test:interval" in data["tasks"]
        task_data = data["tasks"]["test:interval"]
        assert task_data["status"] == "success"
        assert isinstance(task_data["duration_ms"], int)
        assert task_data["duration_ms"] >= 0
        assert datetime.fromisoformat(task_data["last_run"]).tzinfo is not None
        assert datetime.fromisoformat(data["updated_at"]).tzinfo is not None

    def test_running_state_observable_before_completion(self, tmp_path, monkeypatch):
        """VAL-DAEMON-002：执行前置 running 状态可观测。"""
        state_path = tmp_path / "state.json"
        gate = tmp_path / "gate.txt"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        def slow_handler():
            # 等待 gate 文件出现
            while not gate.exists():
                time.sleep(0.05)
            return "done"

        spec = _spec(
            task_id="test:slow",
            display_name="测试慢任务",
            handler=slow_handler,
            timeout_sec=10,
        )

        async def run():
            task = asyncio.create_task(_execute_task(spec, state_path, "test/project"))

            # 等待 running 状态
            for _ in range(50):  # 最多等 2.5s
                await asyncio.sleep(0.05)
                if state_path.exists():
                    data = json.loads(state_path.read_text(encoding="utf-8"))
                    if data.get("tasks", {}).get("test:slow", {}).get("status") == "running":
                        # 验证 running 中间态
                        task_data = data["tasks"]["test:slow"]
                        assert task_data["status"] == "running"
                        assert task_data["duration_ms"] is None
                        assert task_data["error"] is None
                        break
            else:
                pytest.fail("未观测到 running 状态")

            # 放行 handler
            gate.write_text("go", encoding="utf-8")
            await task

            # 验证终态
            data = json.loads(state_path.read_text(encoding="utf-8"))
            task_data = data["tasks"]["test:slow"]
            assert task_data["status"] == "success"
            assert isinstance(task_data["duration_ms"], int)
            assert task_data["duration_ms"] >= 0

        asyncio.run(run())

    def test_failed_handler_records_error(self, tmp_path, monkeypatch):
        """VAL-DAEMON-003：失败 handler 记 failed 且 error 非空。"""
        state_path = tmp_path / "state.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        def failing_handler():
            raise RuntimeError("演示失败")

        spec = _spec(
            task_id="test:fail",
            display_name="测试失败",
            handler=failing_handler,
        )

        asyncio.run(_execute_task(spec, state_path, "test/project"))

        data = json.loads(state_path.read_text(encoding="utf-8"))
        task_data = data["tasks"]["test:fail"]
        assert task_data["status"] == "failed"
        assert isinstance(task_data["duration_ms"], int)
        assert task_data["duration_ms"] >= 0
        assert task_data["error"] is not None
        assert "演示失败" in task_data["error"]
        assert data["health"] == "degraded"

    def test_timeout_truncates_and_marks_failed(self, tmp_path, monkeypatch):
        """VAL-DAEMON-004：超时任务被截断并记 failed。"""
        state_path = tmp_path / "state.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        def very_slow_handler():
            time.sleep(5)
            return "done"

        spec = _spec(
            task_id="test:timeout",
            display_name="测试超时",
            handler=very_slow_handler,
            timeout_sec=1,
        )

        async def run_and_check():
            # 启动任务但不等待它完成
            task = asyncio.create_task(_execute_task(spec, state_path, "test/project"))
            
            # 轮询状态文件，等待 failed 状态被写入（预算 ≤3s）
            start = time.time()
            while time.time() - start < 3.0:
                if state_path.exists():
                    data = json.loads(state_path.read_text(encoding="utf-8"))
                    task_data = data.get("tasks", {}).get("test:timeout", {})
                    if task_data.get("status") == "failed":
                        # 找到了 failed 状态，取消后台任务
                        task.cancel()
                        try:
                            await task
                        except asyncio.CancelledError:
                            pass
                        return task_data
                
                await asyncio.sleep(0.1)
            
            # 超时前取消任务
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass
            
            pytest.fail("3s 内未检测到 failed 状态")

        task_data = asyncio.run(run_and_check())
        
        # 验证 failed 状态的内容
        assert task_data["status"] == "failed"
        assert isinstance(task_data["duration_ms"], int)
        assert task_data["error"] is not None
        assert "超时" in task_data["error"]


# ---------------------------------------------------------------------------
# enabled=False
# ---------------------------------------------------------------------------


class TestDisabledTask:
    def test_disabled_task_never_runs(self, tmp_path, monkeypatch):
        """VAL-DAEMON-009：enabled=False 的任务不注册不执行。"""
        state_path = tmp_path / "state.json"
        marker_disabled = tmp_path / "marker_disabled.txt"
        marker_enabled = tmp_path / "marker_enabled.txt"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        def disabled_handler():
            marker_disabled.write_text("executed", encoding="utf-8")

        def enabled_handler():
            marker_enabled.write_text("executed", encoding="utf-8")

        spec_disabled = _spec(
            task_id="test:disabled",
            display_name="禁用任务",
            schedule_value={"seconds": 1},
            handler=disabled_handler,
            enabled=False,
        )
        spec_enabled = _spec(
            task_id="test:enabled",
            display_name="启用任务",
            schedule_value={"seconds": 1},
            handler=enabled_handler,
            enabled=True,
        )

        async def run():
            task = asyncio.create_task(run_forever([spec_disabled, spec_enabled]))
            await asyncio.sleep(1.5)
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass

        # 初始化 state.json
        from mencbo.core.state_writer import _blank_state, write_state

        write_state(_blank_state(), state_path)

        asyncio.run(run())

        # 禁用任务不应执行
        assert not marker_disabled.exists()
        # 启用任务应执行
        assert marker_enabled.exists()

        # 禁用任务不应出现在 state.json
        data = json.loads(state_path.read_text(encoding="utf-8"))
        assert "test:disabled" not in data.get("tasks", {})
        assert "test:enabled" in data.get("tasks", {})
