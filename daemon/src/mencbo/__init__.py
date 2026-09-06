"""mencbo —— MenCbo 桌面调度宿主守护器。

Phase 1：基础契约（TaskSpec、GitScopeInfo）与 Git 作用域解析。
"""

from mencbo.contracts.git_scope import GitScopeInfo, resolve_git_scope
from mencbo.contracts.task_spec import TaskSpec

__version__ = "0.1.0"

__all__ = ["GitScopeInfo", "TaskSpec", "resolve_git_scope", "__version__"]
