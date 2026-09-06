import type { State } from "../types";
import { inTauri } from "./env";
import { MOCK_STATE } from "../test/fixtures";

/**
 * Load state from Rust backend (Tauri) or return mock (browser).
 */
export async function loadState(): Promise<State> {
  if (!inTauri) {
    return MOCK_STATE;
  }
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<State>("get_state");
  } catch {
    return MOCK_STATE;
  }
}

/**
 * Subscribe to state changes.
 * In Tauri: listen to "state-changed" events.
 * In browser: no-op (mock is static).
 */
export function subscribeState(cb: (s: State) => void): () => void {
  if (!inTauri) {
    return () => {};
  }
  let unlisten: (() => void) | undefined;
  import("@tauri-apps/api/event").then(({ listen }) => {
    listen<State>("state-changed", (event) => cb(event.payload)).then((un) => {
      unlisten = un;
    });
  });
  return () => {
    unlisten?.();
  };
}

/**
 * Trigger a task to run immediately.
 * In Tauri: invoke Rust command.
 * In browser: no-op (mock mode).
 */
export async function triggerTask(taskId: string): Promise<void> {
  if (!inTauri) {
    console.log("[mock] trigger task:", taskId);
    return;
  }
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("run_task", { task: taskId });
}
