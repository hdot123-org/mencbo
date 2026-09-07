import posthog from "posthog-js";

// PostHog US project "MenCbo Desktop" — capture-only key, public by design.
const POSTHOG_KEY = "phc_BKjuzVRSBJ26E49hUin3nKxxN2wX8BeD4pSgixUxpcfF";
const POSTHOG_HOST = "https://us.i.posthog.com";

let launchedAt = 0;

export function initAnalytics() {
  if (!POSTHOG_KEY) return;
  try {
    posthog.init(POSTHOG_KEY, {
      api_host: POSTHOG_HOST,
      // Autocapture: every webview click arrives as $autocapture with element
      // metadata. Freeze signal: $autocapture gaps while tray_click (native)
      // keeps flowing → webview layer is hung.
      autocapture: true,
      capture_pageview: false,
      persistence: "memory",
    });
  } catch {
    return; // analytics must never break the app
  }
  launchedAt = Date.now();
  capture("js_launch");

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
