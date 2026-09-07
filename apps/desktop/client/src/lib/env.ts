/**
 * Detect whether we are running inside Tauri.
 * Tauri 2.0+ injects window.isTauri(); fallback to __TAURI_INTERNALS__.
 */
export const inTauri: boolean =
  typeof window !== "undefined" &&
  (typeof (window as any).isTauri === "function"
    ? (window as any).isTauri()
    : typeof (globalThis as any).__TAURI_INTERNALS__ !== "undefined");
