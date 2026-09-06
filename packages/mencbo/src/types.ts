/**
 * Core types for mencbo.
 *
 * The model mirrors the layered memory architecture of memory-core:
 * - `global`  : cross-context knowledge (Layer 2 equivalent)
 * - `project` : one app / workspace / user context (Layer 3 equivalent)
 * - `pending` : auto-captured staging entries waiting to be promoted
 */

/** Knowledge domain. Ships with memory-core's four domains, open for custom strings. */
export type Domain = 'operations' | 'engineering' | 'collaboration' | 'audit' | (string & {})

/** Where an entry currently lives. */
export type Layer = 'global' | 'project' | 'pending'

/** A single memory document. */
export interface MemoryEntry {
  id: string
  title: string
  content: string
  domain: Domain
  tags: string[]
  layer: Layer
  schemaVersion: number
  /** ISO 8601 timestamp. */
  createdAt: string
  /** ISO 8601 timestamp. Doubles as the optimistic-concurrency token (`ifMatch`). */
  updatedAt: string
}

/** Input for {@link MemoryEngine.write}. */
export interface WriteInput {
  /** Explicit id. Omit to auto-generate. */
  id?: string
  title: string
  content: string
  domain?: Domain
  tags?: string[]
}

/** Partial changes for {@link MemoryEngine.update}. */
export type UpdateInput = Partial<Pick<WriteInput, 'title' | 'content' | 'domain' | 'tags'>>

/** Query for {@link MemoryEngine.search}. */
export interface SearchQuery {
  /** Free text. Matched against title (x3), tags (x2) and content (x1), case-insensitive. */
  text?: string
  domains?: string[]
  tags?: string[]
  /** Layers to scan. Defaults to `['project', 'global']` — project first. */
  layers?: Layer[]
  limit?: number
}

/** A search result with relevance score. */
export interface SearchHit {
  entry: MemoryEntry
  score: number
}
