"""测试：state_writer（契约 v1 读写器）。

覆盖断言：
- VAL-DAEMON-005：原子写并发 reader 不读半截
- VAL-DAEMON-006：MENCBO_STATE_PATH 重定向 + 默认路径零污染
- VAL-DAEMON-007：project_id 解析（fixture / real repo / fallback）
- VAL-DAEMON-008：health 汇总规则
"""

from __future__ import annotations

import json
import subprocess
import threading
from datetime import datetime
from pathlib import Path

import pytest

from mencbo.core.state_writer import (
    _blank_state,
    compute_health,
    read_state,
    resolve_project_id,
    resolve_state_path,
    set_task_state,
    write_state,
)


def _git(cwd: Path, *args: str) -> None:
    subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True, text=True
    )


def _init_repo(
    path: Path, *, remote: str | None = None, with_commit: bool = True
) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    _git(path, "init", "-b", "main")
    _git(path, "config", "user.email", "dev@mencbo.local")
    _git(path, "config", "user.name", "mencbo-dev")
    if with_commit:
        (path / "README.md").write_text("# fixture\n", encoding="utf-8")
        _git(path, "add", ".")
        _git(path, "commit", "-m", "fixture: initial commit")
    if remote is not None:
        _git(path, "remote", "add", "origin", remote)
    return path


# ---------------------------------------------------------------------------
# 路径解析
# ---------------------------------------------------------------------------


class TestResolveStatePath:
    def test_state_path_env_redirect(self, tmp_path, monkeypatch):
        """VAL-DAEMON-006：MENCBO_STATE_PATH 重定向生效。"""
        redirect = tmp_path / "custom" / "state.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(redirect))
        assert resolve_state_path() == redirect

    def test_default_path_under_home(self, tmp_path, monkeypatch):
        """VAL-DAEMON-006：不设 MENCBO_STATE_PATH 时写在 HOME 下。"""
        monkeypatch.delenv("MENCBO_STATE_PATH", raising=False)
        monkeypatch.setattr(Path, "home", lambda: tmp_path)
        result = resolve_state_path()
        assert result == tmp_path / ".mencbo" / "state.json"


# ---------------------------------------------------------------------------
# 读写 + 原子写
# ---------------------------------------------------------------------------


class TestReadWrite:
    def test_write_creates_file(self, tmp_path, monkeypatch):
        state_path = tmp_path / "state.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        state = _blank_state()
        write_state(state, state_path)

        assert state_path.exists()
        data = json.loads(state_path.read_text(encoding="utf-8"))
        assert set(data) >= {"updated_at", "project_id", "health", "tasks"}

    def test_set_task_state_preserves_others(self, tmp_path, monkeypatch):
        """VAL-DAEMON-012 基础：读-改-写不丢数据。"""
        state_path = tmp_path / "state.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        state = _blank_state()
        set_task_state(
            state, "a:1", display_name="任务A", status="success", duration_ms=10
        )
        set_task_state(
            state, "b:2", display_name="任务B", status="success", duration_ms=20
        )
        write_state(state, state_path)

        data = json.loads(state_path.read_text(encoding="utf-8"))
        assert "a:1" in data["tasks"]
        assert "b:2" in data["tasks"]
        assert data["tasks"]["a:1"]["status"] == "success"
        assert data["tasks"]["b:2"]["status"] == "success"

    def test_iso8601_with_timezone(self, tmp_path, monkeypatch):
        """时间字段带 UTC 偏移。"""
        state_path = tmp_path / "state.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        state = _blank_state()
        set_task_state(state, "x:1", display_name="X", status="success", duration_ms=5)
        write_state(state, state_path)

        data = json.loads(state_path.read_text(encoding="utf-8"))
        dt = datetime.fromisoformat(data["tasks"]["x:1"]["last_run"])
        assert dt.tzinfo is not None
        dt2 = datetime.fromisoformat(data["updated_at"])
        assert dt2.tzinfo is not None

    def test_concurrent_reader_never_sees_partial_json(self, tmp_path, monkeypatch):
        """VAL-DAEMON-005：200 次写盘并发 reader 0 次解析失败。"""
        state_path = tmp_path / "state.json"
        monkeypatch.setenv("MENCBO_STATE_PATH", str(state_path))

        parse_errors = 0
        reads = 0
        stop = False

        def reader():
            nonlocal parse_errors, reads
            while not stop:
                try:
                    text = state_path.read_text(encoding="utf-8")
                    json.loads(text)
                    reads += 1
                except FileNotFoundError:
                    pass  # 首写前允许
                except json.JSONDecodeError:
                    parse_errors += 1
                    reads += 1

        reader_thread = threading.Thread(target=reader, daemon=True)
        reader_thread.start()

        # 写 200 次，交替 success/failed
        for i in range(200):
            state = _blank_state()
            status = "success" if i % 2 == 0 else "failed"
            set_task_state(
                state, "x:1", display_name="X", status=status, duration_ms=i
            )
            write_state(state, state_path)

        stop = True
        reader_thread.join(timeout=2)

        assert parse_errors == 0, f"reads={reads}, parse_errors={parse_errors}"
        # 无 tmp 残留
        remaining = list(tmp_path.glob("*.tmp"))
        assert len(remaining) == 0, f"tmp 残留: {remaining}"


# ---------------------------------------------------------------------------
# health 汇总
# ---------------------------------------------------------------------------


class TestHealthAggregation:
    def test_health_aggregation(self):
        """VAL-DAEMON-008：全 success / 混合 / 含 failed / 空 tasks 四组合。"""
        assert compute_health({}) == "ok"

        tasks_all_success = {
            "a": {"status": "success"},
            "b": {"status": "success"},
        }
        assert compute_health(tasks_all_success) == "ok"

        tasks_mixed = {
            "a": {"status": "success"},
            "b": {"status": "running"},
        }
        assert compute_health(tasks_mixed) == "ok"

        tasks_with_failed = {
            "a": {"status": "success"},
            "b": {"status": "failed"},
        }
        assert compute_health(tasks_with_failed) == "degraded"


# ---------------------------------------------------------------------------
# project_id 解析
# ---------------------------------------------------------------------------


class TestProjectId:
    def test_project_id_from_git_scope_fixture(self, tmp_path):
        """VAL-DAEMON-007(a)：受控 fixture。"""
        repo = _init_repo(
            tmp_path / "repo",
            remote="https://github.com/hdot123-org/mencbo.git",
        )
        pid = resolve_project_id(repo)
        assert pid == "hdot123-org/mencbo"

    def test_project_id_from_git_scope_real_repo(self):
        """VAL-DAEMON-007(b)：真实仓库。"""
        real_repo = Path(__file__).resolve().parents[4]  # mencbo/mencbo
        pid = resolve_project_id(real_repo)
        assert pid == "hdot123-org/mencbo"

    def test_project_id_from_git_scope_fallback(self, tmp_path):
        """VAL-DAEMON-007(c)：非 git 目录降级 unknown。"""
        plain = tmp_path / "plain"
        plain.mkdir()
        pid = resolve_project_id(plain)
        assert pid == "unknown"
