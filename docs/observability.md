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
| `rust_exit` | rust | Process exit | `via: command\|tray_menu` |
| `rust_heartbeat` | rust | Every 5 min | — |
| `js_launch` | webview | Webview init | — |
| `js_heartbeat` | webview | Every 5 min | `uptime_sec` |

### Panel Events

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `panel_open` | both | Panel shown | `via: tray` |
| `panel_close` | both | Panel hidden | `via: tray\|blur` |

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

### Planned Events (v1.1 Standard)

> Not yet implemented. Tracked for future milestones.

| Event | Layer | Trigger | Properties |
|-------|-------|---------|------------|
| `rust_exit` (enhanced) | rust | Normal/dirty exit | `reason: normal\|dirty`, `uptime_s`, `prev_session_id` |
| `diag_webview_unresponsive` | rust | Watchdog: webview hang | `missed_js_beats`, `threshold_s` |
| `diag_webview_recovered` | rust | Watchdog: recovery | `freeze_duration_ms` |
| `diag_webview_terminated` | rust | Watchdog: webview crash | — |
| `diag_native_main_unresponsive` | rust | Main thread probe timeout | — |
| `diag_ipc_timeout` | webview | Tauri invoke timeout | `error_code`, `timeout_ms` |
| `diag_rust_panic` | rust | Panic hook | `message`, `backtrace` |
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
