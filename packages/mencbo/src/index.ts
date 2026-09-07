// Validation comment (VAL-CI-002): engine-only change to verify Desktop workflow path filter skips Client/Tauri jobs. No behavior change.

export { MemoryEngine } from './engine.js'
export type { EngineOptions, ListOptions, MutationOptions, ReadOptions, WriteOptions } from './engine.js'

export { ConflictError, EntryNotFoundError, GuardDeniedError, ValidationError } from './errors.js'

export { OwnershipGuard } from './guard.js'
export type { GuardDecision, GuardOperation, GuardOptions } from './guard.js'

export { LayerRouter } from './router.js'

export { SCHEMA_VERSION, validateEntry } from './schema.js'
export type { ValidationIssue } from './schema.js'

export { EntryParseError, createEntry, newId, nextTimestamp, nowIso, parseEntry, serializeEntry } from './entry.js'

export { MigrationChain } from './migrate.js'
export type { Migration } from './migrate.js'

export { MemoryStore } from './stores/memory-store.js'
export { KVStore } from './stores/kv-store.js'
export type { KVLike } from './stores/kv-store.js'
export type { MemoryStoreAdapter } from './stores/adapter.js'

export type { Domain, Layer, MemoryEntry, SearchHit, SearchQuery, UpdateInput, WriteInput } from './types.js'
