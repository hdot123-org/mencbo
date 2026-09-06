import { describe, expect, it } from 'vitest'
import { LayerRouter, MemoryStore } from '../src/index.js'

describe('LayerRouter', () => {
  it('routes project and pending to the project adapter', () => {
    const project = new MemoryStore('project')
    const router = new LayerRouter(project)
    expect(router.adapterFor('project')).toBe(project)
    expect(router.adapterFor('pending')).toBe(project)
  })

  it('returns undefined when no global adapter is configured', () => {
    const router = new LayerRouter(new MemoryStore())
    expect(router.adapterFor('global')).toBeUndefined()
  })

  it('orders project before global', () => {
    const router = new LayerRouter(new MemoryStore('p'), new MemoryStore('g'))
    expect(router.searchOrder()).toEqual(['project', 'global'])
  })

  it('degrades to single-layer order without global', () => {
    const router = new LayerRouter(new MemoryStore('p'))
    expect(router.searchOrder()).toEqual(['project'])
  })
})
