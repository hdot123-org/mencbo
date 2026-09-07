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
 * When Tauri returns {mock: true, tasks: []} (state.json missing or corrupt),
 * we fall back to built-in mock data — same fixture as browser mode.
 * This is required for VAL-CROSS-005: no state.json → show mock + MOCK badge.
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
    const result = await invoke<State>("get_state");
    // VAL-CROSS-005: Rust returns {mock: true, tasks: []} when state.json is
    // missing or corrupt. Frontend must fall back to built-in mock fixture
    // (same as browser mode) so user sees demo data + MOCK badge instead of
    // an empty "暂无任务" screen.
    if (result.mock && (!Array.isArray(result.tasks) || result.tasks.length === 0)) {
      return MOCK_STATE;
    }
    return result;
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
