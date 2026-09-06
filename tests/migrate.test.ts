import { describe, expect, it } from 'vitest'
import { createEntry, MigrationChain } from '../src/index.js'
import type { MemoryEntry, Migration } from '../src/index.js'

const toV2: Migration = {
  from: 1,
  to: 2,
  migrate: (entry) => ({ ...entry, domain: entry.domain === 'general' ? 'engineering' : entry.domain }),
}

describe('MigrationChain', () => {
  it('starts at version 1', () => {
    expect(new MigrationChain().latestVersion()).toBe(1)
  })

  it('applies contiguous steps in order', () => {
    const chain = new MigrationChain([
      toV2,
      { from: 2, to: 3, migrate: (entry) => ({ ...entry, tags: [...entry.tags, 'v3'] }) },
    ])
    const legacy = createEntry({ title: 'T', content: 'C' }, 'project', 1)
    const upgraded = chain.apply(legacy)

    expect(upgraded.schemaVersion).toBe(3)
    expect(upgraded.domain).toBe('engineering')
    expect(upgraded.tags).toContain('v3')
  })

  it('is a no-op for current entries', () => {
    const chain = new MigrationChain([toV2])
    const current = createEntry({ title: 'T', content: 'C' }, 'project', 2)
    expect(chain.apply(current).schemaVersion).toBe(2)
  })

  it('refuses entries from a newer engine', () => {
    const chain = new MigrationChain()
    const future: MemoryEntry = { ...createEntry({ title: 'T', content: 'C' }, 'project', 1), schemaVersion: 5 }
    expect(() => chain.apply(future)).toThrow(/newer/)
  })

  it('rejects non-contiguous or duplicate steps at registration', () => {
    expect(() => new MigrationChain([{ from: 1, to: 3, migrate: (entry) => entry }])).toThrow(/one version/)
    const chain = new MigrationChain([toV2])
    expect(() => chain.register({ from: 1, to: 2, migrate: (entry) => entry })).toThrow(/duplicate/)
  })
})
