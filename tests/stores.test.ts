import { describe, expect, it } from 'vitest'
import { KVStore, MemoryStore } from '../src/index.js'
import type { KVLike } from '../src/index.js'

class FakeKV implements KVLike {
  readonly map = new Map<string, string>()

  getItem(key: string): string | null {
    return this.map.get(key) ?? null
  }

  setItem(key: string, value: string): void {
    this.map.set(key, value)
  }

  removeItem(key: string): void {
    this.map.delete(key)
  }
}

describe('MemoryStore', () => {
  it('supports crud and key listing', async () => {
    const store = new MemoryStore()
    await store.set('mem:p:1', 'a')
    await store.set('mem:p:2', 'b')
    expect(await store.get('mem:p:1')).toBe('a')
    expect(await store.keys()).toEqual(['mem:p:1', 'mem:p:2'])

    await store.delete('mem:p:1')
    expect(await store.get('mem:p:1')).toBeUndefined()
    expect(await store.keys()).toEqual(['mem:p:2'])
  })
})

describe('KVStore', () => {
  it('maintains an index that survives reloads', async () => {
    const kv = new FakeKV()
    const store = new KVStore(kv, 'app')
    await store.set('mem:p:1', 'a')
    await store.set('mem:p:2', 'b')

    // A fresh instance over the same substrate simulates a page reload.
    const reloaded = new KVStore(kv, 'app')
    expect(await reloaded.get('mem:p:2')).toBe('b')
    expect(await reloaded.keys()).toEqual(['mem:p:1', 'mem:p:2'])

    await reloaded.delete('mem:p:1')
    expect(await reloaded.keys()).toEqual(['mem:p:2'])
    expect(kv.getItem('mem:p:1')).toBeNull()
  })

  it('tolerates a corrupted index', async () => {
    const kv = new FakeKV()
    kv.setItem('__mencbo:broken:index__', '{not json')
    const store = new KVStore(kv, 'broken')
    expect(await store.keys()).toEqual([])
    await store.set('mem:p:1', 'a')
    expect(await store.keys()).toEqual(['mem:p:1'])
  })
})
