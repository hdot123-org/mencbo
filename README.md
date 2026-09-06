# memengine

[![CI](https://github.com/hdot123-org/memengine/actions/workflows/ci.yml/badge.svg)](https://github.com/hdot123-org/memengine/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**面向 AI 应用的客户端分层记忆引擎。** TypeScript 编写，零依赖，同构 —— 浏览器、Node、Edge 运行时均可运行。

简体中文 | [English](README.en.md)

## 为什么需要它

AI 应用总是在遗忘。聊天历史不是记忆：它是线性的、非结构化的，并且随标签页关闭而蒸发。memengine 给你的应用一个轻量、有纪律、分层的记忆系统，完全运行在客户端 —— 无服务器、无数据库、无厂商锁定。

它把 [memory-core](#与-memory-core-的关系)（服务端 Agent 记忆系统）中经过验证的概念移植到客户端：分层路由、read-first 变更、所有权守卫、schema 迁移。

## 安装

```bash
npm install memengine
```

要求 Node >= 18 或任意现代浏览器。零运行时依赖。

## 快速上手

```ts
import { KVStore, MemoryEngine } from 'memengine'

const engine = new MemoryEngine({
  project: new KVStore(localStorage, 'project'), // 本应用的记忆
  global: new KVStore(localStorage, 'global'),   // 跨应用兜底（可选）
})

// 记录知识
const entry = await engine.write({
  title: '用户讨厌长表单',
  content: '注册流程砍到 3 步后，转化率 +18%。',
  domain: 'collaboration',
  tags: ['onboarding', 'insight'],
})

// 之后检索 —— 项目层优先，全局层兜底
const hits = await engine.search({ text: 'onboarding' })
const found = await engine.read(entry.id)
```

条目以纯 frontmatter 文档存储，人类可读、可迁移：

```md
---
id: 0f1c2a34-...
title: 用户讨厌长表单
domain: collaboration
tags: [onboarding, insight]
layer: project
schemaVersion: 1
createdAt: 2026-09-06T02:59:00.000Z
updatedAt: 2026-09-06T02:59:00.000Z
---

注册流程砍到 3 步后，转化率 +18%。
```

## 分层与路由

```
┌─────────────────────────────────────────────┐
│                MemoryEngine                  │
│                                             │
│   读 / 搜索:     project → global           │
│   写:            project | pending          │
└───────────────┬─────────────┬───────────────┘
                │             │
        ┌───────▼──────┐ ┌────▼─────────┐
        │   project    │ │    global    │
        │  (本应用)     │ │  (跨应用)     │
        │  + pending   │ │    兜底层     │
        └──────────────┘ └──────────────┘
```

- **project** —— 本应用 / 工作区 / 用户的记忆，必备层。
- **global** —— 跨上下文的共享知识，可选；仅当项目层未命中时才查询。
- **pending** —— 自动捕获内容的暂存区，默认对读取隐藏，通过 `engine.promote(id)` 晋升。

同 id 的项目层条目会遮蔽全局层条目。

## Read-first 变更

盲写是记忆损坏的经典来源。memengine 把 memory-core 的 read-first CRUD 规则转化为乐观并发控制：

```ts
const entry = (await engine.read(someId))!

// ✅ 携带读取时拿到的 updatedAt 令牌
await engine.update(someId, { content: '复测后 +19%' }, { ifMatch: entry.updatedAt })

// ❌ 被拒绝：必须先读取条目
await engine.update(someId, { content: '盲写' }) // 抛出 ConflictError

// 脚本 / 迁移场景的逃生门
await engine.update(someId, { content: '强制写入' }, { force: true })
```

如果你读取之后另一个标签页改了条目，`ifMatch` 不再匹配，你会得到 `ConflictError`，而不是静默覆盖。删除遵循同样的规则。更新令牌严格单调递增——即使同一毫秒内的两次更新也会产生不同的令牌，过期令牌必定被检测出来。

## 所有权守卫

所有变更在触碰存储之前都要经过故障关闭（fail-closed）守卫：

```ts
import { MemoryEngine, MemoryStore, OwnershipGuard } from 'memengine'

const engine = new MemoryEngine({
  project: new MemoryStore(),
  guard: new OwnershipGuard({
    protectedPatterns: [/^system:/, /^audit:/], // 永不允许变更的 id
    protectedLayers: ['global'],                // 将全局层设为只读
    allowByDefault: true,
  }),
})
```

守卫自身失效（非法输入、matcher 抛异常）时操作一律拒绝，受保护的状态永远不会暴露在风险中。

## Schema 迁移

条目携带 `schemaVersion`。读取时自动通过已注册的迁移链升级；**更新引擎版本**写入的条目会被拒绝，绝不静默降级：

```ts
import { MemoryEngine, MemoryStore, MigrationChain } from 'memengine'

const chain = new MigrationChain([
  {
    from: 1,
    to: 2,
    migrate: (e) => ({ ...e, domain: e.domain === 'general' ? 'engineering' : e.domain }),
  },
])

const engine = new MemoryEngine({ project: new MemoryStore(), chain })
engine.schemaVersion // 2 —— 新写入盖章 v2
```

## 存储适配器

各层构建在 `MemoryStoreAdapter` 之上 —— 四个异步方法。内置 `MemoryStore`（内存）和 `KVStore`（localStorage 类）。可以自带：

```ts
import type { MemoryStoreAdapter } from 'memengine'

const redisAdapter: MemoryStoreAdapter = {
  name: 'redis',
  async get(key) { return (await redis.get(key)) ?? undefined },
  async set(key, value) { await redis.set(key, value) },
  async delete(key) { await redis.del(key) },
  async keys() { return redis.keys('mem:*') },
}
```

异步适配器意味着 IndexedDB、SQLite（WASM）、OPFS 或远程 KV 都无需改动引擎代码即可接入。

## API 概览

| 成员 | 用途 |
| --- | --- |
| `new MemoryEngine({ project, global?, guard?, chain? })` | 构建引擎 |
| `engine.write(input, { layer? })` | 创建条目（project 或 pending） |
| `engine.read(id, { includePending? })` | 按 id 读取，project → global 路由 |
| `engine.update(id, changes, { ifMatch? \| force })` | read-first 更新 |
| `engine.delete(id, { ifMatch? \| force })` | read-first 删除 |
| `engine.search({ text?, domains?, tags?, layers?, limit? })` | 加权搜索（标题 ×3、标签 ×2、内容 ×1） |
| `engine.list({ layer?, domain? })` | 列出条目，按时间倒序 |
| `engine.promote(id)` | 将 pending 条目晋升到 project 层 |
| `engine.schemaVersion` | 引擎当前写入的 schema 版本 |
| `OwnershipGuard` | 故障关闭的变更守卫 |
| `MigrationChain` | 连续版本迁移链，读取时应用 |
| `LayerRouter` | 分层路由策略 |
| `MemoryStore` / `KVStore` | 内置适配器 |
| `ConflictError`、`GuardDeniedError`、`EntryNotFoundError`、`ValidationError`、`EntryParseError` | 错误类型 |

## 设计原则

- **项目优先，全局兜底。** 本地上下文遮蔽共享知识，全局层按需启用。
- **先读后写。** 变更必须携带先前读取的并发令牌，冲突显式暴露而非静默覆盖。
- **故障关闭。** 守卫或解析失败即拒绝操作；损坏文档在扫描中跳过、在直接读取时报错。
- **只向前迁移，绝不向后。** 旧条目读取时升级；来自更新引擎版本的条目被拒绝。
- **零依赖，同构。** 一个可以在任何 JavaScript 环境运行的小核心。
- **人类可读的存储。** 可读、可 diff、可手改的 frontmatter 文档。

## 与 memory-core 的关系

memengine 是 [memory-core](https://github.com/hdot123-org) 的客户端后裔。memory-core 是一个服务端、hook 驱动的 Agent 记忆系统，采用三层架构（运行时状态 / 全局知识库 / 项目知识库）。memengine 保留了它的概念内核 —— 分层路由、所有权守卫、read-first CRUD、schema 版本化 —— 并将其重构为零依赖的客户端 TypeScript 库。

## 开发

```bash
npm install
npm test          # vitest
npm run typecheck # tsc --noEmit
npm run build     # tsc → dist/
```

## 贡献

欢迎 Issue 和 PR，见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 许可证

[MIT](LICENSE) © hdot123-org and contributors
