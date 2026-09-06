# MenCbo Desktop

MenCbo 桌面控制台与调度宿主 —— macOS 菜单栏常驻托盘控制台 + 后台单进程调度守护器。本目录是 [monorepo](../../README.md) 中的桌面应用交付单元：客户端与守护器同一版本号、同一发布流水线（DMG）。

## 结构

```
daemon/    Phase 1-2 · Python 单进程统一调度守护器（uv 管理）
client/    Phase 3 · Tauri v2 + TypeScript 控制台客户端（未启动）
```

守护器通过 pnpm workspace 与 `packages/mencbo`（TS 记忆引擎库）同仓联动：客户端未来以 `workspace:*` 依赖该库实现自己的记忆层，改库即刻热更新，无需发包联调。

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
- [ ] **Phase 2** 单进程 asyncio 统一调度器、`~/.mencbo/state.json`、CLI status
- [ ] **Phase 3** Tauri v2 客户端外壳
- [ ] **Phase 4** sidecar 冻结打包与 `MenCbo.dmg` 交付

守护器开发命令见 [daemon/README.md](daemon/README.md)。
