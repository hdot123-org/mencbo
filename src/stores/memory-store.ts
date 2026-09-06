import type { MemoryStoreAdapter } from './adapter.js'

/** In-memory adapter. The default choice for tests and ephemeral sessions. */
export class MemoryStore implements MemoryStoreAdapter {
  readonly name: string
  private readonly map = new Map<string, string>()

  constructor(name = 'memory') {
    this.name = name
  }

  async get(key: string): Promise<string | undefined> {
    return this.map.get(key)
  }

  async set(key: string, value: string): Promise<void> {
    this.map.set(key, value)
  }

  async delete(key: string): Promise<void> {
    this.map.delete(key)
  }

  async keys(): Promise<string[]> {
    return [...this.map.keys()]
  }
}
