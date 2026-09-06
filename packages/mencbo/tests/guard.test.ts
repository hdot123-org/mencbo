import { describe, expect, it } from 'vitest'
import { OwnershipGuard } from '../src/index.js'

describe('OwnershipGuard', () => {
  it('protects system ids by default', () => {
    const guard = new OwnershipGuard()
    expect(guard.check('write', 'system:config').allowed).toBe(false)
    expect(guard.check('update', 'system:config').allowed).toBe(false)
    expect(guard.check('delete', 'system:config').allowed).toBe(false)
    expect(guard.check('write', 'normal-id').allowed).toBe(true)
  })

  it('fails closed on invalid input', () => {
    const guard = new OwnershipGuard()
    expect(guard.check('write', '').allowed).toBe(false)
    expect(guard.check('write', undefined as unknown as string).allowed).toBe(false)
  })

  it('fails closed when a matcher throws', () => {
    const throwingPattern = { test: () => { throw new Error('boom') } } as unknown as RegExp
    const guard = new OwnershipGuard({ protectedPatterns: [throwingPattern] })
    expect(guard.check('write', 'any-id').allowed).toBe(false)
  })

  it('supports protected layers', () => {
    const guard = new OwnershipGuard({ protectedLayers: ['global'] })
    expect(guard.check('write', 'x', 'global').allowed).toBe(false)
    expect(guard.check('write', 'x', 'project').allowed).toBe(true)
  })

  it('supports default deny', () => {
    const guard = new OwnershipGuard({ allowByDefault: false })
    expect(guard.check('write', 'x').allowed).toBe(false)
  })
})
