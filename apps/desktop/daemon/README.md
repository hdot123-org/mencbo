# mencbo（守护器）

MenCbo 桌面调度宿主的 Python 后台守护器。当前为 Phase 1：基础契约与 Git 作用域解析。

> Tauri 控制台客户端（Phase 3）与 DMG 交付（Phase 4）后续进入同一工作区。

## 架构位置

- **契约 1 · Git 作用域**：`mencbo.contracts.git_scope` —— 锁定 GitHub `owner/repo` 为唯一物理命名空间；分支/提交仅作元数据记录，不干预分支逻辑。
- **契约 2 · 任务规范**：`mencbo.contracts.task_spec` —— `TaskSpec` dataclass；各子项目（memory、infra-core）通过自身 `tasks.py` 暴露符合规范的清单，由 Phase 2 的统一调度中心动态加载。
- **契约 3 · 状态快照**：`~/.mencbo/state.json`，Phase 2 调度器落地后输出。

## 开发

```bash
uv sync          # 创建 .venv 并安装项目与 dev 依赖
uv run pytest    # 运行全部测试
uv run python -c "from mencbo.contracts.git_scope import resolve_git_scope; print(resolve_git_scope('.'))"
```

## Phase 1 模块

| 模块 | 职责 |
| --- | --- |
| `src/mencbo/contracts/task_spec.py` | 统一任务规范 `TaskSpec`（构造期校验） |
| `src/mencbo/contracts/git_scope.py` | `resolve_git_scope()` 作用域解析 + remote URL 归一化 |
| `src/mencbo/core/example_tasks.py` | 示例任务清单 `get_tasks()`，供 Phase 2 调度器联调 |

零运行时依赖，Python >= 3.12。
