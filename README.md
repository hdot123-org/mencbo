# MenCbo App

MenCbo 桌面控制台与调度宿主 —— macOS 菜单栏常驻托盘控制台 + 后台单进程调度守护器。

## 仓库定位

本仓库是 MenCbo 系统的应用交付仓库（双仓库结构）：

| 仓库 | 职责 |
| --- | --- |
| [`hdot123-org/mencbo`](https://github.com/hdot123-org/mencbo) | TS 记忆引擎库（npm: `mencbo`），面向 AI 应用的通用客户端记忆 |
| `hdot123-org/mencbo-app`（本仓库） | 桌面应用：Python 守护器 + Tauri 客户端 + DMG 交付 |

## 结构

```
daemon/    Phase 1-2 · Python 单进程统一调度守护器（uv 管理）
client/    Phase 3 · Tauri v2 + TypeScript 控制台客户端（未启动）
```

## 路线图

- [x] **Phase 1** 基础契约：`TaskSpec`（契约 2）、Git 作用域解析（契约 1）、示例任务清单
- [ ] **Phase 2** 单进程 asyncio 统一调度器、`~/.mencbo/state.json`（契约 3）、CLI status
- [ ] **Phase 3** Tauri v2 菜单栏客户端外壳（非侵入式读取 state.json 渲染）
- [ ] **Phase 4** Python 冻结为 sidecar 二进制，打包输出 `MenCbo.dmg`

守护器的开发命令（`uv sync` / `uv run pytest` 等）见 [daemon/README.md](daemon/README.md)。
