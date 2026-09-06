"""Git 作用域解析器（契约 1）。

锁定 GitHub 仓库（``owner/repo``）为唯一物理命名空间；分支与提交仅作
元数据日志记录，不干预分支逻辑。任何失败（非 Git 仓库、无 remote、
git 不可用、超时……）一律优雅降级为 ``is_locked=False``，绝不抛出异常。
"""

from __future__ import annotations

import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

_GIT_TIMEOUT_SEC = 10


@dataclass(slots=True)
class GitScopeInfo:
    """Git 作用域快照（契约 1）。

    Attributes:
        project_id: 归一化的 ``owner/repo`` 命名空间；无法解析时为 None。
        repo_root: 仓库顶层绝对路径；非 Git 仓库时为 None。
        current_branch: 当前分支名（纯元数据）；detached HEAD 时为 None。
        current_commit: 当前提交 SHA（纯元数据）；空仓库（unborn HEAD）
            时为 None。
        is_locked: 是否成功锁定 GitHub 命名空间。
    """

    project_id: str | None = None
    repo_root: str | None = None
    current_branch: str | None = None
    current_commit: str | None = None
    is_locked: bool = False

    def to_dict(self) -> dict:
        """导出为契约 1 的 JSON 形状。"""
        return {
            "project_id": self.project_id,
            "repo_root": self.repo_root,
            "current_branch": self.current_branch,
            "current_commit": self.current_commit,
            "is_locked": self.is_locked,
        }


def normalize_project_id(remote_url: str) -> str | None:
    """将常见形式的 Git remote URL 归一化为 ``owner/repo``。

    支持 ``https://github.com/owner/repo.git``、带凭据前缀的 https、
    scp 形式 ``git@github.com:owner/repo.git``、
    ``ssh://git@github.com/owner/repo.git`` 等；无法解析时返回 None。
    """
    s = remote_url.strip()
    if not s:
        return None

    scheme = re.match(r"^[A-Za-z][A-Za-z0-9+.-]*://", s)
    if scheme:
        s = s[scheme.end() :]
        s = re.sub(r"^[^/@]+@", "", s)  # 去掉 user:pass@ / token@ 凭据
    else:
        s = re.sub(r"^[^/@]+@", "", s)  # scp 形式的 git@host:
        s = s.replace(":", "/", 1)  # host:owner/repo -> host/owner/repo

    s = s.rstrip("/")
    if s.endswith(".git"):
        s = s[: -len(".git")]

    parts = [p for p in s.split("/") if p]
    if len(parts) < 3:  # 至少 host/owner/repo
        return None
    owner, repo = parts[-2], parts[-1]
    if not owner or not repo:
        return None
    return f"{owner}/{repo}"


def _git(args: list[str], cwd: Path) -> str | None:
    """运行 git 子命令；git 缺失、超时或非零退出码一律返回 None。"""
    try:
        proc = subprocess.run(
            ["git", *args],
            cwd=str(cwd),
            capture_output=True,
            text=True,
            timeout=_GIT_TIMEOUT_SEC,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired, ValueError):
        return None
    if proc.returncode != 0:
        return None
    return proc.stdout.strip() or None


def resolve_git_scope(cwd: str | Path | None = None) -> GitScopeInfo:
    """解析 ``cwd`` 所在 Git 仓库的作用域。

    - 取 ``remote.origin.url`` 归一化出 ``project_id``（owner/repo）；
    - HEAD 提交与当前分支仅作元数据记录；
    - 非 Git 目录、路径不存在、无 remote 等情形优雅降级为
      ``is_locked=False``，绝不抛出异常。
    """
    try:
        target = Path(cwd).expanduser() if cwd is not None else Path.cwd()
    except (RuntimeError, OSError, ValueError):
        return GitScopeInfo()
    if not target.is_dir():
        return GitScopeInfo()

    try:
        toplevel = _git(["rev-parse", "--show-toplevel"], target)
        if toplevel is None:
            return GitScopeInfo()
        repo_root = Path(toplevel).resolve()

        remote_url = _git(["config", "--get", "remote.origin.url"], repo_root)
        project_id = normalize_project_id(remote_url) if remote_url else None

        return GitScopeInfo(
            project_id=project_id,
            repo_root=str(repo_root),
            current_branch=_git(["symbolic-ref", "--short", "HEAD"], repo_root),
            current_commit=_git(["rev-parse", "HEAD"], repo_root),
            is_locked=project_id is not None,
        )
    except Exception:  # 契约要求：任何意外都降级，不向上抛出
        return GitScopeInfo()
