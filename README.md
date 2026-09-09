# MenCbo Monorepo

<p align="center"><img src="docs/logo.png" alt="MenCbo" width="320"></p>

[![CI](https://github.com/hdot123-org/mencbo/actions/workflows/ci.yml/badge.svg)](https://github.com/hdot123-org/mencbo/actions/workflows/ci.yml)

pnpm workspace 单仓多包：面向 AI 应用的 TS 记忆引擎库 **`mencbo`**（npm 包）与 macOS 桌面控制台 **MenCbo.app** 共用一个仓库、各自独立发布。

## 结构

```
packages/
  mencbo/            TS 记忆引擎库（npm: mencbo）— 零依赖、同构、分层记忆
apps/
  desktop/
    daemon/          Phase 1-2 · Python 单进程统一调度守护器（uv 管理）
    client/          Phase 3 · Tauri v2 控制台客户端（Rust + React + TypeScript）
    analytics/       事件注册表 events.yml 与代码生成/守门脚本
docs/
  observability.md   桌面端可观测性标准（事件与诊断的权威参考）
```

## 快速开始

库（Node >= 18 或任意现代浏览器）：

```bash
pnpm install
pnpm -F mencbo run test        # vitest
pnpm -F mencbo run typecheck   # tsc --noEmit
pnpm -F mencbo run build       # tsc -> packages/mencbo/dist/
```

守护器（Python >= 3.12）：

```bash
cd apps/desktop/daemon
uv sync
uv run pytest
```

桌面客户端（Node 22 + Rust stable）：

```bash
pnpm -F desktop-client test                    # vitest
pnpm -F desktop-client exec tsc --noEmit
# cargo 依赖 ../dist 存在（tauri::generate_context!），必须先构建前端
pnpm -F desktop-client build
cd apps/desktop/client/src-tauri
cargo check
cargo test --lib
```

## 埋点事件注册表

所有 PostHog 事件以 [`apps/desktop/analytics/events.yml`](apps/desktop/analytics/events.yml) 为唯一事实源（`active` / `deprecated` 状态，废弃事件带 `replaced_by` 迁移链）。新增事件流程：

1. 先在 `events.yml` 登记（含属性契约）；
2. 运行 `pnpm analytics:codegen`，生成 `analytics-events.gen.ts` 与 `analytics_events_gen.rs` —— 生成确定性且幂等，产物必须随源一起提交，CI 以 `git diff --exit-code` 校验。

配套守门脚本（CI 强制执行）：

- `check-imports.mjs`：`posthog-js` 仅允许在封装层 `src/lib/analytics.ts` import，其余文件一律封堵；
- `verify-versions.mjs`：`packages/mencbo`、`apps/desktop/client` 的 package.json、`tauri.conf.json`、`Cargo.toml` 四处 SemVer 必须一致。

## CI

- `desktop.yml`：路径过滤（detect）后运行两个作业 —— **Client**（版本一致性、import 封堵、codegen 幂等校验、tsc、vitest）与 **Tauri**（前端构建 → `cargo check` / `cargo test --lib`）；
- `taxonomy-scan.yml`：每日定时扫描（可手动触发，支持 dry-run），用 HogQL 拉取 PostHog 近 7 天在线事件与注册表比对：出现未注册事件或已注册事件消失时创建 GitHub issue（`taxonomy-drift` + `needs-triage`，事件恢复后自动关闭/重开），并输出 active 但 30 天零流量的死埋点报告；无凭证时自动降级为 dry-run；
- PR 检查全绿后由 auto-merge workflow 自动合并。

## 可观测性（桌面端）

完整规范见 [docs/observability.md](docs/observability.md)。要点：

- **统一身份**：`install_id`（`desktop-{uuidv4}`，原子写持久化）+ `launch_id`（每次启动生成），Rust 与 webview 经 Tauri command 桥接共享；
- **批量上报**：Rust 阻塞线程批量队列（20 条或 60s），托盘 Quit 退出路径同步 flush（3s 超时）补发 `rust_exit`；
- **面板生命周期**：`panel_open` / `panel_close`，带 `via=blur|tray|close|command` 归因；单实例守卫在二次启动时聚焦既有窗口并补发 `panel_open`；
- **双层心跳**：`rust_heartbeat` 与 `js_heartbeat`（5 分钟）对照可区分卡死发生在原生层还是 webview 层；
- **看门狗诊断**：`diag_webview_unresponsive` / `recovered`、`diag_native_main_unresponsive`（含 `stalled_ms`）、`diag_ipc_timeout`（`E_IPC_TIMEOUT`）、`diag_rust_panic`（带 backtrace，有界重试）；
- **上报门槛**：`POSTHOG_KEY`（Rust 侧 build.rs 注入）/ `VITE_POSTHOG_KEY`（JS 侧）—— 未设置 key 时全部捕获为 no-op；debug 构建零上报，仅 keyed release 构建上报；
- **故障注入**：`MENCBO_DIAG_TEST` 支持 `freeze_webview | block_main | slow_ipc | panic | close_panel`，用于端到端验证诊断链路。

## 桌面端路线图

- [x] **Phase 1** 基础契约：`TaskSpec`（契约 2）、Git 作用域解析（契约 1）、示例任务清单
- [x] **Phase 2** 单进程 asyncio 统一调度器、`~/.mencbo/state.json`（契约 3）、CLI status
- [x] **Phase 3** Tauri v2 托盘客户端、控制面板与 daemon 实时联动（#18）
- [ ] **Phase 4** Python 冻结为 sidecar 二进制，打包输出 `MenCbo.dmg`

## 文档

- 库文档（中/英）：[packages/mencbo/README.md](packages/mencbo/README.md) · [README.en.md](packages/mencbo/README.en.md)
- 桌面应用：[apps/desktop/README.md](apps/desktop/README.md)
- 可观测性标准：[docs/observability.md](docs/observability.md)
- 贡献指南：[CONTRIBUTING.md](CONTRIBUTING.md)

## 许可证

[MIT](LICENSE) © hdot123-org and contributors
