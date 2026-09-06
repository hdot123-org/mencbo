# MenCbo

[![CI](https://github.com/hdot123-org/mencbo/actions/workflows/ci.yml/badge.svg)](https://github.com/hdot123-org/mencbo/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Client-side layered memory engine for AI apps.** TypeScript, zero dependencies, isomorphic — runs in the browser, Node, and edge runtimes.

[简体中文](README.md) | English

## Why

AI apps keep forgetting. Chat history is not memory: it is linear, unstructured, and evaporates with the tab. MenCbo gives your app a small, disciplined, layered memory that lives entirely on the client — no server, no database, no vendor lock-in.

It ports the proven concepts of [memory-core](#relationship-to-memory-core) (a server-side agent memory system) to the client: layered routing, read-first mutations, an ownership guard, and schema migrations.

## Install

```bash
npm install mencbo
```

Requires Node >= 18 or any modern browser. Zero runtime dependencies.

## Quick start

```ts
import { KVStore, MemoryEngine } from 'mencbo'

const engine = new MemoryEngine({
  project: new KVStore(localStorage, 'project'), // this app's memory
  global: new KVStore(localStorage, 'global'),   // cross-app fallback (optional)
})

// Capture knowledge
const entry = await engine.write({
  title: 'Users hate long forms',
  content: 'Cut onboarding to 3 steps; conversion +18%.',
  domain: 'collaboration',
  tags: ['onboarding', 'insight'],
})

// Retrieve it later — project layer first, global fallback
const hits = await engine.search({ text: 'onboarding' })
const found = await engine.read(entry.id)
```

Entries are stored as plain frontmatter documents, human-readable and portable:

```md
---
id: 0f1c2a34-...
title: Users hate long forms
domain: collaboration
tags: [onboarding, insight]
layer: project
schemaVersion: 1
createdAt: 2026-09-06T02:59:00.000Z
updatedAt: 2026-09-06T02:59:00.000Z
---

Cut onboarding to 3 steps; conversion +18%.
```

## Layers & routing

```
┌─────────────────────────────────────────────┐
│                MemoryEngine                  │
│                                             │
│   reads / searches:  project → global       │
│   writes:            project | pending      │
└───────────────┬─────────────┬───────────────┘
                │             │
        ┌───────▼──────┐ ┌────▼─────────┐
        │   project    │ │    global    │
        │  (this app)  │ │ (cross-app)  │
        │  + pending   │ │   fallback   │
        └──────────────┘ └──────────────┘
```

- **project** — this app's / workspace's / user's memory. Always present.
- **global** — shared knowledge across contexts. Optional; consulted only when the project layer has nothing.
- **pending** — staging area for auto-captured notes. Hidden from reads until promoted with `engine.promote(id)`.

A project entry with the same id shadows its global counterpart.

## Read-first mutations

Blind overwrites are the classic memory-corruption bug. MenCbo borrows memory-core's read-first CRUD rule and turns it into optimistic concurrency:

```ts
const entry = (await engine.read(someId))!

// ✅ carry the updatedAt token from your read
await engine.update(someId, { content: '…+19% after rerun' }, { ifMatch: entry.updatedAt })

// ❌ refused: you must have read the entry first
await engine.update(someId, { content: 'blind write' }) // throws ConflictError

// escape hatch for scripts and migrations
await engine.update(someId, { content: 'forced' }, { force: true })
```

If another tab changed the entry since your read, `ifMatch` no longer matches and you get a `ConflictError` instead of silently clobbering the update. Deletes follow the same rule. Update tokens are strictly monotonic — even two updates within the same millisecond produce different tokens, so a stale token is always detected.

## Ownership guard

Every mutation passes through a fail-closed guard before touching storage:

```ts
import { MemoryEngine, MemoryStore, OwnershipGuard } from 'mencbo'

const engine = new MemoryEngine({
  project: new MemoryStore(),
  guard: new OwnershipGuard({
    protectedPatterns: [/^system:/, /^audit:/], // ids that may never be mutated
    protectedLayers: ['global'],                // make the global layer read-only
    allowByDefault: true,
  }),
})
```

If the guard itself fails (bad input, throwing matcher), the operation is denied — protected state is never left exposed.

## Schema migrations

Entries carry a `schemaVersion`. On read, entries are upgraded automatically through a registered migration chain; entries written by a **newer** engine version are refused, never silently downgraded:

```ts
import { MemoryEngine, MemoryStore, MigrationChain } from 'mencbo'

const chain = new MigrationChain([
  {
    from: 1,
    to: 2,
    migrate: (e) => ({ ...e, domain: e.domain === 'general' ? 'engineering' : e.domain }),
  },
])

const engine = new MemoryEngine({ project: new MemoryStore(), chain })
engine.schemaVersion // 2 — new writes are stamped v2
```

## Storage adapters

Layers sit on `MemoryStoreAdapter` — four async methods. Ship with `MemoryStore` (in-memory) and `KVStore` (localStorage-like). Bring your own:

```ts
import type { MemoryStoreAdapter } from 'mencbo'

const redisAdapter: MemoryStoreAdapter = {
  name: 'redis',
  async get(key) { return (await redis.get(key)) ?? undefined },
  async set(key, value) { await redis.set(key, value) },
  async delete(key) { await redis.del(key) },
  async keys() { return redis.keys('mem:*') },
}
```

Async adapters mean IndexedDB, SQLite (via WASM), OPFS, or a remote KV all work without changing engine code.

## API overview

| Member | Purpose |
| --- | --- |
| `new MemoryEngine({ project, global?, guard?, chain? })` | Build an engine |
| `engine.write(input, { layer? })` | Create an entry (project or pending) |
| `engine.read(id, { includePending? })` | Read by id, project → global routing |
| `engine.update(id, changes, { ifMatch? \| force })` | Read-first update |
| `engine.delete(id, { ifMatch? \| force })` | Read-first delete |
| `engine.search({ text?, domains?, tags?, layers?, limit? })` | Scored search (title ×3, tags ×2, content ×1) |
| `engine.list({ layer?, domain? })` | List entries, newest first |
| `engine.promote(id)` | Move a pending entry into the project layer |
| `engine.schemaVersion` | Schema version this engine writes |
| `OwnershipGuard` | Fail-closed mutation guard |
| `MigrationChain` | Contiguous schema migrations, applied on read |
| `LayerRouter` | Layer routing policy |
| `MemoryStore` / `KVStore` | Built-in adapters |
| `ConflictError`, `GuardDeniedError`, `EntryNotFoundError`, `ValidationError`, `EntryParseError` | Error types |

## Design principles

- **Project first, global fallback.** Local context shadows shared knowledge; the global layer is opt-in.
- **Read before write.** Mutations require the concurrency token from a prior read; conflicts surface instead of clobbering.
- **Fail closed.** Guard or parsing failures deny the operation; corrupt documents are skipped in scans and surfaced on direct reads.
- **Version forward, never backward.** Old entries migrate on read; entries from newer engine versions are rejected.
- **Zero dependencies, isomorphic.** One small core that runs anywhere JavaScript does.
- **Human-readable storage.** Frontmatter documents you can read, diff, and hand-edit.

## Relationship to memory-core

MenCbo is a client-side descendant of [memory-core](https://github.com/hdot123-org), a server-side, hook-driven agent memory system with a three-layer architecture (runtime state / global knowledge base / project knowledge base). MenCbo keeps its conceptual core — layered routing, ownership guarding, read-first CRUD, schema versioning — and rebuilds it as a dependency-free TypeScript library for client applications.

## Development

```bash
npm install
npm test          # vitest
npm run typecheck # tsc --noEmit
npm run build     # tsc → dist/
```

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE) © hdot123-org and contributors
