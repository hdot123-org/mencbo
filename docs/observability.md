# 可观测性规范（PostHog）

项目：**MenCbo Desktop**（PostHog US，项目名 `MenCbo Desktop`）。
App 内嵌 capture-only key（公开是设计使然，伪造事件最多污染匿名遥测，无安全边界影响）。

## 硬规则：功能必带监控

> **任何新功能合入前，必须至少带两个事件：入口事件（证明被触发）+ 结果事件（证明成功或失败）。**
> PR review 检查项之一：`invoke_handler` 新增命令或新 UI 动作，是否有对应埋点。

事件命名：snake_case。原生层生命周期事件带 `rust_` 前缀，webview 层带 `js_` 前缀，功能事件不带前缀（按功能命名）。

## 事件清单

### 生命周期与假死差分诊断

| 事件 | 层 | 触发 | 属性 |
|---|---|---|---|
| `rust_launch` | 原生 | 进程启动 | `version` |
| `js_launch` | webview | 前端初始化 | — |
| `rust_heartbeat` | 原生 | 每 5 min | — |
| `js_heartbeat` | webview | 每 5 min | `uptime_sec` |
| `quit` / `tray_menu{item:quit}` | 原生 | 退出（尽力而为，可能丢失） | `via` |

**假死诊断矩阵**（心跳 + 交互事件差分）：

| 现象 | 结论 |
|---|---|
| `rust_heartbeat` 持续，`js_heartbeat` + `$autocapture` 断流 | webview 层假死（主线程阻塞） |
| `tray_click`/`tray_menu` 持续，`$autocapture`/`panel_open` 断流 | 同上（原生交互正常、页面死了） |
| 两者都断，进程仍在 | 原生层假死（查看系统日志/样本进程） |
| 全部停止且进程退出 | 崩溃（对照 `rust_launch` 时间线） |

### 面板与交互

| 事件 | 层 | 触发 | 属性 |
|---|---|---|---|
| `$autocapture` | webview | webview 内任意点击（autocapture） | 元素信息（tag/text/class） |
| `tray_click` | 原生 | 托盘图标左键点按 | `action: open\|hide` |
| `tray_menu` | 原生 | 托盘菜单项 | `item: check-update\|quit` |
| `panel_open` / `panel_close` | webview | 面板显示/隐藏（经 panel-event） | — |
| `panel_blur_hide` | 原生 | 失焦自动隐藏 | — |

### 状态链路

| 事件 | 层 | 触发 | 属性 |
|---|---|---|---|
| `js_state_loaded` | webview | 启动首次加载成功 | `tasks`, `duration_ms` |
| `state_load_failed` | webview | 首次加载失败 | `reason` |
| `state_changed` | 原生 | watcher 检测到 daemon 写入 state.json | `tasks`, `mock` |
| `js_state_updated` | webview | 收到 state-changed 推送 | `tasks` |

`state_changed`（原生发出）与 `js_state_updated`（前端收到）差分 = Tauri 事件管道是否阻塞。

### 功能动作

| 事件 | 层 | 触发 | 属性 |
|---|---|---|---|
| `run_task` | 原生 | 运行任务命令被调用 | `task` |
| `run_task_spawn_failed` | 原生 | uv 进程拉起失败 | `task`, `error` |
| `open_logs` / `open_logs_failed` | 原生 | 打开日志目录 | `stage`, `error`（失败时） |
| `updater_check` | 原生 | 更新检查开始 | `source: auto\|manual\|tray` |
| `updater_uptodate` | 原生 | 检查后无更新 | `source` |
| `updater_installed` | 原生 | 更新装完（即将重启） | `to`, `source` |
| `updater_error` | 原生 | 任一阶段失败 | `stage: builder\|check\|install`, `detail`, `source` |
| `js_error` / `js_unhandled_rejection` | webview | 前端异常 | `message`/`reason` |

## 查询入口

HogQL 查询 API（PAT 认证）：

```
POST /api/projects/<PROJECT_ID>/query
{"query": {"kind": "HogQLQuery", "query": "SELECT event, count() FROM events WHERE timestamp > now() - interval 1 day GROUP BY event"}}
```

注意：`query` 字段必须是对象；`/api/projects/<id>/events/` 列表端点不可用（持续 server error）。

## 验证（Validation）

### 单实例守卫（Single-Instance Guard）

接入 Tauri 2 官方 `tauri-plugin-single-instance` 插件：同一 bundle identifier 已运行时，再次启动聚焦既有主窗口/面板后立即退出，不产生可见双实例。

**实现**：`src-tauri/src/lib.rs` 中 `.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| { ... }))` 注册回调，二次启动时 `panel.show()` + `panel.set_focus()`，插件随后自动终止新进程。

**测试构建隔离**：测试/开发构建使用 `tauri.conf.dev.json` 覆盖 identifier 为 `com.mencbo.desktop.dev`，确保与用户正式实例（`com.mencbo.desktop`）互不冲突、互不抢占。使用方式：

```bash
# 开发构建：使用 dev identifier（不抢占生产实例）
pnpm -F desktop-client tauri dev --config src-tauri/tauri.conf.dev.json

# 构建 dev bundle
pnpm -F desktop-client tauri build --config src-tauri/tauri.conf.dev.json
```

Dev 构建使用独立 `app_data_dir`（`com.mencbo.desktop.dev`），`install_id` 独立生成，PostHog 查询按该 `distinct_id` 过滤。

**VAL-SI-001 验证**（2026-09-07 release 构建实测）：
- 启动 release 产物（PID 41105，`target/release/bundle/macos/MenCbo.app`）
- 二次启动同 identifier 产物（PID 41311）→ 5s 后 PID 41311 已退出
- 仅剩 1 个 `desktop-client` 进程（原始实例）
- 清理：`kill 41105`，验证无残留
