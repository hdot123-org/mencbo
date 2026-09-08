# MenCbo Desktop Observability Standard v1.1

> **Source**: 监控标准 v1.1, 2026-09-07, mission library
> **Single source of truth for events**: `apps/desktop/analytics/events.yml`
> **Date**: 2026-09-07

---

## 1. Identity Model

| Identity | Format | Authority | Notes |
|----------|--------|-----------|-------|
| `distinct_id` | `desktop-{uuidv4}` | Rust | Persists in `app_data_dir/install_id` (atomic write). Replaces legacy `mencbo-{USER}`. |
| `session_id` | `launch_id` (uuidv7) | Rust | Generated per startup, bridges to webview via Tauri command. |
| User identity | via `setPersonProperties` | Both | Optional; use `$create_alias` if needed. Never call `identify()` alone. |

**Status**: Implemented in P0 (PR #41). Identity unified as `desktop-{uuidv4}` with environment gating.

### Identity Injection (P0)

```rust
#[tauri::command]
fn analytics_identity(app: AppHandle) -> Value {
    json!({ "installId": install_id(&app), "sessionId": launch_id(&app) })
}
```

```typescript
const id = await invoke('analytics_identity');
posthog.init(PH_KEY, {
  bootstrap: { distinctID: id.installId, isIdentifiedID: true },
});
posthog.register({ session_id: id.sessionId });
```

---

## 2. Naming Convention

- **Lifecycle/diagnostic events**: `rust_` or `js_` prefix (e.g., `rust_launch`, `js_error`)
- **Business events**: `[module]_[action]` format, no prefix (e.g., `panel_open`, `task_run`)
- **Diagnostic failures**: `diag_` prefix (e.g., `diag_task_spawn_failed`)
- **Format**: `snake_case` only; no camelCase, no kebab-case
- **Reserved**: No `$` prefix (PostHog system reserved)

---

## 3. Property Contract

All events **must** include baseline properties (P0 implemented in PR #41):

| Property | Type | Example | Description |
|----------|------|---------|-------------|
| `app_version` | string | `"0.2.4"` | SemVer from `tauri.conf.json` |
| `environment` | string | `"production"` | `production` / `development` |
| `session_id` | string | uuidv7 | Current launch ID |
| `source` | string | `"rust_native"` / `"webview"` | Event origin layer |
| `platform` | string | `"macos"` / `"windows"` / `"linux"` | Operating system |
| `arch` | string | `"aarch64"` / `"x86_64"` | CPU architecture |
| `identity_degraded` | boolean | `false` | (JS side) True when install_id IO failed at startup |

**Additional context properties** (automatically attached by Rust layer):
- `os_version`: OS version string (e.g., `"14.5"` for macOS, `"10.0.19045"` for Windows). Provided by `os_info` crate.
- `platform_arch`: Combined identifier for easier filtering (e.g., `"macos-aarch64"`)

**Event-specific properties**:
- `status`: `success` / `failure`
- `duration_ms`, `error_code`, `target_id`

**Error sanitization**: `error` / `detail` fields ≤300 chars, no absolute paths (PII).

**Identity Degraded Mode** (fix-sentinel-degraded-flag): When Rust's install_id IO fails, identity is set to sentinel value `analytics-disabled`. The `analytics_identity` bridge returns `degraded: true`, causing JS to skip bootstrap (avoiding collapsing all degraded installs into a single fake `distinct_id`). All JS-side events from degraded installs carry `identity_degraded: true` as a baseline property. Rust-side capture becomes no-op in degraded mode.

---

## 4. Environment Gate

| Build | Behavior |
|-------|----------|
| Debug (`cfg!(debug_assertions)` / `import.meta.env.DEV`) | **No-op** — no events sent |
| Release | Full reporting with `environment: "production"` |

**Status**: Implemented in P0 (PR #41). Debug builds no-op; release builds report with `environment: "production"`.

---

## 5. Event Catalog

> **Authoritative source**: `apps/desktop/analytics/events.yml`
> This section mirrors the registry for quick reference.

### Lifecycle Events

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `rust_launch` | rust | Process startup | `version` |
| `rust_exit` | rust | Process exit | `reason: normal\|dirty\|abnormal`, `via: command\|tray_menu\|system`, `uptime_s` |
| `rust_heartbeat` | rust | Every 5 min | `seq` (monotonic within session, resets on restart) |
| `js_launch` | webview | Webview init | — |
| `js_heartbeat` | webview | Every 5 min (visible period only) | `uptime_sec`, `seq` (monotonic within visible window, resets on context rebuild) |

**Heartbeat sequence gap detection**:
- **`rust_heartbeat`** (process-level): seq is monotonic across the entire process lifecycle. A gap where `seq[n+1] - seq[n] > 1` combined with a timestamp gap ≥10 minutes indicates genuine event loss.
- **`js_heartbeat`** (visibility-period, 2026-09-08 user ruling): seq is monotonic **only within each visible window** (panel shown). When the panel is hidden, macOS suspends WKWebView (App Nap), freezing JS timers — this is normal behavior, not event loss. Across hidden windows, seq resets to 0 (JS context rebuilt). Gap detection applies only within visible windows: seq gap ≥10 minutes without filling = suspected event loss. Hidden-period gaps are exempt.

See §6.1 for HogQL detection query.

**Exit event semantics (architecture decision 4)**:
- **Normal exit** (`reason: normal`): Triggered by user-initiated quit (tray menu, window close). Rust clears the session marker, emits `rust_exit` with `uptime_s` (seconds since launch), and performs synchronous flush (≤3s timeout).
- **Dirty exit** (`reason: dirty`): Detected on next startup when a session marker from a previous session is found (indicating crash/SIGKILL). Rust emits `rust_exit` for the **previous** session with `prev_session_id` and estimated `uptime_s` (calculated from marker's `started_at` timestamp). This reconstructs the missing exit event.
- **Abnormal exit** (`reason: abnormal`): Reserved for future use (e.g., panic hook). Currently not emitted; dirty exit detection handles crash recovery.
- **SIGTERM/SIGKILL**: Always treated as dirty exit. No signal handler is installed (architecture decision: release builds use `panic="abort"`, preventing in-process signal handling). The killed session's marker file remains; the next startup detects it and emits the dirty exit event.

### Panel Events

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `panel_open` | rust | Panel shown | `via: tray\|reopen`, `panel_id: session_id` |
| `panel_close` | rust | Panel hidden | `via: tray\|blur\|close`, `panel_id: session_id` |

**Panel event paths**:
- `tray`: User clicks tray icon to toggle panel visibility
- `blur`: Panel loses focus (WindowEvent::Focused(false))
- `close`: Window close request (WindowEvent::CloseRequested, prevented and hidden instead)
- `reopen`: Single-instance guard callback — second launch attempt shows existing panel (fix-reopen-panel-open)

All panel events include `panel_id` (derived from session_id) for correlation across open/close pairs.

### State Management

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `state_load` | webview | Initial state loaded | `tasks`, `duration_ms` |
| `diag_state_load_failed` | webview | State load failed | `reason` |
| `state_sync` | both | State file changed | `tasks`, `mock` |

### Task Execution

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `task_run` | rust | Task invoked | `task` |
| `diag_task_spawn_failed` | rust | Spawn failed | `task`, `error` |

### Logging

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `logs_open` | rust | Log dir opened | — |
| `diag_logs_open_failed` | rust | Open failed | `stage: mkdir\|finder`, `error` |

### Auto-Update

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `app_update_check` | rust | Check initiated | `source: auto\|manual\|tray` |
| `app_update_checked` | rust | No update available | `result: uptodate`, `source` |
| `app_update_failed` | rust | Update failed | `stage: builder\|check\|install`, `detail`, `source` |
| `app_update_installed` | rust | Update installed | `to`, `source` |

### Error Events

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `js_error` | webview | Frontend error | `message` |
| `js_unhandled_rejection` | webview | Unhandled rejection | `reason` |
| `diag_identity_failed` | webview | Identity bootstrap failed | `reason` |

### Diagnostic Events (M4 Watchdog — Implemented)

> These events are emitted by the watchdog state machine (watchdog.rs). Only state transitions produce events (not repeated per check cycle).

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `diag_webview_unresponsive` | rust | Webview hang detected (2 consecutive 5s checks with stale JS heartbeat) | `missed_js_beats`, `threshold_s`, `error_code: E_WEBVIEW_UNRESPONSIVE` |
| `diag_webview_recovered` | rust | Webview recovered from hang | `freeze_duration_ms`, `error_code: E_WEBVIEW_RECOVERED` |
| `diag_native_main_unresponsive` | rust | Main thread probe timeout (>10s without run_on_main_thread response) | `stalled_ms`, `error_code: E_MAIN_THREAD_UNRESPONSIVE` |
| `diag_ipc_timeout` | webview | JS invoke() exceeded timeout threshold | `error_code: E_IPC_TIMEOUT`, `timeout_ms` |
| `diag_rust_panic` | rust | Panic hook fires (sync send via blocking reqwest) | `message`, `backtrace`, `error_code: E_RUST_PANIC` |

**Watchdog false-positive protections (2026-09-09 fix)**:
- **Sleep/wake safety**: Main thread probe uses monotonic `Instant` (not `SystemTime`), so system sleep doesn't produce false `diag_native_main_unresponsive`. Sleep/wake detection runs BEFORE native gate in `check_and_transition`, resetting all state.
- **Visibility grace period**: `set_visibility(true)` resets `last_heartbeat` to now, preventing false `diag_webview_unresponsive`+`recovered` pairs after hide→show cycles.

**Session marker semantics**: The session marker (for dirty exit detection) is a single-slot file — N consecutive kills only attribute to the most recent session (each new startup overwrites the marker; only the last unclean session's `prev_session_id` is reported).

### Planned Events (v1.1 Standard)

> Not yet implemented. Tracked for future milestones.

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `diag_webview_terminated` | rust | Watchdog: webview crash/termination | — |
| `app_update_downloaded` | rust | Update binary downloaded | `version`, `source` |

---

## 6. Deprecated Events (Migration Map)

> Renamed in PR #34 (2026-09-07) to align with standard v1.1 §3/§5.

| Old Name | New Name | Rationale |
|----------|----------|-----------|
| `quit` | `rust_exit` | Lifecycle event with `via` attribute |
| `tray_menu{item:quit}` | `rust_exit{via:tray_menu}` | Quit → lifecycle exit |
| `tray_click{action:open}` | `panel_open{via:tray}` | Semantic mapping |
| `tray_click{action:hide}` | `panel_close{via:tray}` | Semantic mapping |
| `run_task` | `task_run` | `[module]_[action]` format |
| `run_task_spawn_failed` | `diag_task_spawn_failed` | `diag_` prefix |
| `open_logs` | `logs_open` | `[module]_[action]` format |
| `open_logs_failed` | `diag_logs_open_failed` | `diag_` prefix |
| `updater_check` | `app_update_check` | `[module]_[action]` format |
| `updater_error` | `app_update_failed` | `[module]_[action]` format |
| `updater_uptodate` | `app_update_checked` | With `result:uptodate` |
| `updater_installed` | `app_update_installed` | `[module]_[action]` format |
| `state_changed` | `state_sync` | `[module]_[action]` format |
| `js_state_loaded` | `state_load` | `[module]_[action]` format |
| `js_state_updated` | `state_sync` | Aligns with Rust side |
| `state_load_failed` | `diag_state_load_failed` | `diag_` prefix |
| `panel_blur_hide` | `panel_close{via:blur}` | Merged into panel_close |

---

## 7. Verification Queries

### HogQL Examples

```sql
-- Events by name (last 24h)
SELECT event, count() AS cnt
FROM events
WHERE timestamp > now() - INTERVAL 1 DAY
GROUP BY event
ORDER BY cnt DESC

-- Identity split detection (P0 validation)
SELECT session_id, groupUniqArray(distinct_id) AS ids
FROM events
WHERE event IN ('rust_launch', 'js_launch')
  AND timestamp > now() - INTERVAL 1 DAY
GROUP BY session_id
HAVING length(ids) > 1

-- Heartbeat gap detection (±300s tolerance for sleep/batch delay)
-- Note: js_heartbeat seq resets across hidden windows (macOS App Nap);
-- gap detection applies only within visible windows, not across hide/show cycles.
SELECT session_id,
       maxIf(timestamp, event = 'rust_heartbeat') AS last_rust,
       maxIf(timestamp, event = 'js_heartbeat') AS last_js
FROM events
WHERE timestamp >= now() - INTERVAL 1 DAY
  AND event LIKE '%_heartbeat'
GROUP BY session_id
HAVING last_rust > last_js + INTERVAL 300 SECOND
```

### PostHog API

```
POST /api/projects/597439/query
{
  "query": {
    "kind": "HogQLQuery",
    "query": "SELECT event, count() FROM events WHERE timestamp > now() - interval 1 day GROUP BY event"
  }
}
```

> **Note**: Use `/query` endpoint (not `/events/` which has server errors).

---

## 8. Diagnostic Matrix

| Symptom | Conclusion |
|---------|------------|
| `rust_heartbeat` continues, `js_heartbeat` + `$autocapture` gaps | Webview hang (main thread blocked) |
| `panel_open`/`panel_close` continue, `$autocapture` gaps | Same as above (native interaction OK, page dead) |
| Both stop, process alive | Native layer hang (check system log / sample process) |
| All stop, process exited | Crash (correlate with `rust_launch` timeline) |

---

## 9. Implementation Roadmap

| Phase | Scope | Status |
|-------|-------|--------|
| **P0** | Identity unification (`desktop-{uuidv4}`), environment gate, `app_version` | Completed |
| **P1** | `rust_exit` (normal + dirty), flush on exit, per-event timestamp, heartbeat seq | Planned |
| **P2** | Watchdog diagnostics (`diag_*` events), error-gateway integration | Planned |
| **P3** | Registry codegen, type safety, drift audit | In progress (this PR) |

---

## 10. PostHog Project

- **Project**: MenCbo Desktop
- **ID**: 597439
- **Host**: us.posthog.com
- **Capture key**: Public by design (capture-only, no security boundary)

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

---

## 已知限制（Known Limitations）

### Exit-flush HTTP 竞态（结构性，M4 watchdog 期处理）

`RunEvent::Exit` 中 `flush_sync` 调用受 3s 超时约束。若网络延迟或 PostHog 端点响应缓慢，部分事件可能在进程终止前未完成发送。这是结构性限制：进程退出时序与 HTTP 请求的竞态无法在用户态完全消除。

**当前缓解**：两次 `flush_sync` 调用（先 flush pending，再 flush `rust_exit` 本身），总预算 ≤6s。
**长期方案**：M4 watchdog 诊断（`diag_exit_flush_failed`）将捕获 flush 失败并记录，便于后续优化（如本地持久化队列）。

### Degraded 安装的 session_id 哨兵语义

当 `install_id` IO 失败（`get_or_create_install_id` 返回错误），identity 被设置为哨兵值 `"analytics-disabled"`，`analytics_identity` bridge 返回 `degraded: true`。JS 侧跳过 bootstrap，所有事件携带 `identity_degraded: true`。

**语义**：同一 degraded 安装的所有会话共享 `distinct_id = "analytics-disabled"`，但这不会导致 person 合并问题，因为：
1. JS 侧不 bootstrap（避免 `posthog.init` 将 `"analytics-disabled"` 作为真实 distinct_id 持久化）
2. Rust 侧 `posthog_capture` 在检测到 `"analytics-disabled"` 时直接 no-op（不发送任何事件）
3. 仅 JS 侧事件（携带 `identity_degraded: true`）会到达 PostHog，用于诊断 IO 故障

**注意**：此机制假设 `app_data_dir` 在 degraded 模式下仍可写（用于 session marker）。若 `app_data_dir` 本身不可访问，dirty exit 检测也会失效（marker 无法写入/读取），但这是极端边缘场景（磁盘满/权限损坏），不在当前缓解范围内。

---

## 诊断测试钩子（Diagnostic Test Hooks）

为验证特定场景（如面板关闭、webview 假死等），系统支持通过环境变量 `MENCBO_DIAG_TEST` 触发诊断测试模式。这些钩子**仅用于验证**，不应在生产环境使用。

### 使用方法

在启动应用时设置环境变量：

```bash
MENCBO_DIAG_TEST=<hook_name> pnpm -F desktop-client tauri dev
```

### 可用钩子

| 值 | 用途 | 说明 |
|----|------|------|
| `close_panel` | 自动关闭面板 | 面板首次可见后 3 秒触发 `perform_close()`，产生真实的 CloseRequested 事件链（→ `panel_close{via:close}`）。单次触发即止，用于验证 VAL-PAN-003。 |
| `freeze_webview` | Webview 假死模拟 | JS 侧 setInterval 死循环 60s+，阻止 JS heartbeat 更新 → 触发 `diag_webview_unresponsive` |
| `block_main` | 主线程阻塞模拟 | 主线程 sleep 阻塞 >10s，阻止 run_on_main_thread probe 更新 → 触发 `diag_native_main_unresponsive` |
| `slow_ipc` | IPC 延迟模拟 | command 执行超时 → 触发 `diag_ipc_timeout` |
| `panic` | Panic 触发 | 故意 panic → 触发 `diag_rust_panic`（同步直发 PostHog） |

### 示例

```bash
# 验证面板关闭事件（VAL-PAN-003）
MENCBO_DIAG_TEST=close_panel pnpm -F desktop-client tauri dev
```

**注意**：诊断钩子仅在 release 构建或显式启用时生效，debug 构建默认忽略这些环境变量。

