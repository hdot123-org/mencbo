# MenCbo Desktop

MenCbo 桌面控制台与调度宿主 —— macOS 菜单栏常驻托盘控制台 + 后台单进程调度守护器。本目录是 [monorepo](../../README.md) 中的桌面应用交付单元：客户端与守护器同一版本号、同一发布流水线（DMG）。

## 结构

```
daemon/    Phase 1-2 · Python 单进程统一调度守护器（uv 管理）
client/    Phase 3 · Tauri v2 + TypeScript 控制台客户端
```

守护器通过 pnpm workspace 与 `packages/mencbo`（TS 记忆引擎库）同仓联动：客户端以 `workspace:*` 依赖该库实现自己的记忆层，改库即刻热更新，无需发包联调。

## 架构

```
┌─────────────────────────────────────────────┐
│  MenCbo.app（Tauri 菜单栏托盘，纯观察与触发） │
│  监听 ~/.mencbo/state.json，不干预内部业务    │
└──────────────────┬──────────────────────────┘
                   │ state.json / 管道
┌──────────────────▼──────────────────────────┐
│  Python 守护器（单进程 asyncio 调度）         │
│  ├─ Git 作用域解析（锁定 owner/repo 命名空间）│
│  ├─ 统一调度中心（动态加载各项目 tasks.py）   │
│  └─ 状态单源落盘（state.json）               │
└──────────────────┬──────────────────────────┘
                   │ 动态注册
┌──────────────────▼──────────────────────────┐
│  已标准化项目能力层                          │
│  ├─ memory（做梦整理、快照沉淀、时序衰减）    │
│  └─ infra-core（心跳检测、工作流、自检）      │
└─────────────────────────────────────────────┘
```

## 路线图

- [x] **Phase 1** 基础契约：`TaskSpec`、Git 作用域解析、示例任务清单（30 测试）
- [x] **Phase 2** 单进程 asyncio 统一调度器、`~/.mencbo/state.json`、CLI status
- [x] **Phase 3** Tauri v2 客户端外壳 + daemon 联动
- [ ] **Phase 4** sidecar 冻结打包与 `MenCbo.dmg` 交付

守护器开发命令见 [daemon/README.md](daemon/README.md)。

## 运行手册

### 启动 daemon（后台调度器）

```bash
cd apps/desktop/daemon
/opt/homebrew/bin/uv run python -m mencbo run
```

daemon 会在后台运行，定时执行注册的任务并写入 `~/.mencbo/state.json`。

**查看状态**：
```bash
/opt/homebrew/bin/uv run python -m mencbo status
```

**立即执行单个任务**：
```bash
/opt/homebrew/bin/uv run python -m mencbo run-task example:heartbeat
```

### 启动客户端（Tauri 开发模式）

```bash
cd apps/desktop/client
pnpm tauri dev
```

这会启动 macOS 菜单栏托盘应用，实时读取 daemon 写入的 state.json 并显示任务状态。

### 浏览器预览模式（无需 Tauri）

```bash
cd apps/desktop/client
pnpm dev
```

然后打开 `http://localhost:1420`。浏览器模式使用内置 mock 数据，适合快速预览 UI。

**场景切换**：
- 默认三任务演示：`http://localhost:1420/`
- 仅成功/运行中：`http://localhost:1420/?scenario=ok`
- 空状态：`http://localhost:1420/?scenario=empty`

### 种子数据脚本（可选）

向 `~/.mencbo/state.json` 写入演示数据：

```bash
cd apps/desktop/client
pnpm exec tsx scripts/seed-state.ts
```

这会在 daemon 未运行时快速预览面板效果。
