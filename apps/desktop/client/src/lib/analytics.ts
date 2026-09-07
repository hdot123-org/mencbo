import posthog from "posthog-js";
import { invoke } from "@tauri-apps/api/core";

// PostHog US project "MenCbo Desktop" — capture-only key, public by design.
const POSTHOG_KEY = "************************************************";
const POSTHOG_HOST = "https://us.i.posthog.com";

let launchedAt = 0;

export async function initAnalytics() {
  if (!POSTHOG_KEY) return;

  try {
    // Fetch identity from Rust: install_id (persistent) + session_id (per-launch)
    const identity = await invoke<{ installId: string; sessionId: string }>("analytics_identity");

    posthog.init(POSTHOG_KEY, {
      api_host: POSTHOG_HOST,
      // Use Rust's install_id as the distinct_id (VAL-ID-001/006)
      bootstrap: {
        distinctID: identity.installId,
        isIdentifiedID: true,
      },
      // Autocapture: every webview click arrives as $autocapture with element
      // metadata. Freeze signal: $autocapture gaps while tray_click (native)
      // keeps flowing → webview layer is hung.
      autocapture: true,
      capture_pageview: false,
      persistence: "memory",
    });

    // Register baseline properties (VAL-ID-002: session_id贯通)
    posthog.register({
      session_id: identity.sessionId,
      source: "webview",
    });

    // js_launch now that identity is ready (VAL-ID-006: no random UUID)
    launchedAt = Date.now();
    capture("js_launch");
  } catch {
    return; // analytics must never break the app
  }

  window.addEventListener("error", (e) =>
    capture("js_error", {
      message: String(e.message).slice(0, 300),
      line: e.lineno,
    }),
  );
  window.addEventListener("unhandledrejection", (e) =>
    capture("js_unhandled_rejection", { reason: String(e.reason).slice(0, 300) }),
  );

  // JS heartbeat (5 min). Compare with rust_heartbeat in PostHog: if rust
  // keeps flowing while js gaps, the hang is in the webview layer.
  window.setInterval(() => {
    capture("js_heartbeat", {
      uptime_sec: Math.round((Date.now() - launchedAt) / 1000),
    });
  }, 5 * 60 * 1000);
}

export function capture(event: string, props?: Record<string, unknown>) {
  try {
    posthog.capture(event, { app: "mencbo-desktop", ...props });
  } catch {
    /* never break the app for analytics */
  }
}
