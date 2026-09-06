import { describe, expect, it } from 'vitest'
import { createEntry, EntryParseError, parseEntry, serializeEntry } from '../src/index.js'

describe('entry', () => {
  it('creates an entry with defaults', () => {
    const entry = createEntry({ title: 'T', content: 'C' }, 'project')
    expect(entry.id).toBeTruthy()
    expect(entry.domain).toBe('general')
    expect(entry.tags).toEqual([])
    expect(entry.layer).toBe('project')
    expect(entry.schemaVersion).toBe(1)
  })

  it('round-trips through serialization', () => {
    const entry = createEntry(
      {
        title: 'Proxy pitfalls',
        content: 'body with --- inside\nsecond line',
        domain: 'operations',
        tags: ['proxy', 'net'],
      },
      'project',
      1,
    )
    const doc = serializeEntry(entry)
    expect(doc.startsWith('---\n')).toBe(true)
    expect(parseEntry(doc)).toEqual(entry)
  })

  it('parses hand-written frontmatter with a blank line after the delimiter', () => {
    const doc = [
      '---',
      'id: hand-1',
      'title: Hand written',
      'domain: engineering',
      'tags: [ci, github]',
      'layer: global',
      'schemaVersion: 1',
      'createdAt: 2026-01-01T00:00:00.000Z',
      'updatedAt: 2026-01-02T00:00:00.000Z',
      '---',
      '',
      'Some notes.',
    ].join('\n')
    const parsed = parseEntry(doc)
    expect(parsed.id).toBe('hand-1')
    expect(parsed.title).toBe('Hand written')
    expect(parsed.tags).toEqual(['ci', 'github'])
    expect(parsed.layer).toBe('global')
    expect(parsed.content).toBe('Some notes.')
  })

  it('rejects malformed documents', () => {
    expect(() => parseEntry('no frontmatter at all')).toThrow(EntryParseError)
    expect(() => parseEntry('---\nid: x\n---\nbody')).toThrow(EntryParseError) // missing title
    expect(() => parseEntry('---\ntitle: x\n---\nbody')).toThrow(EntryParseError) // missing id
    expect(() => parseEntry('---\nid: x\ntitle: t\nlayer: yolo\n---\nbody')).toThrow(EntryParseError)
  })
})
