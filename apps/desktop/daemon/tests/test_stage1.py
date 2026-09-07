"""Phase 1 单元测试：TaskSpec 契约（契约 2）与 Git 作用域解析器（契约 1）。"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from mencbo.contracts.git_scope import (
    GitScopeInfo,
    normalize_project_id,
    resolve_git_scope,
)
from mencbo.contracts.task_spec import TaskSpec
from mencbo.core.example_tasks import get_tasks, write_heartbeat_log


def _git(cwd: Path, *args: str) -> None:
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True)


def _init_repo(path: Path, *, remote: str | None = None, with_commit: bool = True) -> Path:
    """在临时目录构造一个可控的 Git 仓库夹具。"""
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


def _spec(**overrides) -> TaskSpec:
    defaults: dict = dict(
        task_id="example:demo",
        display_name="示例任务",
        schedule_type="interval",
        schedule_value={"hours": 4},
        handler=lambda: None,
    )
    defaults.update(overrides)
    return TaskSpec(**defaults)


# ---------------------------------------------------------------------------
# 契约 2：TaskSpec
# ---------------------------------------------------------------------------


class TestTaskSpecContract:
    def test_defaults(self):
        spec = _spec()
        assert spec.timeout_sec == 300
        assert spec.enabled is True

    def test_cron_variant(self):
        spec = _spec(schedule_type="cron", schedule_value={"hour": 19, "minute": 0})
        assert spec.schedule_value == {"hour": 19, "minute": 0}

    def test_invalid_schedule_type(self):
        with pytest.raises(ValueError, match="schedule_type"):
            _spec(schedule_type="daily")

    def test_empty_schedule_value(self):
        with pytest.raises(ValueError, match="schedule_value"):
            _spec(schedule_value={})

    def test_unknown_interval_key(self):
        with pytest.raises(ValueError, match="不支持的字段"):
            _spec(schedule_value={"hours": 1, "lightyears": 2})

    def test_non_int_schedule_value(self):
        with pytest.raises(ValueError, match="非负整数"):
            _spec(schedule_value={"hours": "4"})

    def test_non_callable_handler(self):
        with pytest.raises(TypeError, match="handler"):
            _spec(handler="not-callable")

    def test_blank_task_id(self):
        with pytest.raises(ValueError, match="task_id"):
            _spec(task_id="   ")

    def test_task_id_with_whitespace(self):
        with pytest.raises(ValueError, match="task_id"):
            _spec(task_id="bad id")

    def test_invalid_timeout(self):
        with pytest.raises(ValueError, match="timeout_sec"):
            _spec(timeout_sec=0)


# ---------------------------------------------------------------------------
# 契约 1：resolve_git_scope
# ---------------------------------------------------------------------------


class TestResolveGitScope:
    def test_locked_repo_over_https(self, tmp_path):
        repo = _init_repo(
            tmp_path / "repo", remote="https://github.com/hdot123-org/memory.git"
        )
        scope = resolve_git_scope(repo)
        assert scope.is_locked is True
        assert scope.project_id == "hdot123-org/memory"
        assert scope.current_branch == "main"
        assert scope.current_commit is not None
        assert len(scope.current_commit) == 40
        assert Path(scope.repo_root) == repo.resolve()

    def test_scope_to_dict_shape(self, tmp_path):
        repo = _init_repo(
            tmp_path / "repo", remote="https://github.com/hdot123-org/memory.git"
        )
        payload = resolve_git_scope(repo).to_dict()
        assert set(payload) == {
            "project_id",
            "repo_root",
            "current_branch",
            "current_commit",
            "is_locked",
        }
        assert payload["project_id"] == "hdot123-org/memory"
        assert payload["is_locked"] is True

    def test_subdirectory_resolves_to_toplevel(self, tmp_path):
        repo = _init_repo(
            tmp_path / "repo", remote="https://github.com/hdot123-org/memory.git"
        )
        nested = repo / "src" / "deep"
        nested.mkdir(parents=True)
        assert resolve_git_scope(nested).repo_root == str(repo.resolve())

    def test_scp_remote_normalized(self, tmp_path):
        repo = _init_repo(
            tmp_path / "repo", remote="git@github.com:hdot123-org/infra-core.git"
        )
        assert resolve_git_scope(repo).project_id == "hdot123-org/infra-core"

    def test_plain_directory_unlocked(self, tmp_path):
        plain = tmp_path / "plain"
        plain.mkdir()
        scope = resolve_git_scope(plain)
        assert scope.is_locked is False
        assert scope.project_id is None
        assert scope.repo_root is None

    def test_repo_without_remote_unlocked(self, tmp_path):
        repo = _init_repo(tmp_path / "repo", remote=None)
        scope = resolve_git_scope(repo)
        assert scope.is_locked is False
        assert scope.project_id is None
        assert scope.current_branch == "main"  # 元数据仍尽力提供

    def test_fresh_repo_unborn_head(self, tmp_path):
        repo = _init_repo(
            tmp_path / "repo",
            remote="https://github.com/hdot123-org/memory.git",
            with_commit=False,
        )
        scope = resolve_git_scope(repo)
        assert scope.is_locked is True
        assert scope.current_commit is None
        assert scope.current_branch == "main"

    def test_nonexistent_path_never_raises(self):
        scope = resolve_git_scope("/definitely/not/a/real/path")
        assert isinstance(scope, GitScopeInfo)
        assert scope.is_locked is False

    def test_string_path(self, tmp_path):
        repo = _init_repo(
            tmp_path / "repo", remote="https://github.com/hdot123-org/memory.git"
        )
        assert resolve_git_scope(str(repo)).is_locked is True


class TestNormalizeProjectId:
    @pytest.mark.parametrize(
        ("url", "expected"),
        [
            ("https://github.com/hdot123-org/memory.git", "hdot123-org/memory"),
            ("https://github.com/hdot123-org/memory", "hdot123-org/memory"),
            (
                "https://x-access-token:ghp_secret@github.com/hdot123-org/memory.git",
                "hdot123-org/memory",
            ),
            ("git@github.com:hdot123-org/memory.git", "hdot123-org/memory"),
            ("git@github.com:hdot123-org/memory", "hdot123-org/memory"),
            ("ssh://git@github.com/hdot123-org/memory.git", "hdot123-org/memory"),
            ("", None),
            ("not-a-url", None),
        ],
    )
    def test_variants(self, url, expected):
        assert normalize_project_id(url) == expected


# ---------------------------------------------------------------------------
# 示例任务清单
# ---------------------------------------------------------------------------


class TestExampleTasks:
    def test_registry_shape(self):
        specs = get_tasks()
        assert len(specs) >= 2
        ids = [spec.task_id for spec in specs]
        assert len(set(ids)) == len(ids)  # task_id 全局唯一
        for spec in specs:
            assert callable(spec.handler)
            assert spec.enabled is True
            assert spec.task_id.startswith("example:")

    def test_handler_writes_log(self, tmp_path, monkeypatch):
        monkeypatch.setenv("MENCBO_LOG_DIR", str(tmp_path))
        path = Path(write_heartbeat_log())
        assert path.exists()
        assert path.read_text(encoding="utf-8").strip() != ""

    def test_registry_handlers_executable(self, tmp_path, monkeypatch):
        monkeypatch.setenv("MENCBO_LOG_DIR", str(tmp_path))
        for spec in get_tasks():
            if spec.task_id == "example:failure-demo":
                # failure-demo 故意抛异常演示 failed 状态
                with pytest.raises(RuntimeError, match="示例失败"):
                    spec.handler()
            else:
                result = spec.handler()
                assert Path(result).exists()
