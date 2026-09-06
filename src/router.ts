import type { MemoryStoreAdapter } from './stores/adapter.js'
import type { Layer } from './types.js'

/**
 * Layer routing policy: project first, global fallback.
 *
 * - `project` and `pending` share the project adapter (pending under a prefix).
 * - `global` is optional; without it the engine degrades to single-layer mode.
 */
export class LayerRouter {
  private readonly project: MemoryStoreAdapter
  private readonly global: MemoryStoreAdapter | undefined

  constructor(project: MemoryStoreAdapter, global?: MemoryStoreAdapter) {
    this.project = project
    this.global = global
  }

  /** Adapter serving a layer, or undefined when the layer is not configured. */
  adapterFor(layer: Layer): MemoryStoreAdapter | undefined {
    if (layer === 'global') return this.global
    return this.project
  }

  /** Read/search order. Project entries shadow global entries with the same id. */
  searchOrder(): Layer[] {
    return this.global ? ['project', 'global'] : ['project']
  }
}
