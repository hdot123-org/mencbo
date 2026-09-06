import { describe, expect, it } from 'vitest'
import { createEntry, SCHEMA_VERSION, validateEntry } from '../src/index.js'
import type { MemoryEntry } from '../src/index.js'

function base(): MemoryEntry {
  return createEntry({ title: 'T', content: 'C' }, 'project', SCHEMA_VERSION)
}

function fields(entry: MemoryEntry): string[] {
  return validateEntry(entry).map((issue) => issue.field)
}

describe('schema', () => {
  it('accepts a valid entry', () => {
    expect(validateEntry(base())).toEqual([])
  })

  it('flags missing or multiline titles', () => {
    const emptyTitle = base()
    emptyTitle.title = ''
    expect(fields(emptyTitle)).toContain('title')

    const multiline = base()
    multiline.title = 'line one\nline two'
    expect(fields(multiline)).toContain('title')
  })

  it('flags future schema versions', () => {
    const future = base()
    future.schemaVersion = 99
    expect(fields(future)).toContain('schemaVersion')
  })

  it('flags invalid layers', () => {
    const invalid = base()
    invalid.layer = 'yolo' as MemoryEntry['layer']
    expect(fields(invalid)).toContain('layer')
  })

  it('flags invalid tags', () => {
    const invalid = base()
    invalid.tags = ['ok', 'has,comma', '']
    expect(fields(invalid)).toContain('tags')
  })

  it('flags missing timestamps', () => {
    const invalid = base()
    invalid.createdAt = ''
    expect(fields(invalid)).toContain('createdAt')
  })
})
