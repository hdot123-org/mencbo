import { SCHEMA_VERSION } from './schema.js'
import type { Layer, MemoryEntry, WriteInput } from './types.js'

/** Raised when a serialized document cannot be parsed. */
export class EntryParseError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'EntryParseError'
  }
}

const LAYERS: readonly string[] = ['global', 'project', 'pending']

type CryptoLike = { randomUUID?: () => string }

/** Generate an entry id. Uses crypto.randomUUID when available. */
export function newId(): string {
  const crypto = (globalThis as { crypto?: CryptoLike }).crypto
  if (crypto?.randomUUID) return crypto.randomUUID()
  return `mem_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 10)}`
}

/** Current time as ISO 8601. */
export function nowIso(): string {
  return new Date().toISOString()
}

/**
 * Strictly monotonic timestamp for optimistic-concurrency tokens.
 * If the clock has not advanced past `previous` (same-millisecond writes),
 * the new timestamp is bumped 1ms forward so stale tokens can never match.
 */
export function nextTimestamp(previous: string): string {
  const previousMs = Date.parse(previous)
  const now = new Date()
  if (Number.isNaN(previousMs) || now.getTime() > previousMs) {
    return now.toISOString()
  }
  return new Date(previousMs + 1).toISOString()
}

/** Build a fresh entry for the given layer, stamped with the engine's schema version. */
export function createEntry(input: WriteInput, layer: Layer, schemaVersion: number = SCHEMA_VERSION): MemoryEntry {
  const now = nowIso()
  return {
    id: input.id ?? newId(),
    title: input.title,
    content: input.content,
    domain: input.domain ?? 'general',
    tags: input.tags ?? [],
    layer,
    schemaVersion,
    createdAt: now,
    updatedAt: now,
  }
}

/** Serialize an entry as a frontmatter + body document. */
export function serializeEntry(entry: MemoryEntry): string {
  const frontmatter = [
    '---',
    `id: ${entry.id}`,
    `title: ${entry.title}`,
    `domain: ${entry.domain}`,
    `tags: [${entry.tags.join(', ')}]`,
    `layer: ${entry.layer}`,
    `schemaVersion: ${entry.schemaVersion}`,
    `createdAt: ${entry.createdAt}`,
    `updatedAt: ${entry.updatedAt}`,
    '---',
  ].join('\n')
  return `${frontmatter}\n${entry.content}`
}

function parseTags(raw: string | undefined): string[] {
  if (!raw) return []
  const stripped = raw.trim().replace(/^\[/, '').replace(/\]$/, '')
  return stripped
    .split(',')
    .map((tag) => tag.trim())
    .filter((tag) => tag.length > 0)
}

/** Parse a serialized entry. Throws {@link EntryParseError} on malformed documents. */
export function parseEntry(doc: string): MemoryEntry {
  if (typeof doc !== 'string') {
    throw new EntryParseError('document is not a string')
  }
  const lines = doc.split('\n')
  if (lines[0]?.trim() !== '---') {
    throw new EntryParseError('missing opening frontmatter delimiter')
  }
  const end = lines.indexOf('---', 1)
  if (end === -1) {
    throw new EntryParseError('missing closing frontmatter delimiter')
  }

  const meta: Record<string, string> = {}
  for (const line of lines.slice(1, end)) {
    const colon = line.indexOf(':')
    if (colon <= 0) continue
    const key = line.slice(0, colon).trim()
    const value = line.slice(colon + 1).trim()
    if (key.length > 0) meta[key] = value
  }

  if (!meta.id) throw new EntryParseError('missing required field: id')
  if (!meta.title) throw new EntryParseError('missing required field: title')

  const layer = meta.layer ?? 'project'
  if (!LAYERS.includes(layer)) {
    throw new EntryParseError(`unknown layer: ${layer}`)
  }

  const schemaVersion = meta.schemaVersion === undefined ? 1 : Number(meta.schemaVersion)
  if (!Number.isInteger(schemaVersion) || schemaVersion < 1) {
    throw new EntryParseError(`invalid schemaVersion: ${meta.schemaVersion}`)
  }

  const content = lines.slice(end + 1).join('\n').replace(/^\n/, '')

  return {
    id: meta.id,
    title: meta.title,
    content,
    domain: meta.domain ?? 'general',
    tags: parseTags(meta.tags),
    layer: layer as Layer,
    schemaVersion,
    createdAt: meta.createdAt ?? nowIso(),
    updatedAt: meta.updatedAt ?? nowIso(),
  }
}
