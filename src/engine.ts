import { createEntry, newId, nowIso, parseEntry, serializeEntry } from './entry.js'
import { ConflictError, EntryNotFoundError, GuardDeniedError, ValidationError } from './errors.js'
import { OwnershipGuard } from './guard.js'
import type { GuardDecision } from './guard.js'
import { MigrationChain } from './migrate.js'
import { LayerRouter } from './router.js'
import { validateEntry } from './schema.js'
import type { MemoryStoreAdapter } from './stores/adapter.js'
import type { Layer, MemoryEntry, SearchHit, SearchQuery, UpdateInput, WriteInput } from './types.js'

const KEY_PREFIXES = {
  project: 'mem:p:',
  pending: 'mem:pending:',
  global: 'mem:g:',
} as const

function keyFor(id: string, layer: Layer): string {
  const prefix = KEY_PREFIXES[layer]
  return `${prefix}${id}`
}

function prefixFor(layer: Layer): string {
  return KEY_PREFIXES[layer]
}

function definedFields<T extends object>(input: T): Partial<T> {
  const result: Partial<T> = {}
  for (const [key, value] of Object.entries(input)) {
    if (value !== undefined) (result as Record<string, unknown>)[key] = value
  }
  return result
}

function scoreEntry(entry: MemoryEntry, text: string | undefined): number {
  if (!text) return 0
  const needle = text.toLowerCase()
  let score = 0
  if (entry.title.toLowerCase().includes(needle)) score += 3
  if (entry.tags.some((tag) => tag.toLowerCase().includes(needle))) score += 2
  if (entry.content.toLowerCase().includes(needle)) score += 1
  return score
}

function passesFilters(entry: MemoryEntry, query: SearchQuery): boolean {
  if (query.domains?.length && !query.domains.includes(entry.domain)) return false
  if (query.tags?.length && !entry.tags.some((tag) => query.tags?.includes(tag))) return false
  return true
}

export interface EngineOptions {
  /** Required. The app / workspace / user-scoped layer. */
  project: MemoryStoreAdapter
  /** Optional. Cross-context fallback layer. */
  global?: MemoryStoreAdapter
  /** Ownership guard. Defaults to protecting `system:*` ids. */
  guard?: OwnershipGuard
  /** Schema migration chain. Defaults to the current schema version. */
  chain?: MigrationChain
}

export interface WriteOptions {
  /** Write into the pending staging layer instead of project. Default: 'project'. */
  layer?: 'project' | 'pending'
}

/**
 * Read-first discipline, ported from memory-core's read-first CRUD rules:
 * updates and deletes must carry the `updatedAt` token observed on read
 * (optimistic concurrency), or explicitly opt out with `force`.
 */
export interface MutationOptions {
  ifMatch?: string
  force?: boolean
}

export interface ReadOptions {
  /** Also look in the pending staging layer. Default: false. */
  includePending?: boolean
}

export interface ListOptions {
  layer?: Layer
  domain?: string
}

/**
 * Client-side layered memory engine.
 *
 * Routing is project-first, global-fallback: a project entry shadows a global
 * entry with the same id. All mutations pass through the ownership guard and
 * schema validation; reads upgrade entries through the migration chain.
 */
export class MemoryEngine {
  private readonly router: LayerRouter
  private readonly guard: OwnershipGuard
  private readonly chain: MigrationChain

  constructor(options: EngineOptions) {
    this.router = new LayerRouter(options.project, options.global)
    this.guard = options.guard ?? new OwnershipGuard()
    this.chain = options.chain ?? new MigrationChain()
  }

  /** Schema version this engine writes. */
  get schemaVersion(): number {
    return this.chain.latestVersion()
  }

  /** Create an entry. Returns the stored entry (including its generated id). */
  async write(input: WriteInput, options: WriteOptions = {}): Promise<MemoryEntry> {
    const layer = options.layer ?? 'project'
    const id = input.id ?? newId()
    const decision = this.guard.check('write', id, layer)
    if (!decision.allowed) throw new GuardDeniedError(decision)

    const entry = createEntry({ ...input, id }, layer, this.chain.latestVersion())
    const issues = validateEntry(entry, this.chain.latestVersion())
    if (issues.length > 0) throw new ValidationError(issues)

    const adapter = this.requireAdapter(layer)
    await adapter.set(keyFor(id, layer), serializeEntry(entry))
    return entry
  }

  /** Read an entry by id. Project layer first, then global, then pending (opt-in). */
  async read(id: string, options: ReadOptions = {}): Promise<MemoryEntry | undefined> {
    const order: Layer[] = [...this.router.searchOrder()]
    if (options.includePending) order.push('pending')
    for (const layer of order) {
      const entry = await this.readRaw(id, layer)
      if (entry) return entry
    }
    return undefined
  }

