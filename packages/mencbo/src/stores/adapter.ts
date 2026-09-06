/**
 * Storage contract. Any key-value-ish substrate can back a layer:
 * localStorage / IndexedDB shims / Redis / SQLite / files — you name it.
 *
 * Values are opaque strings (serialized entries); keys are engine-managed.
 */
export interface MemoryStoreAdapter {
  /** Debug name. */
  readonly name: string
  get(key: string): Promise<string | undefined>
  set(key: string, value: string): Promise<void>
  delete(key: string): Promise<void>
  /** All keys currently held by this adapter (engine filters by layer prefix). */
  keys(): Promise<string[]>
}
