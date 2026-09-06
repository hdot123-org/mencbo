"""数据契约层：Phase 1 定义的三份契约中的两份（git_scope、TaskSpec）。"""

from mencbo.contracts.git_scope import GitScopeInfo, normalize_project_id, resolve_git_scope
from mencbo.contracts.task_spec import TaskSpec

__all__ = ["GitScopeInfo", "TaskSpec", "normalize_project_id", "resolve_git_scope"]
