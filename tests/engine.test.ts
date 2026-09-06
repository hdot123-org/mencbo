import { describe, expect, it } from 'vitest'
import {
  ConflictError,
  createEntry,
  EntryNotFoundError,
  GuardDeniedError,
  MemoryEngine,
  MemoryStore,
  MigrationChain,
  serializeEntry,
  ValidationError,
} from '../src/index.js'

function makeEngine() {
  const project = new MemoryStore('project')
  const global = new MemoryStore('global')
  const engine = new MemoryEngine({ project, global })
  return { engine, project, global }
}

describe('MemoryEngine', () => {
  it('writes and reads back entries', async () => {
    const { engine } = makeEngine()
    const entry = await engine.write({
      title: 'SSH pitfalls',
      content: 'accept-routes breaks LAN routing',
      domain: 'operations',
      tags: ['ssh', 'tailscale'],
    })
    const read = await engine.read(entry.id)
    expect(read?.title).toBe('SSH pitfalls')
    expect(read?.layer).toBe('project')
    expect(read?.tags).toEqual(['ssh', 'tailscale'])
  })

  it('rejects invalid writes', async () => {
    const { engine } = makeEngine()
    await expect(engine.write({ title: '', content: 'x' })).rejects.toBeInstanceOf(ValidationError)
  })

  it('guards protected ids', async () => {
    const { engine } = makeEngine()
    await expect(engine.write({ id: 'system:config', title: 'T', content: 'C' })).rejects.toBeInstanceOf(
      GuardDeniedError,
    )
  })

  it('enforces read-first updates', async () => {
    const { engine } = makeEngine()
    const entry = await engine.write({ title: 'T', content: 'v1' })

    await expect(engine.update(entry.id, { content: 'v2' })).rejects.toBeInstanceOf(ConflictError)

    const updated = await engine.update(entry.id, { content: 'v2' }, { ifMatch: entry.updatedAt })
    expect(updated.content).toBe('v2')

    // Stale token from the first read no longer matches.
    await expect(engine.update(entry.id, { content: 'v3' }, { ifMatch: entry.updatedAt })).rejects.toBeInstanceOf(
      ConflictError,
    )

    const forced = await engine.update(entry.id, { content: 'v3' }, { force: true })
    expect(forced.content).toBe('v3')
  })

  it('enforces read-first deletes', async () => {
    const { engine } = makeEngine()
    const entry = await engine.write({ title: 'T', content: 'C' })

    await expect(engine.delete(entry.id)).rejects.toBeInstanceOf(ConflictError)

    await engine.delete(entry.id, { ifMatch: entry.updatedAt })
    expect(await engine.read(entry.id)).toBeUndefined()

    await expect(engine.delete(entry.id, { force: true })).rejects.toBeInstanceOf(EntryNotFoundError)
  })

  it('routes project entries before global fallback', async () => {
    const { engine, global } = makeEngine()

    const globalEntry = createEntry({ title: 'Global wisdom', content: 'shared knowledge' }, 'global')
    await global.set(`mem:g:${globalEntry.id}`, serializeEntry(globalEntry))

    const read = await engine.read(globalEntry.id)
    expect(read?.layer).toBe('global')
    expect(read?.title).toBe('Global wisdom')

    // A project entry with the same id shadows the global one.
    await engine.write({ id: globalEntry.id, title: 'Local override', content: 'local' })
    const shadowed = await engine.read(globalEntry.id)
    expect(shadowed?.title).toBe('Local override')
    expect(shadowed?.layer).toBe('project')
  })

  it('searches with scoring and filters', async () => {
    const { engine } = makeEngine()
    await engine.write({ title: 'Vitest strategy', content: 'use vitest everywhere', domain: 'engineering', tags: ['testing'] })
    await engine.write({ title: 'Deploy notes', content: 'also mentions vitest once', domain: 'operations', tags: ['deploy'] })
    await engine.write({ title: 'Unrelated', content: 'nothing to see', domain: 'engineering' })

    const hits = await engine.search({ text: 'vitest' })
    expect(hits.map((hit) => hit.entry.title)).toEqual(['Vitest strategy', 'Deploy notes'])

    const filtered = await engine.search({ domains: ['operations'] })
    expect(filtered.map((hit) => hit.entry.title)).toEqual(['Deploy notes'])
  })

  it('stages pending entries and promotes them', async () => {
    const { engine } = makeEngine()
    const pending = await engine.write({ title: 'Captured', content: 'raw note' }, { layer: 'pending' })

    // Pending entries are hidden from default reads.
    expect(await engine.read(pending.id)).toBeUndefined()
    expect(await engine.read(pending.id, { includePending: true })).toBeDefined()

    const promoted = await engine.promote(pending.id)
    expect(promoted.layer).toBe('project')

    const read = await engine.read(pending.id)
    expect(read?.title).toBe('Captured')
    expect(read?.layer).toBe('project')
  })

  it('upgrades legacy-schema entries on read', async () => {
    const chain = new MigrationChain([
      { from: 1, to: 2, migrate: (entry) => ({ ...entry, title: `[migrated] ${entry.title}` }) },
    ])
    const project = new MemoryStore()
    const engine = new MemoryEngine({ project, chain })
    expect(engine.schemaVersion).toBe(2)

    const legacy = createEntry({ title: 'Old', content: 'v1 doc' }, 'project', 1)
    await project.set(`mem:p:${legacy.id}`, serializeEntry(legacy))

    const read = await engine.read(legacy.id)
    expect(read?.title).toBe('[migrated] Old')
    expect(read?.schemaVersion).toBe(2)
  })

  it('lists entries with optional domain filter', async () => {
    const { engine } = makeEngine()
    await engine.write({ title: 'A', content: 'x' })
    await engine.write({ title: 'B', content: 'y', domain: 'operations' })

    const all = await engine.list()
    expect(all.map((entry) => entry.title).sort()).toEqual(['A', 'B'])

    const ops = await engine.list({ domain: 'operations' })
    expect(ops.map((entry) => entry.title)).toEqual(['B'])
  })
})
