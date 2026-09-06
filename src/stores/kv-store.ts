import type { MemoryStoreAdapter } from './adapter.js'

/**
 * Minimal key-value interface. Compatible with localStorage, sessionStorage,
 * and any synchronous KV shim (e.g. React Native AsyncStorage wrappers).
 */
export interface KVLike {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

/**
 * Adapter over a {@link KVLike} substrate (localStorage & friends).
 *
 * Maintains a small index document so `keys()` works even though plain KV
 * stores cannot enumerate. Engine layer prefixes (`mem:p:` / `mem:pending:` /
 * `mem:g:`) keep multiple stores over the same substrate from colliding.
 */
export class KVStore implements MemoryStoreAdapter {
  readonly name: string
  private readonly kv: KVLike

  constructor(kv: KVLike, name = 'kv') {
    this.kv = kv
    this.name = name
  }

  private get indexKey(): string {
    return `__memengine:${this.name}:index__`
  }

  private readIndex(): Set<string> {
    try {
      const raw: unknown = JSON.parse(this.kv.getItem(this.indexKey) ?? '[]')
      if (!Array.isArray(raw)) return new Set()
      return new Set(raw.filter((key): key is string => typeof key === 'string'))
    } catch {
      return new Set()
    }
  }

  private writeIndex(index: Set<string>): void {
    this.kv.setItem(this.indexKey, JSON.stringify([...index]))
  }

  async get(key: string): Promise<string | undefined> {
    return this.kv.getItem(key) ?? undefined
  }

  async set(key: string, value: string): Promise<void> {
    this.kv.setItem(key, value)
    const index = this.readIndex()
    index.add(key)
    this.writeIndex(index)
  }

  async delete(key: string): Promise<void> {
    this.kv.removeItem(key)
    const index = this.readIndex()
    index.delete(key)
    this.writeIndex(index)
  }

  async keys(): Promise<string[]> {
    return [...this.readIndex()]
  }
}
