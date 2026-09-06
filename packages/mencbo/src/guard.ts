import type { Layer } from './types.js'

/** Mutating operations the guard arbitrates. */
export type GuardOperation = 'write' | 'update' | 'delete'

export interface GuardDecision {
  allowed: boolean
  reason: string
}

export interface GuardOptions {
  /** Id patterns that may never be mutated through the engine. Default: `/^system:/`. */
  protectedPatterns?: RegExp[]
  /** Layers that are read-only through the engine (e.g. `'global'`). Default: none. */
  protectedLayers?: Layer[]
  /** Allow ids that match no rule. Default: true. Set false for default-deny. */
  allowByDefault?: boolean
}

/**
 * Ownership guard, ported from memory-core's PreToolUse guard.
 *
 * Fail-closed: if the guard itself misbehaves (bad input, throwing matcher),
 * the operation is denied — protected state is never left exposed.
 */
export class OwnershipGuard {
  private readonly protectedPatterns: RegExp[]
  private readonly protectedLayers: Set<string>
  private readonly allowByDefault: boolean

  constructor(options: GuardOptions = {}) {
    this.protectedPatterns = options.protectedPatterns ?? [/^system:/]
    this.protectedLayers = new Set(options.protectedLayers ?? [])
    this.allowByDefault = options.allowByDefault ?? true
  }

  check(operation: GuardOperation, id: string, layer: Layer = 'project'): GuardDecision {
    try {
      if (typeof id !== 'string' || id.length === 0) {
        return { allowed: false, reason: `guard: invalid id for ${operation}` }
      }
      if (this.protectedLayers.has(layer)) {
        return { allowed: false, reason: `guard: layer "${layer}" is protected` }
      }
      for (const pattern of this.protectedPatterns) {
        if (pattern.test(id)) {
          return { allowed: false, reason: `guard: id "${id}" is protected (${operation})` }
        }
      }
      if (this.allowByDefault) {
        return { allowed: true, reason: `guard: "${id}" allowed for ${operation}` }
      }
      return { allowed: false, reason: `guard: default deny for ${operation}` }
    } catch (error) {
      return { allowed: false, reason: `guard: fail-closed (${String(error)})` }
    }
  }
}
