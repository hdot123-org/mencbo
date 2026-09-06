import type { State } from "../types";
import { inTauri } from "./env";
import { MOCK_STATE, MOCK_STATE_OK, MOCK_STATE_EMPTY } from "../test/fixtures";

/**
 * Get scenario from URL query parameter.
 * Supports: default, ok, empty (default = default if not specified)
 */
function getScenario(): string {
  if (typeof window === "undefined") return "default";
  const params = new URLSearchParams(window.location.search);
  return params.get("scenario") || "default";
}

/**
 * Load state from Rust backend (Tauri) or return mock (browser).
 * In browser mode, respects ?scenario= URL parameter.
 */
export async function loadState(): Promise<State> {
  if (!inTauri) {
    const scenario = getScenario();
    switch (scenario) {
      case "ok":
        return MOCK_STATE_OK;
      case "empty":
        return MOCK_STATE_EMPTY;
      case "default":
      default:
        return MOCK_STATE;
    }
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