  /** Update an entry. Requires `ifMatch` from a prior read, or `force: true`. */
  async update(id: string, changes: UpdateInput, options: MutationOptions = {}): Promise<MemoryEntry> {
    const located = await this.locate(id)
    if (!located) throw new EntryNotFoundError(id)
    this.assertMutation(id, located.entry, options)

    const decision = this.guard.check('update', id, located.layer)
    if (!decision.allowed) throw new GuardDeniedError(decision)

    const updated: MemoryEntry = {
      ...located.entry,
      ...definedFields(changes),
      updatedAt: nowIso(),
    }
    const issues = validateEntry(updated, this.chain.latestVersion())
    if (issues.length > 0) throw new ValidationError(issues)

    const adapter = this.requireAdapter(located.layer)
    await adapter.set(keyFor(id, located.layer), serializeEntry(updated))
    return updated
  }

  /** Delete an entry. Requires `ifMatch` from a prior read, or `force: true`. */
  async delete(id: string, options: MutationOptions = {}): Promise<void> {
    const located = await this.locate(id)
    if (!located) throw new EntryNotFoundError(id)
    this.assertMutation(id, located.entry, options)

    const decision = this.guard.check('delete', id, located.layer)
    if (!decision.allowed) throw new GuardDeniedError(decision)

    await this.requireAdapter(located.layer).delete(keyFor(id, located.layer))
  }

  /**
   * Search across layers. Project results come first by routing order; within
   * the result set, hits are sorted by score (title x3, tags x2, content x1),
   * then by recency. Corrupt documents are skipped.
   */
  async search(query: SearchQuery = {}): Promise<SearchHit[]> {
    const layers = query.layers ?? this.router.searchOrder()
    const limit = query.limit ?? 50
    const hits: SearchHit[] = []

    for (const layer of layers) {
      await this.scan(layer, (entry) => {
        if (!passesFilters(entry, query)) return
        const score = scoreEntry(entry, query.text)
        if (query.text && score === 0) return
        hits.push({ entry, score })
      })
    }

    hits.sort((a, b) => b.score - a.score || b.entry.updatedAt.localeCompare(a.entry.updatedAt))
    return hits.slice(0, limit)
  }

  /** List entries, newest first. Optionally scoped to one layer and/or domain. */
  async list(options: ListOptions = {}): Promise<MemoryEntry[]> {
    const layers: Layer[] = options.layer ? [options.layer] : [...this.router.searchOrder()]
    const entries: MemoryEntry[] = []
    for (const layer of layers) {
      await this.scan(layer, (entry) => {
        if (options.domain && entry.domain !== options.domain) return
        entries.push(entry)
      })
    }
    entries.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
    return entries
  }

  /** Promote a pending entry into the project layer. */
  async promote(id: string): Promise<MemoryEntry> {
    const pending = await this.readRaw(id, 'pending')
    if (!pending) throw new EntryNotFoundError(`pending:${id}`)

    const promoted: MemoryEntry = { ...pending, layer: 'project', updatedAt: nowIso() }
    const decision = this.guard.check('write', id, 'project')
    if (!decision.allowed) throw new GuardDeniedError(decision)

    await this.requireAdapter('project').set(keyFor(id, 'project'), serializeEntry(promoted))
    await this.requireAdapter('pending').delete(keyFor(id, 'pending'))
    return promoted
  }

  private requireAdapter(layer: Layer): MemoryStoreAdapter {
    const adapter = this.router.adapterFor(layer)
    if (!adapter) throw new Error(`no store configured for layer "${layer}"`)
    return adapter
  }

  private async readRaw(id: string, layer: Layer): Promise<MemoryEntry | undefined> {
    const adapter = this.router.adapterFor(layer)
    if (!adapter) return undefined
    const raw = await adapter.get(keyFor(id, layer))
    if (raw === undefined) return undefined
    return this.chain.apply(parseEntry(raw))
  }

  /** Locate an entry across layers: project first, then global, then pending. */
  private async locate(id: string): Promise<{ entry: MemoryEntry; layer: Layer } | undefined> {
    const order: Layer[] = ['project', 'global', 'pending']
    for (const layer of order) {
      const entry = await this.readRaw(id, layer)
      if (entry) return { entry, layer }
    }
    return undefined
  }

  private assertMutation(id: string, entry: MemoryEntry, options: MutationOptions): void {
    if (options.force) return
    if (options.ifMatch === undefined) {
      throw new ConflictError(
        `${id}: mutation requires ifMatch (pass entry.updatedAt from your read) or { force: true }`,
      )
    }
    if (options.ifMatch !== entry.updatedAt) {
      throw new ConflictError(
        `${id}: stale ifMatch (expected ${entry.updatedAt}, got ${options.ifMatch}); re-read and retry`,
      )
    }
  }

  private async scan(layer: Layer, visit: (entry: MemoryEntry) => void): Promise<void> {
    const adapter = this.router.adapterFor(layer)
    if (!adapter) return
    const prefix = prefixFor(layer)
    for (const key of await adapter.keys()) {
      if (!key.startsWith(prefix)) continue
      const raw = await adapter.get(key)
      if (raw === undefined) continue
      try {
        visit(this.chain.apply(parseEntry(raw)))
      } catch {
        // Corrupt or future-schema documents are skipped during scans; read() surfaces the error.
      }
    }
  }
}
