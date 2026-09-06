import type { MemoryEntry } from './types.js'

/**
 * A single schema migration step. Steps must be contiguous (`to === from + 1`).
 */
export interface Migration {
  from: number
  to: number
  migrate: (entry: MemoryEntry) => MemoryEntry
}

/**
 * Ordered chain of schema migrations.
 *
 * - Entries are upgraded automatically on read.
 * - Entries written by a *newer* engine version are refused (never silently downgraded).
 */
export class MigrationChain {
  private readonly steps: Migration[]

  constructor(steps: Migration[] = []) {
    this.steps = []
    for (const step of steps) this.register(step)
  }

  /** Register a step. Steps must advance by exactly one version and not duplicate a `from`. */
  register(step: Migration): this {
    if (step.to !== step.from + 1) {
      throw new Error(`migration must advance one version at a time (got ${step.from} -> ${step.to})`)
    }
    if (this.steps.some((existing) => existing.from === step.from)) {
      throw new Error(`duplicate migration from v${step.from}`)
    }
    this.steps.push(step)
    this.steps.sort((a, b) => a.from - b.from)
    return this
  }

  /** The newest schema version this chain can produce. */
  latestVersion(): number {
    const last = this.steps[this.steps.length - 1]
    return last ? last.to : 1
  }

  /** Upgrade an entry to the chain's latest version. */
  apply(entry: MemoryEntry): MemoryEntry {
    const latest = this.latestVersion()
    if (entry.schemaVersion > latest) {
      throw new Error(
        `entry schema v${entry.schemaVersion} is newer than engine v${latest}; upgrade mencbo first`,
      )
    }
    let current = entry
    while (current.schemaVersion < latest) {
      const step = this.steps.find((candidate) => candidate.from === current.schemaVersion)
      if (!step) {
        throw new Error(`missing migration step from v${current.schemaVersion}`)
      }
      current = { ...step.migrate(current), schemaVersion: step.to }
    }
    return current
  }
}
