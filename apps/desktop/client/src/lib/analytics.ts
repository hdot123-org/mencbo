import posthog from "posthog-js";
import { invoke } from "@tauri-apps/api/core";

// PostHog US project "MenCbo Desktop" — capture-only key, public by design.
const POSTHOG_KEY = "************************************************";
const POSTHOG_HOST = "https://us.i.posthog.com";

let launchedAt = 0;
let analyticsReady = false;

/**
 * Initialize PostHog analytics with unified identity from Rust.
 *
 * If the identity invoke or posthog.init with bootstrap fails, we:
 * 1. Fall back to a basic posthog.init (no bootstrap, persistence:memory)
 * 2. Capture a `diag_identity_failed` event with the failure reason
 * 3. Still register error listeners and heartbeat (failure must be observable)
 *
 * This ensures the failure path is always visible in PostHog — an architectural
 * invariant (see p0-identity-unification round 3 diagnosis).
 */
export async function initAnalytics() {
  if (!POSTHOG_KEY) return;

  let identityOk = false;
  let installId = "";
  let sessionId = "";
  let failReason = "";

  // Step 1: try to fetch identity from Rust
  try {
    const identity = await invoke<{ installId: string; sessionId: string }>(
      "analytics_identity",
    );
    installId = identity.installId;
    sessionId = identity.sessionId;
    identityOk = true;
  } catch (e) {
    failReason = `invoke_failed: ${String(e).slice(0, 200)}`;
  }

  // Step 2: init posthog — with bootstrap if identity succeeded
  let initOk = false;
  if (identityOk) {
    try {
      posthog.init(POSTHOG_KEY, {
        api_host: POSTHOG_HOST,
        bootstrap: {
          distinctID: installId,
          isIdentifiedID: true,
        },
        autocapture: true,
        capture_pageview: false,
        persistence: "memory",
      });
      initOk = true;
    } catch (e) {
      failReason = `init_bootstrap_failed: ${String(e).slice(0, 200)}`;
    }
  }

  // Step 3: fallback init without bootstrap if anything failed
  if (!initOk) {
    try {
      posthog.init(POSTHOG_KEY, {
        api_host: POSTHOG_HOST,
        autocapture: true,
        capture_pageview: false,
        persistence: "memory",
      });
      initOk = true;
    } catch (e) {
      // Total failure — can't init posthog at all.
      // Still register listeners below so at least the architecture is intact
      // for when/if posthog becomes available later.
      failReason = `init_fallback_also_failed: ${String(e).slice(0, 200)}`;
    }
  }

  // Step 4: register baseline properties and capture diagnostic + launch
  if (initOk) {
    if (identityOk) {
      posthog.register({
        session_id: sessionId,
        source: "webview",
      });
    }

    analyticsReady = true;
    launchedAt = Date.now();

    // If identity failed, capture the diagnostic event first
    if (!identityOk) {
      capture("diag_identity_failed", { reason: failReason });
    } else if (failReason) {
      // Identity was ok but bootstrap init failed
      capture("diag_identity_failed", { reason: failReason });
    }

    // js_launch — now that posthog is initialized
    capture("js_launch");
  }

  // Step 5: ALWAYS register error listeners and heartbeat, regardless of
  // whether identity/init succeeded. This is the architectural invariant:
  // the failure path must be observable.
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
    if (analyticsReady) {
      capture("js_heartbeat", {
        uptime_sec: Math.round((Date.now() - launchedAt) / 1000),
      });
    }
  }, 5 * 60 * 1000);
}

export function capture(event: string, props?: Record<string, unknown>) {
  try {
    posthog.capture(event, { app: "mencbo-desktop", ...props });
  } catch {
    /* never break the app for analytics */
  }
}
