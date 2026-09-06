import type { MemoryEntry } from './types.js'

/** Current entry schema version. Entries are stamped with the engine's latest version on write. */
export const SCHEMA_VERSION = 1

const LAYERS: readonly string[] = ['global', 'project', 'pending']

/** One schema rule violation. */
export interface ValidationIssue {
  field: string
  message: string
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0
}

/** Validate an entry against the schema. Returns all issues found (empty array = valid). */
export function validateEntry(entry: MemoryEntry, maxVersion: number = SCHEMA_VERSION): ValidationIssue[] {
  const issues: ValidationIssue[] = []

  if (!isNonEmptyString(entry.id)) {
    issues.push({ field: 'id', message: 'must be a non-empty string' })
  }
  if (!isNonEmptyString(entry.title)) {
    issues.push({ field: 'title', message: 'must be a non-empty string' })
  } else if (entry.title.includes('\n')) {
    issues.push({ field: 'title', message: 'must not contain line breaks' })
  }
  if (!isNonEmptyString(entry.domain)) {
    issues.push({ field: 'domain', message: 'must be a non-empty string' })
  }
  if (typeof entry.content !== 'string') {
    issues.push({ field: 'content', message: 'must be a string' })
  }
  if (!Array.isArray(entry.tags)) {
    issues.push({ field: 'tags', message: 'must be an array of strings' })
  } else {
    for (const tag of entry.tags) {
      if (!isNonEmptyString(tag) || tag.includes(',')) {
        issues.push({ field: 'tags', message: `invalid tag: ${JSON.stringify(tag)}` })
      }
    }
  }
  if (!LAYERS.includes(entry.layer)) {
    issues.push({ field: 'layer', message: `must be one of ${LAYERS.join(', ')}` })
  }
  if (!Number.isInteger(entry.schemaVersion) || entry.schemaVersion < 1) {
    issues.push({ field: 'schemaVersion', message: 'must be an integer >= 1' })
  } else if (entry.schemaVersion > maxVersion) {
    issues.push({ field: 'schemaVersion', message: `v${entry.schemaVersion} is newer than supported v${maxVersion}` })
  }
  if (!isNonEmptyString(entry.createdAt)) {
    issues.push({ field: 'createdAt', message: 'must be a non-empty ISO string' })
  }
  if (!isNonEmptyString(entry.updatedAt)) {
    issues.push({ field: 'updatedAt', message: 'must be a non-empty ISO string' })
  }

  return issues
}
